//! CalDAV download (RFC 4791): discover the account's calendars, then
//! keep each one in step with `sync-collection`, one resource per event
//! series. Fastmail, iCloud, Nextcloud and Google's CalDAV all answer it.

pub mod dav;

use anyhow::{Context, Result};
use datalib_etl::control::DownloadControl;
use datalib_etl::dav::sync::{CollectionSync, Page};
use datalib_etl::download_problems::{self, RecordProblem, RunProblem};
use datalib_etl::http::LatchkeySettings;
use datalib_etl::progress::Progress;
use tracing::info;

use super::db::RawDb;
use super::schema_raw::{AccountRow, CalendarRow, IcsObjectRow};
use super::{select_calendars, FetchSummary, Window};
use crate::ical;
use dav::{CalendarProps, Multistatus};

pub struct FetchOptions {
    /// The store this run writes into, opened and closed by the caller.
    pub db: RawDb,
    pub server_url: String,
    /// Calendar names or ids to mirror; empty for all of them.
    pub calendars: Vec<String>,
    /// Only what falls in these days; `None` mirrors everything.
    pub window: Option<Window>,
    pub latchkey: LatchkeySettings,
    pub progress: Progress,
    pub control: DownloadControl,
}

pub async fn fetch(opts: FetchOptions) -> Result<FetchSummary> {
    let db = &opts.db;
    let mut summary = FetchSummary::default();
    let lk = &opts.latchkey;

    let Reached {
        found,
        account_id,
        calendars,
    } = reach(&opts.server_url, lk, &mut summary).await?;
    db.upsert_account(&AccountRow {
        id: account_id.clone(),
        method: "caldav".into(),
        server_url: Some(opts.server_url.clone()),
        principal_href: Some(found.principal_url.clone()),
        login: found.login.clone(),
    })
    .await?;
    info!(
        event = "caldav_discovery",
        principal = %found.principal_url,
        home = %found.home_url,
        "discovered the principal and its calendar home"
    );

    db.upsert_calendars(&calendars.iter().map(|c| c.row.clone()).collect::<Vec<_>>())
        .await?;

    let selected = select_calendars(
        db,
        &opts.calendars,
        calendars
            .iter()
            .map(|c| (&c.row.id, c.row.display_name.as_deref())),
    )
    .await?;
    summary.calendars = selected.len();

    let mut run_problems: Vec<RunProblem> = Vec::new();
    let mut record_problems: Vec<RecordProblem> = Vec::new();
    for cal in calendars.iter().filter(|c| selected.contains(&c.row.id)) {
        if opts.control.stop.requested() {
            break;
        }
        let label = cal.row.display_name.as_deref().unwrap_or(&cal.row.id);
        opts.progress
            .set_message(&format!("syncing calendar {label}"));
        let synced = sync_calendar(
            db,
            cal,
            opts.window.as_ref(),
            lk,
            &mut summary,
            &mut record_problems,
        )
        .await;
        let listing = format!("calendar {label}");
        match synced {
            Ok(None) => {}
            Ok(Some(cut_short)) => run_problems.push(RunProblem::listing(
                &listing,
                format!("{cut_short}; nothing was deleted"),
            )),
            Err(e) => {
                summary.errors += 1;
                run_problems.push(RunProblem::listing(&listing, format!("{e:#}")));
            }
        }
    }
    // A stop leaves the rest unsynced; their last rows stand.
    if !opts.control.stop.requested() {
        download_problems::report_run(db.pool(), &run_problems).await;
        download_problems::report_records(db.pool(), &record_problems).await;
    }
    Ok(summary)
}

/// What discovery and the calendar listing found: everything a run
/// needs before it syncs, and all a probe reports.
pub(crate) struct Reached {
    pub(crate) found: Discovered,
    pub(crate) account_id: String,
    pub(crate) calendars: Vec<Calendar>,
}

pub(crate) async fn reach(
    server_url: &str,
    lk: &LatchkeySettings,
    summary: &mut FetchSummary,
) -> Result<Reached> {
    let found = discover(server_url, lk, summary).await?;
    let account_id = dav::origin(&found.home_url)
        .and_then(|o| o.split("://").nth(1))
        .unwrap_or("caldav")
        .to_string();
    summary.requests += 1;
    let listing = dav::propfind(&found.home_url, "1", dav::BODY_LIST_CALENDARS, lk)
        .await
        .map_err(|e| anyhow::anyhow!("list calendars: {e}"))?;
    let calendars = calendars_in(&account_id, &found.home_url, &listing);
    Ok(Reached {
        found,
        account_id,
        calendars,
    })
}

pub(crate) struct Discovered {
    principal_url: String,
    home_url: String,
    pub(crate) login: Option<String>,
}

async fn discover(
    server_url: &str,
    lk: &LatchkeySettings,
    summary: &mut FetchSummary,
) -> Result<Discovered> {
    let principal_url = datalib_etl::dav::find_principal(
        dav::HTTP_SERVICE,
        server_url,
        "caldav",
        lk,
        &mut summary.requests,
    )
    .await?;

    summary.requests += 1;
    let ms = dav::propfind(&principal_url, "0", dav::BODY_PRINCIPAL, lk)
        .await
        .map_err(|e| anyhow::anyhow!("propfind calendar-home-set: {e}"))?;
    let home = ms
        .responses
        .iter()
        .find_map(|r| r.props.calendar_home_set.clone())
        .context("the principal has no calendar-home-set")?;
    let home_url = dav::absolutize(&principal_url, &home).context("calendar home URL")?;
    let login = ms
        .responses
        .iter()
        .flat_map(|r| r.props.user_addresses.iter())
        .find_map(|a| {
            a.strip_prefix("mailto:")
                .or_else(|| a.strip_prefix("MAILTO:"))
        })
        .map(str::to_string)
        .or_else(|| last_segment(&principal_url));
    Ok(Discovered {
        principal_url,
        home_url,
        login,
    })
}

pub(crate) struct Calendar {
    pub(crate) row: CalendarRow,
    url: String,
}

/// The collections of a home listing that hold events.
fn calendars_in(account_id: &str, home_url: &str, listing: &Multistatus) -> Vec<Calendar> {
    listing
        .responses
        .iter()
        .filter(|r| r.props.is_calendar)
        .filter(|r| {
            r.props.components.is_empty() || r.props.components.iter().any(|c| c == "VEVENT")
        })
        .filter_map(|r| {
            let id = last_segment(&r.href)?;
            Some(Calendar {
                url: dav::absolutize(home_url, &r.href)?,
                row: CalendarRow {
                    id,
                    account_id: account_id.to_string(),
                    href: Some(r.href.clone()),
                    display_name: r.props.display_name.clone(),
                    description: r.props.description.clone(),
                    color: r.props.color.clone(),
                    time_zone: r.props.calendar_timezone.as_deref().and_then(timezone_id),
                },
            })
        })
        .collect()
}

/// The TZID of a `calendar-timezone` value.
fn timezone_id(vcalendar: &str) -> Option<String> {
    ical::parse(vcalendar)
        .iter()
        .flat_map(|c| c.children_named("VTIMEZONE"))
        .find_map(|tz| tz.text("TZID"))
}

fn last_segment(href: &str) -> Option<String> {
    href.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && !s.contains("://"))
        .map(str::to_string)
}

/// Keeps one calendar in step: with `sync-collection` from its stored
/// token, or with `calendar-query` over the window, or whole where the
/// server has no `sync-collection`. Returns why the listing stopped
/// short, if it did; then nothing it did not name is deleted.
async fn sync_calendar(
    db: &RawDb,
    cal: &Calendar,
    window: Option<&Window>,
    lk: &LatchkeySettings,
    summary: &mut FetchSummary,
    problems: &mut Vec<RecordProblem>,
) -> Result<Option<String>> {
    let id = &cal.row.id;
    let mut sync = match window {
        Some(w) => CollectionSync::query(&dav::KIND, &cal.url, dav::body_query_window(w), lk),
        None => CollectionSync::new(
            &dav::KIND,
            &cal.url,
            db.sync_token(id).await?,
            Some(dav::BODY_QUERY_ALL_EVENTS.to_string()),
            lk,
        ),
    };
    let listed = store_pages(db, cal, &mut sync, summary, problems).await;
    summary.requests += sync.requests();
    listed?;
    let outcome = sync.finish();
    let stored = db.ics_hrefs(id).await?;
    let gone: Vec<String> = outcome
        .unlisted(stored.keys())
        .into_iter()
        .filter_map(|href| stored.get(href).cloned())
        .collect();
    summary.events_deleted += gone.len();
    db.delete_ics_uids(id, &gone).await?;
    Ok(outcome.cut_short().map(str::to_string))
}

async fn store_pages(
    db: &RawDb,
    cal: &Calendar,
    sync: &mut CollectionSync<'_>,
    summary: &mut FetchSummary,
    problems: &mut Vec<RecordProblem>,
) -> Result<()> {
    while let Some(page) = sync.next_page::<CalendarProps>().await? {
        let token = page.token.clone();
        apply(db, cal, page, summary, problems).await?;
        db.set_sync_token(&cal.row.id, token.as_deref()).await?;
    }
    Ok(())
}

/// Store what one page changed and drop what it deleted.
async fn apply(
    db: &RawDb,
    cal: &Calendar,
    page: Page<CalendarProps>,
    summary: &mut FetchSummary,
    problems: &mut Vec<RecordProblem>,
) -> Result<()> {
    let id = &cal.row.id;
    let stored = db.ics_hrefs(id).await?;
    for href in &page.unfetched {
        summary.errors += 1;
        problems.push(RecordProblem::new(
            "ics_objects",
            href,
            "the calendar listed this object, but did not return it when asked",
        ));
    }
    let mut rows: Vec<IcsObjectRow> = Vec::with_capacity(page.changed.len());
    for r in &page.changed {
        let data = r.props.calendar_data.as_deref().unwrap_or_default();
        let Some(uid) = ical::first_event_uid(data) else {
            summary.errors += 1;
            problems.push(RecordProblem::new(
                "ics_objects",
                &r.href,
                "the calendar object has no VEVENT with a UID, so it cannot be stored",
            ));
            continue;
        };
        match stored.get(&r.href) {
            Some(_) => summary.events_updated += 1,
            None => summary.events_new += 1,
        }
        rows.push(IcsObjectRow::new(
            id,
            &uid,
            Some(r.href.clone()),
            r.props.etag.clone(),
            data,
        ));
    }
    db.upsert_ics_objects(&rows).await?;
    let deleted: Vec<String> = page
        .deleted
        .iter()
        .filter_map(|href| stored.get(href).cloned())
        .collect();
    summary.events_deleted += deleted.len();
    db.delete_ics_uids(id, &deleted).await
}

#[cfg(test)]
mod tests {
    use super::dav::{CalendarProps, DavResponse};
    use super::*;

    #[test]
    fn a_home_listing_keeps_only_event_calendars() {
        let listing = Multistatus {
            responses: vec![
                DavResponse {
                    href: "/dav/calendars/user/p/".into(),
                    ..Default::default()
                },
                DavResponse {
                    href: "/dav/calendars/user/p/bridge-uuid/".into(),
                    props: CalendarProps {
                        is_calendar: true,
                        display_name: Some("Bridge".into()),
                        components: vec!["VEVENT".into()],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                DavResponse {
                    href: "/dav/calendars/user/p/tasks/".into(),
                    props: CalendarProps {
                        is_calendar: true,
                        components: vec!["VTODO".into()],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                DavResponse {
                    href: "/dav/calendars/user/p/Inbox/".into(),
                    ..Default::default()
                },
            ],
            sync_token: None,
        };
        let cals = calendars_in(
            "caldav.test",
            "https://caldav.test/dav/calendars/user/p/",
            &listing,
        );
        assert_eq!(cals.len(), 1);
        assert_eq!(cals[0].row.id, "bridge-uuid");
        assert_eq!(
            cals[0].url,
            "https://caldav.test/dav/calendars/user/p/bridge-uuid/"
        );
    }

    #[test]
    fn a_calendar_timezone_names_its_zone() {
        assert_eq!(
            timezone_id("BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Zurich\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n").as_deref(),
            Some("Europe/Zurich")
        );
    }
}
