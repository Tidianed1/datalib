//! JMAP downloader. State-token-first incremental sync over four
//! phases:

pub mod api;
pub mod db;
pub mod envelope;
pub mod gmail_api;
pub mod labels;
pub mod mbox;
pub mod schema_raw;
pub mod session;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use datalib_etl::blob_cas::{CasEdgeAccumulator, CasEdgeRow as _};
use datalib_etl::bulk::bulk_upsert_in_tx;
use datalib_etl::download_problems::RunProblem;
use datalib_etl::download_run::DownloadRun;
use datalib_etl::http::LatchkeySettings;
use datalib_etl::progress::{Progress, RunBar};
use datalib_etl::run_problems::{self, RunProblems};
use datalib_time::IsoOffsetTimestamp;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

pub use db::{block_on_load_all, db_path_for, LoadedRaw, RawDb};

use api::call;
use datalib_etl::doltlite_raw as dr;
use db::refresh_email_joins;
use schema_raw::{AccountRow, EmailRow, EmlBlobRow, MailboxRow, ThreadRow, MAILBOX_VOLATILE_PATHS};

async fn upsert_account(
    db: &RawDb,
    now: &IsoOffsetTimestamp,
    id: &str,
    payload: &Value,
) -> Result<()> {
    let row = AccountRow::from_jmap_payload(id, payload)?;
    let mut tx = db.pool().begin().await.context("begin account tx")?;
    bulk_upsert_in_tx(&mut tx, std::slice::from_ref(&row), now).await?;
    tx.commit().await.context("commit account tx")?;
    Ok(())
}

async fn upsert_mailboxes(
    db: &RawDb,
    now: &IsoOffsetTimestamp,
    account_id: &str,
    payloads: &[Value],
) -> Result<()> {
    if payloads.is_empty() {
        return Ok(());
    }
    let mut rows: Vec<MailboxRow> = Vec::with_capacity(payloads.len());
    let mut volatile: Vec<(String, Value)> = Vec::new();
    for p in payloads {
        let (content, counts) = dr::split_volatile(p, MAILBOX_VOLATILE_PATHS);
        let row = MailboxRow::from_jmap_payload(account_id, &content)?;
        if let Some(counts) = counts {
            volatile.push((row.id_and_payload.id.clone(), counts));
        }
        rows.push(row);
    }
    let volatile: Vec<(&str, &Value)> = volatile.iter().map(|(id, v)| (id.as_str(), v)).collect();
    let mut tx = db.pool().begin().await.context("begin mailboxes tx")?;
    bulk_upsert_in_tx(&mut tx, &rows, now).await?;
    dr::set_volatile_payloads_in_tx(&mut tx, "mailboxes", &volatile).await?;
    tx.commit().await.context("commit mailboxes tx")?;
    Ok(())
}

/// Move every email filed under each `from` mailbox to its `to`, or off
/// it when `to` is `None`, then drop the `from` row. Payload and join rows
/// move together, so the `emails` diff and the `email_mailboxes` diff tell
/// the same story. Returns how many emails moved.
///
/// For a label that went away upstream (`None`), and for a row whose id
/// changed recipe while the label stayed (`Some`).
pub(crate) async fn refile_mailboxes(
    db: &RawDb,
    now: &IsoOffsetTimestamp,
    moves: &[(String, Option<String>)],
) -> Result<usize> {
    let mut moved = 0;
    for (from, to) in moves {
        let mut after = String::new();
        loop {
            let batch = db.emails_filed_under(from, &after, REFILE_BATCH).await?;
            let Some((last, _, _)) = batch.last() else {
                break;
            };
            after = last.clone();
            let rows: Vec<EmailRow> = batch
                .into_iter()
                .filter_map(|(_, account, payload)| {
                    EmailRow::from_jmap_envelope(&account, &refiled(payload, from, to.as_deref()))
                })
                .collect();
            upsert_emails(db, now, &rows).await?;
            moved += rows.len();
        }
    }
    let gone: Vec<String> = moves.iter().map(|(from, _)| from.clone()).collect();
    db.delete_mailboxes(&gone).await?;
    if !gone.is_empty() {
        info!(
            event = "email_mailboxes_refiled",
            mailboxes = gone.len(),
            emails = moved,
            "moved emails off mailboxes that are gone or re-keyed",
        );
    }
    Ok(moved)
}

/// Emails per transaction when [`refile_mailboxes`] rewrites them.
const REFILE_BATCH: usize = 500;

fn refiled(mut payload: Value, from: &str, to: Option<&str>) -> Value {
    if let Some(ids) = payload.get_mut("mailboxIds").and_then(Value::as_object_mut) {
        ids.remove(from);
        if let Some(to) = to {
            ids.insert(to.to_string(), Value::Bool(true));
        }
    }
    payload
}

/// The mailbox rows a complete listing of an account's mailboxes leaves
/// behind: every row of `held` it did not name.
fn unlisted_mailboxes<'a>(
    held: impl IntoIterator<Item = &'a String>,
    listed: &HashSet<String>,
) -> Vec<(String, Option<String>)> {
    held.into_iter()
        .filter(|id| !listed.contains(*id))
        .map(|id| (id.clone(), None))
        .collect()
}

async fn upsert_threads(db: &RawDb, now: &IsoOffsetTimestamp, rows: &[ThreadRow]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut tx = db.pool().begin().await.context("begin threads tx")?;
    bulk_upsert_in_tx(&mut tx, rows, now).await?;
    tx.commit().await.context("commit threads tx")?;
    Ok(())
}

async fn upsert_emails(db: &RawDb, now: &IsoOffsetTimestamp, rows: &[EmailRow]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut tx = db.pool().begin().await.context("begin emails tx")?;
    bulk_upsert_in_tx(&mut tx, rows, now).await?;
    for row in rows {
        refresh_email_joins(&mut tx, row).await?;
    }
    tx.commit().await.context("commit emails tx")?;
    Ok(())
}
use session::Session;

/// Batch size for `Email/get` detail fetches. JMAP servers typically
/// cap a single `Email/get` at ~500 ids; we stay well below to keep
/// per-call latency bounded.
const EMAIL_GET_BATCH: usize = 50;
/// Batch size for `Email/query` enumeration (full-resync fallback).
const EMAIL_QUERY_PAGE: usize = 500;
/// Batch size for `Thread/get`.
const THREAD_GET_BATCH: usize = 100;
/// `Email/changes` `maxChanges` ceiling — large enough to drain a
/// month of activity in one call, small enough that a single response
/// stays under a megabyte.
const CHANGES_MAX: u64 = 5_000;
/// Per-request timeout for blob downloads. Big attachments take time.
const BLOB_TIMEOUT: Duration = Duration::from_secs(180);
/// Default number of `.eml` downloads to keep in flight in the blob
/// phase when the config leaves `blob_download_concurrency` unset. JMAP
/// has no bulk-download method, so concurrency is the only lever for a
/// large initial backfill; this value is a polite-but-useful fan-out
/// against the download endpoint. Override per-source in the `sync:`
/// block; set `1` to restore strictly-serial fetching.
const DEFAULT_BLOB_CONCURRENCY: usize = 8;

/// Envelope-only `Email/get` properties. Body parts (`bodyValues`,
/// `textBody`, `htmlBody`, `preview`) are deliberately omitted: the
/// canonical body source is the `.eml` blob in the shared CAS, and
/// render `mail-parse`s it on demand so the JMAP and mbox sources
/// feed identical inputs into the renderer.
const EMAIL_GET_PROPERTIES: &[&str] = &[
    "id",
    "blobId",
    "threadId",
    "mailboxIds",
    "keywords",
    "from",
    "subject",
    "sentAt",
    "receivedAt",
    "size",
    "messageId",
    "hasAttachment",
    "attachments",
];

#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// Which latchkey identity the download authenticates as, from the
    /// source's `latchkey_settings:` block.
    pub latchkey: LatchkeySettings,
    /// The store this run writes into, opened and closed by the caller.
    /// A download never opens a store of its own: one writer per file
    /// (`datalib/backend/etl/README.md` § "One writer per file, by
    /// construction").
    pub db: RawDb,
    /// Seals a flushed batch, so render can start on the mail already
    /// mirrored while the walk continues. `None` commits once at the end.
    pub sealer: Option<datalib_etl::raw_store::Sealer>,
    pub hostname: String,
    pub account_id: Option<String>,
    /// Skip stored `state` tokens and re-enumerate via `Email/query`.
    /// Mailboxes still re-fetch via `Mailbox/get`.
    pub full_resync: bool,
    /// When non-empty, restrict the sync to mailboxes whose full label
    /// path (POSIX-like, e.g. `Work/Projects`; see
    /// [`crate::mailbox_labels`]) exactly matches one of these. Empty =
    /// every mailbox the account exposes. The paths are resolved to
    /// JMAP mailbox ids once `Mailbox/get` has run, then the filter is
    /// pushed server-side on full enumeration and applied client-side
    /// after `Email/get` on the incremental path (since `Email/changes`
    /// is account-scoped on JMAP).
    pub only_mailbox_labels: Vec<String>,
    /// Skip downloading any blob whose advertised size exceeds this.
    /// `None` = no limit.
    pub blob_size_limit_bytes: Option<u64>,
    /// How many `.eml` downloads to keep in flight at once during the
    /// blob phase. `None` → [`DEFAULT_BLOB_CONCURRENCY`]; clamped to ≥ 1.
    pub blob_download_concurrency: Option<usize>,
    pub progress: Progress,
    /// Cross-provider knobs (the checkpoint cadence, the stop flag).
    pub control: datalib_etl::control::DownloadControl,
}

impl FetchOptions {
    /// Every field defaulted except the store, which has none to give:
    /// it is a live handle the caller opens and closes.
    pub fn new(db: RawDb) -> Self {
        Self {
            latchkey: LatchkeySettings::default(),
            db,
            sealer: None,
            hostname: String::new(),
            account_id: None,
            full_resync: false,
            only_mailbox_labels: Vec::new(),
            blob_size_limit_bytes: None,
            blob_download_concurrency: None,
            progress: Progress::noop(),
            control: datalib_etl::control::DownloadControl::default(),
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct FetchSummary {
    /// Configured mailbox paths this account does not have. Reported
    /// rather than fatal, the same as every other provider's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<datalib_etl::download_problems::DownloadProblem>,
    pub account_id: String,
    pub mailboxes_upserted: usize,
    pub mailboxes_destroyed: usize,
    pub emails_upserted: usize,
    pub emails_destroyed: usize,
    pub threads_upserted: usize,
    pub blobs_downloaded: usize,
    pub blobs_skipped: usize,
    pub blobs_errored: usize,
    pub blobs_oversize: usize,
}

/// Scope key for this provider's [`datalib_etl::scope_config`] blob.
/// Matches the `jmap:` prefix the state tokens use.
const SCOPE_CONFIG_KEY: &str = "jmap:download";

/// Blob key. Named so writer and reader can't drift, and the config key
/// a [`datalib_etl::download_problems::DownloadProblem`] names.
pub(crate) const K_ONLY_EXTRACT_LABELS: &str = "only_extract_labels";

/// The subset of [`FetchOptions`] that decides which data lands on disk.
/// Only the label filter qualifies: `hostname`/`account_id` re-key the
/// whole store, `full_resync` is a one-off override, and
/// `blob_download_concurrency` is a throughput knob. The blob cap isn't
/// here either — `sync_blobs` re-scans every email for missing bytes on
/// every run, so raising it already backfills on its own.
fn scope_config_blob(opts: &FetchOptions) -> Value {
    // Sorted so a reordered config list isn't mistaken for a change.
    let mut labels: Vec<&str> = opts
        .only_mailbox_labels
        .iter()
        .map(String::as_str)
        .collect();
    labels.sort_unstable();
    json!({ K_ONLY_EXTRACT_LABELS: labels })
}

/// Session, mailboxes, emails, threads, blobs: the five ticks the outer
/// bar makes whatever the run turns out to hold.
const PHASES: u64 = 5;

/// The listing and phase names a `problems` row carries.
const M_MAILBOX_GET: &str = "Mailbox/get";
const M_EMAIL_QUERY: &str = "Email/query";
const M_THREAD_GET: &str = "Thread/get";

/// `.eml` downloads in a row that may fail before the run stops: past
/// this many, something is wrong with every download, not with one.
const BLOB_FAILURE_BUDGET: usize = 20;

pub async fn fetch(opts: FetchOptions) -> Result<FetchSummary> {
    let (pool, stop) = (opts.db.pool().clone(), opts.control.stop.clone());
    let sealer = opts.sealer.clone();
    run_problems::collecting_sealed(&pool, &stop, sealer.as_ref(), |found| {
        sync_account(opts, found)
    })
    .await
}

async fn sync_account(opts: FetchOptions, found: RunProblems) -> Result<FetchSummary> {
    let db = opts.db.clone();

    // Coarse per-phase progress so the bar moves even though we don't
    // have a meaningful per-item denominator before the first JMAP
    // response. Without this, fastmail looks stuck at 0/0 in the
    // dashboard whether it's running or wedged on Session::discover.
    // Each phase below adds its own real total to this as it learns it.
    let bar = RunBar::new(&opts.progress, PHASES);
    bar.doing("session");

    let session = Session::discover(&opts.hostname, &opts.latchkey)
        .await
        .with_context(|| format!("discover JMAP session at {}", opts.hostname))?;
    bar.did(1);
    let account_id = session.pick_account(opts.account_id.as_deref())?;
    info!(
        event = "jmap_session",
        hostname = %opts.hostname,
        account_id = %account_id,
        api_url = %session.api_url,
        "opened the JMAP session"
    );

    // Stamp the run + a record of the account itself.
    let run = DownloadRun::start(
        db.pool(),
        &json!({
            "hostname": opts.hostname,
            "account_id": account_id,
            "full_resync": opts.full_resync,
            "only_mailbox_labels": opts.only_mailbox_labels,
        }),
    )
    .await?;

    // Diff the scope-affecting params against the ones that produced the
    // stored `Email/changes` cursor. `None` (fresh store, or one written
    // before `sync_scope_config` existed) plans no backfill.
    let scope_cfg = scope_config_blob(&opts);
    let prior_scope_cfg =
        datalib_etl::scope_config::load_or_none(db.pool(), SCOPE_CONFIG_KEY).await;
    let label_change = datalib_etl::scope_config::filter_widened(
        prior_scope_cfg.as_ref(),
        K_ONLY_EXTRACT_LABELS,
        &opts.only_mailbox_labels,
    );

    let result = run_sync(
        &db,
        &session,
        &account_id,
        &opts,
        &label_change,
        &bar,
        &found,
    )
    .await;
    bar.finish();
    // Record the config only once the run satisfied it, so a failure —
    // or a run that stopped when asked, with mailboxes still unwalked —
    // leaves the previous label set in place and the next run re-plans
    // the backfill.
    datalib_etl::scope_config::store_if_satisfied(
        db.pool(),
        SCOPE_CONFIG_KEY,
        &scope_cfg,
        result.is_ok() && !opts.control.stop.requested(),
    )
    .await;
    // On error we still serialize a partial-summary stub so the row
    // has fields for grafana-style dashboards to graph. The summary
    // type is the same on both paths; on error its fields will simply
    // be the defaults populated up to the failure point.
    let summary_for_bookkeeping = result.as_ref().cloned().unwrap_or_default();
    run.finish(&result, &summary_for_bookkeeping).await;
    result
}

async fn run_sync(
    db: &RawDb,
    session: &Session,
    account_id: &str,
    opts: &FetchOptions,
    // How `only_extract_labels` moved since the run that produced the
    // stored cursor. `Email/changes` can't surface existing mail in
    // newly-admitted mailboxes — nothing in them *changed* — so a
    // widening needs its own enumeration.
    label_change: &datalib_etl::scope_config::FilterChange,
    bar: &RunBar,
    found: &RunProblems,
) -> Result<FetchSummary> {
    let mut summary = FetchSummary {
        account_id: account_id.to_string(),
        ..Default::default()
    };

    // One timestamp per fetch run, threaded into every
    // `bulk_upsert_in_tx` call below. Goes into the bookkeeping
    // sidecars' `fetched_at_utc` / `last_attempt_at_utc` columns; the value
    // means "the sync that wrote this row," not "the millisecond the
    // UPSERT query ran" — so consistency across tables matters more
    // than sub-second freshness.
    let now = IsoOffsetTimestamp::now_local();

    // Persist the account row.
    let account_payload = session
        .accounts
        .iter()
        .find(|(k, _)| k == account_id)
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| json!({}));
    upsert_account(db, &now, account_id, &account_payload).await?;

    // ── mailboxes ───────────────────────────────────────────────────
    bar.doing("mailboxes");
    if let Err(e) = sync_mailboxes(db, &now, session, account_id, opts, &mut summary).await {
        // The mailboxes an earlier listing stored still file the mail;
        // with none stored there is nothing to file it under.
        let stored = !db.mailbox_names(account_id).await?.is_empty();
        if !api::is_upstream(&e) || !(stored || opts.control.stop.requested()) {
            return Err(e);
        }
        found.listing(M_MAILBOX_GET, format!("{e:#}"));
    }
    bar.did(1);

    // Parse the mailbox tree once: both the extraction filter and the
    // widened-label backfill resolve label paths against it.
    // (`Mailbox/get` always re-lists, even on an incremental run.)
    let mailbox_nodes: Vec<crate::mailbox_labels::MailboxNode> =
        if opts.only_mailbox_labels.is_empty()
            && *label_change == datalib_etl::scope_config::FilterChange::Unchanged
        {
            Vec::new()
        } else {
            db.load_mailboxes()
                .await?
                .iter()
                .filter_map(crate::mailbox_labels::MailboxNode::from_payload)
                .collect()
        };

    // Resolve the configured label paths to mailbox ids now that the
    // full tree is in the db (`Mailbox/get` always re-lists, even on an
    // incremental run). Empty config = no filter (sync every mailbox).
    // An all-unmatched filter resolves to an empty set, which means
    // "match nothing" — loud-warned below so a typo'd path doesn't
    // silently drop the whole account.
    let mailbox_filter: Option<HashSet<String>> = if opts.only_mailbox_labels.is_empty() {
        None
    } else {
        let resolved = crate::mailbox_labels::resolve(&mailbox_nodes, &opts.only_mailbox_labels);
        for spec in &resolved.unmatched {
            summary
                .problems
                .push(datalib_etl::download_problems::DownloadProblem::not_found(
                    K_ONLY_EXTRACT_LABELS,
                    spec,
                    "no mailbox with this label path; check spelling / parent path",
                ));
        }
        info!(
            event = "jmap_label_filter",
            requested = opts.only_mailbox_labels.len(),
            resolved_mailboxes = resolved.ids.len(),
            "resolved the label filter to mailboxes"
        );
        Some(resolved.ids)
    };
    // Every run, so a filter corrected or removed takes its rows with it.
    found.config(summary.problems.clone());

    // Mailboxes newly admitted by a widened `only_extract_labels`.
    // `Email/changes` only reports what changed since the cursor, so
    // existing mail in these mailboxes is invisible to it and needs its
    // own bounded enumeration.
    // `None` = no backfill. `Some(None)` = the filter was removed, so
    // enumerate the whole account. `Some(Some(ids))` = enumerate just
    // the newly-admitted mailboxes.
    #[allow(clippy::option_option)]
    let backfill: Option<Option<HashSet<String>>> = match label_change {
        datalib_etl::scope_config::FilterChange::Unchanged => None,
        datalib_etl::scope_config::FilterChange::WidenedToAll => {
            info!(
                event = "jmap_label_filter_widened",
                added = "<filter removed>",
                "enumerating the whole account",
            );
            Some(None)
        }
        datalib_etl::scope_config::FilterChange::Added(added) => {
            let resolved = crate::mailbox_labels::resolve(&mailbox_nodes, added);
            if resolved.ids.is_empty() {
                None
            } else {
                info!(
                    event = "jmap_label_filter_widened",
                    added = %added.join(", "),
                    resolved_mailboxes = resolved.ids.len(),
                    "enumerating newly-in-scope mailboxes",
                );
                Some(Some(resolved.ids))
            }
        }
    };

    // ── emails (+ collect threadIds) ────────────────────────────────
    bar.doing("emails");
    let mut touched_threads: HashSet<String> = HashSet::new();
    if let Err(e) = sync_emails(
        db,
        opts.sealer.as_ref(),
        &now,
        session,
        account_id,
        opts,
        mailbox_filter.as_ref(),
        backfill.as_ref(),
        bar,
        &mut summary,
        &mut touched_threads,
    )
    .await
    {
        // The walk stopped part-way: what it stored stays, nothing is
        // pruned, and no state is saved, so the next run walks again.
        let stored = db.holds_emails(account_id).await?;
        if !api::is_upstream(&e) || !(stored || opts.control.stop.requested()) {
            return Err(e);
        }
        found.listing(M_EMAIL_QUERY, format!("{e:#}"));
    }
    bar.did(1);

    // ── threads ─────────────────────────────────────────────────────
    bar.doing("threads");
    // A thread an earlier run could not get has emails and no row; its
    // emails will not change to name it again.
    touched_threads.extend(db.threads_without_a_row(account_id).await?);
    if let Some(p) = sync_threads(
        db,
        &now,
        session,
        account_id,
        opts,
        &touched_threads,
        &mut summary,
    )
    .await?
    {
        found.push(p);
    }
    bar.did(1);

    // ── blobs ───────────────────────────────────────────────────────
    bar.doing("blobs");
    let blobs = sync_blobs(db, session, account_id, opts, bar, &mut summary).await;
    bar.did(1);
    blobs?;

    info!(
        event = "jmap_download_complete",
        mailboxes_upserted = summary.mailboxes_upserted,
        emails_upserted = summary.emails_upserted,
        emails_destroyed = summary.emails_destroyed,
        threads_upserted = summary.threads_upserted,
        blobs_downloaded = summary.blobs_downloaded,
        blobs_oversize = summary.blobs_oversize,
        blobs_errored = summary.blobs_errored,
        "the JMAP download is done"
    );
    Ok(summary)
}

// Mailboxes

async fn sync_mailboxes(
    db: &RawDb,
    now: &IsoOffsetTimestamp,
    session: &Session,
    account_id: &str,
    opts: &FetchOptions,
    summary: &mut FetchSummary,
) -> Result<()> {
    let stored = if opts.full_resync {
        None
    } else {
        db.load_state(account_id, "Mailbox").await?
    };

    if let Some(since) = stored {
        match incremental_mailboxes(db, now, session, account_id, &since, summary).await {
            Ok(()) => return Ok(()),
            Err(e) => warn!(
                event = "jmap_mailbox_changes_fallback",
                error = %e,
                "falling back to full Mailbox/get",
            ),
        }
    }

    // Full re-list.
    let resp = call(
        session,
        "Mailbox/get",
        json!({"accountId": account_id, "ids": null}),
    )
    .await?;
    let list = jmap_list(&resp);
    summary.mailboxes_upserted += list.len();
    upsert_mailboxes(db, now, account_id, &list).await?;
    // A full list names every mailbox the account has, so a row it did
    // not name is one upstream destroyed while we were not replaying
    // `Mailbox/changes`.
    let listed: HashSet<String> = list
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    // An empty list is a server that answered oddly, not an account with
    // no Inbox; it must not strip every label off every email.
    let held = db.mailbox_names(account_id).await?;
    let gone = if listed.is_empty() {
        Vec::new()
    } else {
        unlisted_mailboxes(held.keys(), &listed)
    };
    summary.mailboxes_destroyed += gone.len();
    refile_mailboxes(db, now, &gone).await?;
    if let Some(state) = resp.get("state").and_then(|v| v.as_str()) {
        db.save_state(account_id, "Mailbox", state).await?;
    }
    Ok(())
}

async fn incremental_mailboxes(
    db: &RawDb,
    now: &IsoOffsetTimestamp,
    session: &Session,
    account_id: &str,
    since: &str,
    summary: &mut FetchSummary,
) -> Result<()> {
    let mut cursor = since.to_string();
    loop {
        let changes = call(
            session,
            "Mailbox/changes",
            json!({"accountId": account_id, "sinceState": cursor, "maxChanges": CHANGES_MAX}),
        )
        .await?;
        let created = string_array(&changes, "created");
        let updated = string_array(&changes, "updated");
        let destroyed = string_array(&changes, "destroyed");

        let to_fetch: Vec<String> = created.into_iter().chain(updated).collect();
        if !to_fetch.is_empty() {
            let resp = call(
                session,
                "Mailbox/get",
                json!({"accountId": account_id, "ids": to_fetch}),
            )
            .await?;
            let list = jmap_list(&resp);
            summary.mailboxes_upserted += list.len();
            upsert_mailboxes(db, now, account_id, &list).await?;
        }

        if !destroyed.is_empty() {
            summary.mailboxes_destroyed += destroyed.len();
            let moves: Vec<(String, Option<String>)> =
                destroyed.into_iter().map(|id| (id, None)).collect();
            refile_mailboxes(db, now, &moves).await?;
        }

        let new_state = changes
            .get("newState")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Mailbox/changes response missing newState"))?
            .to_string();
        db.save_state(account_id, "Mailbox", &new_state).await?;

        let has_more = changes
            .get("hasMoreChanges")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !has_more {
            return Ok(());
        }
        cursor = new_state;
    }
}

// Emails

#[allow(clippy::too_many_arguments)]
async fn sync_emails(
    db: &RawDb,
    sealer: Option<&datalib_etl::raw_store::Sealer>,
    now: &IsoOffsetTimestamp,
    session: &Session,
    account_id: &str,
    opts: &FetchOptions,
    mailbox_filter: Option<&HashSet<String>>,
    #[allow(clippy::option_option)] backfill: Option<&Option<HashSet<String>>>,
    bar: &RunBar,
    summary: &mut FetchSummary,
    touched_threads: &mut HashSet<String>,
) -> Result<()> {
    let stored = if opts.full_resync {
        None
    } else {
        db.load_state(account_id, "Email").await?
    };

    if let Some(since) = stored {
        match incremental_emails(
            db,
            sealer,
            now,
            session,
            account_id,
            &since,
            mailbox_filter,
            bar,
            summary,
            touched_threads,
        )
        .await
        {
            Ok(()) => {
                // The incremental pass covered everything that changed.
                // A widened label filter additionally needs the mail
                // that did *not* change in the newly-admitted mailboxes.
                if let Some(scope) = backfill {
                    let seen = full_enumerate_emails(
                        db,
                        sealer,
                        now,
                        session,
                        account_id,
                        scope.as_ref(),
                        bar,
                        summary,
                        touched_threads,
                    )
                    .await?;
                    prune_to_enumeration(db, account_id, scope.as_ref(), seen, summary).await?;
                }
                return Ok(());
            }
            Err(e) => warn!(
                event = "jmap_email_changes_fallback",
                error = %e,
                "falling back to full Email/query enumeration",
            ),
        }
    }

    let seen = full_enumerate_emails(
        db,
        sealer,
        now,
        session,
        account_id,
        mailbox_filter,
        bar,
        summary,
        touched_threads,
    )
    .await?;
    prune_to_enumeration(db, account_id, mailbox_filter, seen, summary).await
}

/// Delete the emails a finished, unfiltered `Email/query` walk did not
/// list: they were destroyed while no `Email/changes` cursor was
/// replaying. A walk narrowed to some mailboxes, or one that stopped
/// part-way, says nothing about the mail it did not reach.
async fn prune_to_enumeration(
    db: &RawDb,
    account_id: &str,
    mailbox_filter: Option<&HashSet<String>>,
    seen: Option<BTreeSet<String>>,
    summary: &mut FetchSummary,
) -> Result<()> {
    match (mailbox_filter, seen) {
        (None, Some(seen)) => {
            summary.emails_destroyed += db.prune_emails_to(account_id, &seen).await?;
        }
        (filter, seen) => info!(
            event = "jmap_prune_skipped",
            label_filtered = filter.is_some(),
            finished = seen.is_some(),
            "the enumeration did not list the whole account; not treating unlisted emails as destroyed",
        ),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn incremental_emails(
    db: &RawDb,
    sealer: Option<&datalib_etl::raw_store::Sealer>,
    now: &IsoOffsetTimestamp,
    session: &Session,
    account_id: &str,
    since: &str,
    mailbox_filter: Option<&HashSet<String>>,
    bar: &RunBar,
    summary: &mut FetchSummary,
    touched_threads: &mut HashSet<String>,
) -> Result<()> {
    bar.doing("replaying changes");
    let mut cursor = since.to_string();
    loop {
        let changes = call(
            session,
            "Email/changes",
            json!({"accountId": account_id, "sinceState": cursor, "maxChanges": CHANGES_MAX}),
        )
        .await?;
        let created = string_array(&changes, "created");
        let updated = string_array(&changes, "updated");
        let destroyed = string_array(&changes, "destroyed");

        // Detail-fetch created + updated in batches.
        let to_fetch: Vec<String> = created.into_iter().chain(updated).collect();
        // One `Email/changes` page at a time: the protocol says whether
        // more are coming, never how many ids they hold in total.
        bar.expect(to_fetch.len() as u64);
        for batch in to_fetch.chunks(EMAIL_GET_BATCH) {
            // Asked to stop: the batch that just landed sealed, and the
            // state for this page is saved only below, so the next run
            // takes the page again from where this one started.
            if sealer.is_some_and(|s| s.stopping()) {
                info!(
                    event = "jmap_interrupted",
                    phase = "Email/changes",
                    "told to stop; leaving the rest of this phase for the next run"
                );
                return Ok(());
            }
            let resp = email_get(session, account_id, batch).await?;
            let list = jmap_list(&resp);
            ingest_email_list(
                db,
                now,
                account_id,
                list,
                mailbox_filter,
                summary,
                touched_threads,
            )
            .await?;
            bar.did(batch.len() as u64);
            // A batch of `Email/get` results has landed in full -- rows and
            // the blobs they name together -- so the store is consistent
            // here. Deletions are applied separately, after the walk.
            if let Some(sealer) = sealer {
                sealer.wrote(batch.len() as u64).await;
            }
        }

        if !destroyed.is_empty() {
            summary.emails_destroyed += destroyed.len();
            db.delete_emails(&destroyed).await?;
        }

        let new_state = changes
            .get("newState")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Email/changes response missing newState"))?
            .to_string();
        db.save_state(account_id, "Email", &new_state).await?;

        let has_more = changes
            .get("hasMoreChanges")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !has_more {
            return Ok(());
        }
        cursor = new_state;
    }
}

#[allow(clippy::too_many_arguments)]
async fn full_enumerate_emails(
    db: &RawDb,
    sealer: Option<&datalib_etl::raw_store::Sealer>,
    now: &IsoOffsetTimestamp,
    session: &Session,
    account_id: &str,
    mailbox_filter: Option<&HashSet<String>>,
    bar: &RunBar,
    summary: &mut FetchSummary,
    touched_threads: &mut HashSet<String>,
) -> Result<Option<BTreeSet<String>>> {
    bar.doing("enumerating");
    // `total` below is this walk's own size, so the run total is it
    // plus whatever a preceding incremental pass already announced.
    let before = bar.announced();
    // Decide filter: if a label filter resolved to mailbox ids, push it
    // server-side as an OR over inMailbox.
    let filter = match mailbox_filter {
        None => Value::Null,
        Some(set) if set.is_empty() => {
            // Label filter resolved to zero mailboxes (all paths
            // unmatched). Nothing can match — skip enumeration rather
            // than send a degenerate empty-OR filter.
            return Ok(None);
        }
        Some(set) if set.len() == 1 => {
            json!({"inMailbox": set.iter().next().unwrap()})
        }
        Some(set) => {
            let conds: Vec<Value> = set.iter().map(|m| json!({"inMailbox": m})).collect();
            json!({"operator": "OR", "conditions": conds})
        }
    };

    let mut position: i64 = 0;
    // The account's live state token, taken from the first `Email/get` and
    // stored only once the enumeration has walked everything: stored
    // earlier, a walk that ends part-way — an error, a stop — leaves a
    // token behind, and the next run goes incremental from it and never
    // enumerates the rest.
    let mut live_state: Option<String> = None;
    let mut query_state: Option<String> = None;
    // Every id any page listed. A restart after a `queryState` shift
    // keeps what it had: those emails existed when listed, and a later
    // destroy reaches us through the next `Email/changes`.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    loop {
        let mut args = json!({
            "accountId": account_id,
            "sort": [{"property": "receivedAt", "isAscending": false}],
            "limit": EMAIL_QUERY_PAGE,
            "position": position,
            "calculateTotal": true,
        });
        if !filter.is_null() {
            args["filter"] = filter.clone();
        }
        let resp = call(session, "Email/query", args).await?;

        let page_state = resp
            .get("queryState")
            .and_then(|v| v.as_str())
            .map(String::from);
        if let (Some(stored), Some(current)) = (&query_state, &page_state) {
            if stored != current {
                // Result set shifted underneath us; restart from page 0.
                warn!(
                    event = "jmap_email_query_state_shift",
                    "queryState changed mid-pagination; restarting"
                );
                position = 0;
                query_state = Some(current.clone());
                continue;
            }
        } else if query_state.is_none() {
            query_state = page_state.clone();
        }

        let ids: Vec<String> = resp
            .get("ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            break;
        }
        // `calculateTotal` means every page carries the size of the whole
        // result set, not of the page — so raise the total to it rather
        // than adding, or each page counts the same messages again.
        if let Some(total) = resp.get("total").and_then(|v| v.as_i64()) {
            bar.expect_at_least(before + total.max(0) as u64);
        }
        position += ids.len() as i64;
        seen.extend(ids.iter().cloned());

        for batch in ids.chunks(EMAIL_GET_BATCH) {
            // Asked to stop: the batch that just landed sealed; with no
            // state token stored, the next run enumerates again.
            if sealer.is_some_and(|s| s.stopping()) {
                info!(
                    event = "jmap_interrupted",
                    phase = "Email/query",
                    "told to stop; leaving the rest of this phase for the next run"
                );
                return Ok(None);
            }
            let getresp = email_get(session, account_id, batch).await?;
            let list = jmap_list(&getresp);
            ingest_email_list(
                db,
                now,
                account_id,
                list,
                mailbox_filter,
                summary,
                touched_threads,
            )
            .await?;
            bar.did(batch.len() as u64);
            // A batch has landed in full, so the store is consistent here.
            if let Some(sealer) = sealer {
                sealer.wrote(batch.len() as u64).await;
            }

            if live_state.is_none() {
                live_state = getresp
                    .get("state")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
        }

        if let Some(total) = resp.get("total").and_then(|v| v.as_i64()) {
            if position >= total {
                break;
            }
        }
    }
    if let Some(state) = live_state {
        db.save_state(account_id, "Email", &state).await?;
    }
    Ok(Some(seen))
}

async fn email_get(session: &Session, account_id: &str, ids: &[String]) -> Result<Value> {
    let props: Vec<Value> = EMAIL_GET_PROPERTIES
        .iter()
        .map(|s| Value::String((*s).to_string()))
        .collect();
    call(
        session,
        "Email/get",
        json!({
            "accountId": account_id,
            "ids": ids,
            "properties": props,
        }),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn ingest_email_list(
    db: &RawDb,
    now: &IsoOffsetTimestamp,
    account_id: &str,
    list: Vec<Value>,
    mailbox_filter: Option<&HashSet<String>>,
    summary: &mut FetchSummary,
    touched_threads: &mut HashSet<String>,
) -> Result<()> {
    let mut rows: Vec<EmailRow> = Vec::with_capacity(list.len());
    for envelope in list {
        let Some(row) = EmailRow::from_jmap_envelope(account_id, &envelope) else {
            continue;
        };
        if let Some(allow) = mailbox_filter {
            if !row.mailbox_ids().iter().any(|m| allow.contains(m)) {
                continue;
            }
        }
        touched_threads.insert(row.thread_id.clone());
        rows.push(row);
    }
    if rows.is_empty() {
        return Ok(());
    }
    summary.emails_upserted += rows.len();
    upsert_emails(db, now, &rows).await
}

// Threads

async fn sync_threads(
    db: &RawDb,
    now: &IsoOffsetTimestamp,
    session: &Session,
    account_id: &str,
    opts: &FetchOptions,
    touched: &HashSet<String>,
    summary: &mut FetchSummary,
) -> Result<Option<RunProblem>> {
    if touched.is_empty() || opts.control.stop.requested() {
        return Ok(None);
    }
    // Sorted: `touched` is a hash set, so the same threads would
    // otherwise go out as a different request — batched differently
    // each run, and unmatchable by a recorded playback fixture.
    let mut ids: Vec<String> = touched.iter().cloned().collect();
    ids.sort_unstable();
    // A batch that will not answer costs only its threads, and is the
    // run's one `phase:` row.
    let mut failed: Option<RunProblem> = None;
    for batch in ids.chunks(THREAD_GET_BATCH) {
        let resp = match call(
            session,
            "Thread/get",
            json!({"accountId": account_id, "ids": batch}),
        )
        .await
        {
            Ok(resp) => resp,
            Err(e) => {
                let terminal = api::is_terminal(&e);
                failed.get_or_insert_with(|| RunProblem::phase(M_THREAD_GET, format!("{e:#}")));
                if terminal {
                    break;
                }
                continue;
            }
        };
        let list = jmap_list(&resp);
        // Build the whole batch's worth of rows up front, then bulk-
        // upsert in one tx. The per-thread `upsert_thread` call this
        // replaced opened a fresh transaction per row, which made a
        // 200-thread sync 200 sequential commits.
        let mut rows: Vec<ThreadRow> = Vec::with_capacity(list.len());
        for thread in &list {
            let Some(id) = thread.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            rows.push(ThreadRow::from_jmap_payload(id, account_id, thread)?);
        }
        summary.threads_upserted += rows.len();
        upsert_threads(db, now, &rows).await?;
        if let Some(state) = resp.get("state").and_then(|v| v.as_str()) {
            db.save_state(account_id, "Thread", state).await?;
        }
    }
    Ok(failed)
}

// Blobs

/// Download the `.eml` for every email that doesn't have its
/// blake3 set yet. After the eml-as-canonical port we no longer
/// fetch attachments separately — the `.eml` is the complete
/// backup, render mail-parses parts on demand.
async fn sync_blobs(
    db: &RawDb,
    session: &Session,
    account_id: &str,
    opts: &FetchOptions,
    bar: &RunBar,
    summary: &mut FetchSummary,
) -> Result<()> {
    if opts.control.stop.requested() {
        return Ok(());
    }
    let have_bytes = db.loaded_blob_ids().await?;

    // Build the to-fetch worklist: per-email `.eml` source only.
    // BTreeMap by blob_id dedupes (multiple emails can share the
    // same blob_id in theory) and gives stable dispatch order.
    let mut wanted: BTreeMap<String, EmlJob> = BTreeMap::new();
    for em in db.load_emails().await? {
        if em.blob_id.is_empty() || have_bytes.contains_key(&em.blob_id) {
            continue;
        }
        wanted.insert(
            em.blob_id.clone(),
            EmlJob {
                owning_id: em.id.clone(),
                advertised_size: em.size,
            },
        );
    }

    summary.blobs_skipped = have_bytes.len();
    if wanted.is_empty() {
        debug!(
            event = "jmap_blobs_up_to_date",
            "every blob is already stored"
        );
        return Ok(());
    }
    info!(
        event = "jmap_blobs_pending",
        count = wanted.len(),
        "blobs still to fetch"
    );

    // Accumulate downloads + their `email_blobs` edges in the shared
    // CAS-edge accumulator: each fetched `.eml` carries its bytes (for
    // the end-of-pass `put_many`) and yields an edge whose `blake3` the
    // accumulator resolves off those bytes. Failures get an edge with
    // NULL `blake3` and an error stamp on `email_blobs_bookkeeping`.
    let mut acc = CasEdgeAccumulator::new();

    // Split the worklist: oversize `.eml`s are recorded as failures up
    // front (no GET), the rest become owned download jobs. The oversize
    // check is cheap and serial; only the network GETs fan out.
    let mut jobs: Vec<(String, String)> = Vec::new();
    for (blob_id, job) in wanted {
        if let Some(limit) = opts.blob_size_limit_bytes {
            if let Some(sz) = job.advertised_size {
                if sz as u64 > limit {
                    summary.blobs_oversize += 1;
                    acc.add_skipped(
                        &job.owning_id,
                        &blob_id,
                        datalib_problems::Reason::OverSizeLimit,
                        format!("the .eml is {sz} bytes, over blob_size_limit_bytes ({limit})"),
                    );
                    continue;
                }
            }
        }
        jobs.push((blob_id, job.owning_id));
    }

    // Bounded fan-out. JMAP exposes no bulk-blob method, so each `.eml`
    // is its own GET; the win on a large backfill is having up to
    // `concurrency` of them in flight at once. The downloads run on the
    // runtime while this single task drains completions and feeds the
    // accumulator — so `acc` mutation stays serial and lock-free even
    // though the network I/O is concurrent.
    let concurrency = opts
        .blob_download_concurrency
        .unwrap_or(DEFAULT_BLOB_CONCURRENCY)
        .max(1);
    info!(
        event = "jmap_blobs_fetch",
        pending = jobs.len(),
        concurrency,
        "fetching the pending blobs"
    );

    // The worklist is fully materialized, so this phase knows its exact
    // size and can add it to the run's total instead of ticking once.
    bar.expect(jobs.len() as u64);
    bar.doing("fetching .eml");

    // Build a download task from an owned (blob_id, owning_id). The
    // `downloadUrl` is substituted here (borrowing `session`) so the
    // spawned future owns only `String`s and is `Send + 'static` — which
    // is also why the latchkey settings are cloned per task rather than
    // borrowed from `opts`.
    let spawn_one = |set: &mut JoinSet<EmlFetchOutcome>, blob_id: String, owning_id: String| {
        let url = session.download_url_for(account_id, &blob_id, "message.eml", "message/rfc822");
        let latchkey = opts.latchkey.clone();
        set.spawn(async move {
            let result = api::download_bytes(&url, BLOB_TIMEOUT, &latchkey).await;
            (blob_id, owning_id, result)
        });
    };

    let mut pending = jobs.into_iter();
    let mut set: JoinSet<EmlFetchOutcome> = JoinSet::new();
    for _ in 0..concurrency {
        match pending.next() {
            Some((blob_id, owning_id)) => spawn_one(&mut set, blob_id, owning_id),
            None => break,
        }
    }

    // What ends the phase early: a refused credential, a retry loop
    // that gave up, or too many failures in a row. Walking on would
    // fail every remaining `.eml` the same way, one row each.
    let mut tripped: Option<anyhow::Error> = None;
    let mut failures_in_a_row = 0usize;
    while let Some(joined) = set.join_next().await {
        let (blob_id, owning_id, result) = match joined {
            Ok(outcome) => outcome,
            Err(e) if e.is_cancelled() => continue,
            Err(e) => return Err(anyhow!(e).context("blob download task panicked")),
        };
        let ending = tripped.is_some() || opts.control.stop.requested();
        match result {
            Ok((bytes, content_type)) => {
                failures_in_a_row = 0;
                acc.add_fetched(
                    &owning_id,
                    &blob_id,
                    bytes,
                    Some(content_type.unwrap_or_else(|| "message/rfc822".to_string())),
                    None,
                );
                summary.blobs_downloaded += 1;
            }
            // The run is ending, and a request the stop refused is no
            // failure of this blob's.
            Err(_) if ending => {}
            Err(e) if api::is_terminal(&e) => {
                summary.blobs_errored += 1;
                tripped = Some(e.context(format!("downloading .eml {blob_id}")));
                set.abort_all();
            }
            Err(e) => {
                summary.blobs_errored += 1;
                failures_in_a_row += 1;
                acc.add_failed(&owning_id, &blob_id, format!("{e:#}"));
                if failures_in_a_row >= BLOB_FAILURE_BUDGET {
                    tripped = Some(e.context(format!(
                        "{failures_in_a_row} .eml downloads failed in a row; the last was {blob_id}"
                    )));
                    set.abort_all();
                }
            }
        }
        bar.did(1);
        // Backfill the freed slot so `concurrency` GETs stay in flight.
        if tripped.is_none() && !opts.control.stop.requested() {
            if let Some((blob_id, owning_id)) = pending.next() {
                spawn_one(&mut set, blob_id, owning_id);
            }
        }
    }

    // What did land is kept either way.
    acc.flush(db.pool(), db.cas(), |email_id, blob_id, blake3| {
        EmlBlobRow {
            id: EmlBlobRow::pk_recipe(email_id, blob_id),
            email_id: email_id.to_string(),
            blob_id: blob_id.to_string(),
            blake3: blake3.map(str::to_string),
        }
    })
    .await?;
    match tripped {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct EmlJob {
    owning_id: String,
    advertised_size: Option<i64>,
}

/// One blob download task's result: `(blob_id, owning_id, bytes-or-err)`.
/// The ids ride along so the draining loop can route the outcome to the
/// accumulator without tracking which task was which.
type EmlFetchOutcome = (String, String, Result<(Vec<u8>, Option<String>)>);

// Helpers

/// A JMAP `*/get` response's `list`, empty when it has none.
fn jmap_list(resp: &Value) -> Vec<Value> {
    resp.get("list")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn string_array(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn tmp_db() -> (tempfile::TempDir, RawDb) {
        let d = tempfile::tempdir().unwrap();
        let db = RawDb::open(&d.path().join("j.doltlite_db")).await.unwrap();
        (d, db)
    }

    fn now() -> IsoOffsetTimestamp {
        IsoOffsetTimestamp::now_local()
    }

    fn email(id: &str, mailboxes: &[&str]) -> EmailRow {
        let ids: serde_json::Map<String, Value> = mailboxes
            .iter()
            .map(|m| (m.to_string(), Value::Bool(true)))
            .collect();
        EmailRow::from_jmap_envelope(
            "A",
            &json!({"id": id, "blobId": "B", "threadId": "T", "mailboxIds": ids}),
        )
        .unwrap()
    }

    async fn payload_mailboxes(db: &RawDb, id: &str) -> Vec<String> {
        let p: String = sqlx::query_scalar("SELECT json(payload) FROM emails WHERE id = ?")
            .bind(id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&p).unwrap();
        let mut ids: Vec<String> = v["mailboxIds"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        ids.sort();
        ids
    }

    async fn joined_mailboxes(db: &RawDb, id: &str) -> Vec<String> {
        let mut ids = db
            .load_email_joins()
            .await
            .unwrap()
            .mailboxes
            .remove(id)
            .unwrap_or_default();
        ids.sort();
        ids
    }

    /// The counts go to the sidecar, so a new message in the Inbox leaves
    /// the `mailboxes` row exactly as it was, and `dolt_diff_mailboxes`
    /// says nothing about it.
    #[tokio::test]
    async fn a_mailbox_count_changing_is_not_a_change_to_the_mailbox() {
        let (_d, db) = tmp_db().await;
        let inbox = |n: i64| json!({"id": "M1", "name": "Inbox", "role": "inbox", "totalEmails": n, "unreadEmails": n});
        upsert_mailboxes(&db, &now(), "A", &[inbox(1)])
            .await
            .unwrap();
        dr::commit_run(db.pool(), "one").await.unwrap();
        upsert_mailboxes(&db, &now(), "A", &[inbox(2)])
            .await
            .unwrap();
        dr::commit_run(db.pool(), "two").await.unwrap();

        let content: String = sqlx::query_scalar("SELECT json(payload) FROM mailboxes")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert!(!content.contains("totalEmails"), "{content}");
        let counts: String = sqlx::query_scalar(
            "SELECT json(volatile_payload) FROM mailboxes_bookkeeping WHERE id = 'M1'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&counts).unwrap(),
            json!({"totalEmails": 2, "unreadEmails": 2})
        );
        let changed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM dolt_diff_mailboxes WHERE from_ref = 'HEAD~1' AND to_ref = 'HEAD'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(changed, 0);
        db.close().await;
    }

    /// A mailbox that went away upstream comes off every email that was
    /// in it — payload and join rows together — and its row goes.
    #[tokio::test]
    async fn refiling_to_nothing_takes_the_label_off_every_email() {
        let (_d, db) = tmp_db().await;
        upsert_mailboxes(
            &db,
            &now(),
            "A",
            &[
                json!({"id": "M1", "name": "Inbox"}),
                json!({"id": "M2", "name": "Work"}),
            ],
        )
        .await
        .unwrap();
        upsert_emails(
            &db,
            &now(),
            &[email("E1", &["M1", "M2"]), email("E2", &["M2"])],
        )
        .await
        .unwrap();

        let moved = refile_mailboxes(&db, &now(), &[("M2".into(), None)])
            .await
            .unwrap();

        assert_eq!(moved, 2);
        assert_eq!(payload_mailboxes(&db, "E1").await, vec!["M1"]);
        assert_eq!(joined_mailboxes(&db, "E1").await, vec!["M1"]);
        assert!(payload_mailboxes(&db, "E2").await.is_empty());
        assert!(joined_mailboxes(&db, "E2").await.is_empty());
        let names = db.mailbox_names("A").await.unwrap();
        assert_eq!(names.keys().collect::<Vec<_>>(), vec!["M1"]);
        db.close().await;
    }

    /// A row re-keyed onto a new id carries its emails with it.
    #[tokio::test]
    async fn refiling_onto_a_new_id_moves_every_email() {
        let (_d, db) = tmp_db().await;
        upsert_emails(&db, &now(), &[email("E1", &["old", "M1"])])
            .await
            .unwrap();

        refile_mailboxes(&db, &now(), &[("old".into(), Some("new".into()))])
            .await
            .unwrap();

        assert_eq!(payload_mailboxes(&db, "E1").await, vec!["M1", "new"]);
        assert_eq!(joined_mailboxes(&db, "E1").await, vec!["M1", "new"]);
        db.close().await;
    }

    #[test]
    fn a_full_listing_leaves_behind_the_rows_it_did_not_name() {
        let held = ["M1".to_string(), "M2".to_string(), "M3".to_string()];
        let listed: HashSet<String> = ["M1".to_string(), "M3".to_string()].into();
        assert_eq!(
            unlisted_mailboxes(held.iter(), &listed),
            vec![("M2".to_string(), None)]
        );
    }
}
