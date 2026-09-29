//! What the Apple Photos page says, from what [`parse`](super::parse)
//! read. It counts the library's photos and videos: the rows of
//! `ZASSET`, less those in the Recently Deleted album.

use datalib_etl_render::text::thousands;
use datalib_etl_summary_render::{Breakdown, Order, Summary, Tally};
use datalib_schema::problems::{Problem, Reason, Severity};
use datalib_time::IsoOffsetTimestamp;

use super::parse::{Parsed, ASSETS};

/// How many album titles the page lists before it only counts.
const ALBUMS_SHOWN: usize = 20;

pub fn summarize(p: &Parsed) -> Summary {
    let Some(assets) = p.assets else {
        return Summary {
            lede: format!(
                "This mirror has no `{ASSETS}` table, so there are no photos to count. \
                 Is the ingest pointed at a Photos library, and is the table left out by \
                 `include_tables` or `exclude_tables`?"
            ),
            problems: vec![Problem::field(
                ASSETS,
                Reason::NotFound,
                "the mirror has no such table",
            )
            .severity(Severity::Warning)],
            ..Default::default()
        };
    };
    let earliest = p.earliest_ms.and_then(iso);
    let latest = p.latest_ms.and_then(iso);
    let mut lede = format!(
        "{} photo{} and video{} in the library.",
        thousands(assets),
        if assets == 1 { "" } else { "s" },
        if assets == 1 { "" } else { "s" },
    );
    if let (Some(a), Some(b)) = (&earliest, &latest) {
        lede.push_str(&format!(" Taken {} to {} (UTC).", &a[..10], &b[..10]));
    }
    let mut facts = vec![
        ("Photos and videos".to_string(), thousands(assets)),
        ("Favorites".to_string(), thousands(p.favorites)),
        ("Hidden".to_string(), thousands(p.hidden)),
        ("With a location".to_string(), thousands(p.located)),
        ("Recently deleted".to_string(), thousands(p.trashed)),
    ];
    if !p.albums.is_empty() {
        facts.push(("Albums".to_string(), albums_line(&p.albums)));
    }
    let table = |heading, key, tally: &Tally, order| Breakdown {
        heading,
        key,
        noun: "Items",
        tally: tally.clone(),
        bytes: false,
        order,
        limit: None,
    };
    Summary {
        item_count: assets,
        lede,
        facts,
        breakdowns: vec![
            table("By kind", "Kind", &p.by_kind, Order::ByCount),
            table("By year taken", "Year", &p.by_year, Order::ByLabel),
            table("By file type", "Type", &p.by_type, Order::ByCount),
        ],
        earliest,
        latest,
        ..Default::default()
    }
}

fn iso(ms: i64) -> Option<String> {
    IsoOffsetTimestamp::from_unix_millis(ms).map(|t| t.to_rfc3339())
}

fn albums_line(albums: &[String]) -> String {
    let shown = albums
        .iter()
        .take(ALBUMS_SHOWN)
        .cloned()
        .collect::<Vec<_>>();
    let more = albums.len().saturating_sub(ALBUMS_SHOWN);
    let mut line = format!("{}: {}", albums.len(), shown.join(", "));
    if more > 0 {
        line.push_str(&format!(", and {more} more"));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mirror_with_no_assets_table_says_so_and_reports_it() {
        let s = summarize(&Parsed::default());
        assert_eq!(s.item_count, 0);
        let [p] = s.problems.as_slice() else {
            panic!("{:?}", s.problems)
        };
        assert_eq!(p.field.as_deref(), Some(ASSETS));
    }

    /// The Recently Deleted album is on its way out of the library, so
    /// it is shown beside the count, not in it.
    #[test]
    fn the_count_leaves_out_the_recently_deleted() {
        let s = summarize(&Parsed {
            assets: Some(4),
            trashed: 1,
            ..Default::default()
        });
        assert_eq!(s.item_count, 4);
        assert!(s
            .facts
            .contains(&("Recently deleted".to_string(), "1".to_string())));
    }
}
