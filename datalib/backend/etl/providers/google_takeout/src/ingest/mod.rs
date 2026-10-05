//! Google Takeout extractor entry point.

pub mod attachment_path;
pub mod db;
pub mod gemini_apps;
pub mod google_chat;
pub mod google_voice;
pub mod maps_photos;
pub mod maps_reviews;
pub mod maps_saved_places;
pub mod mdl_html;
pub mod schema_raw;
pub mod time;
pub mod youtube_subscriptions;
pub mod youtube_watch_history;

pub use db::{db_path_for, RawDb};

use datalib_etl::download_problems::{self, RunProblem};
use datalib_etl::fingerprint_cache::FingerprintCache;
use datalib_etl::fsscan;
use std::path::PathBuf;

use anyhow::{Context, Result};
use datalib_etl::control::DownloadControl;
use datalib_etl::file_checkpoint;
use datalib_etl::progress::Progress;
use datalib_problems::{Outcome, Problem, Reason};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// One switch per Takeout feed. Defaults are all `false` — a fresh
/// user has to enable each feed consciously; INGEST.md says why
/// that matters and what each flag writes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncFlags {
    pub maps_reviews: bool,
    pub maps_saved_places: bool,
    pub maps_photos: bool,
    pub youtube_watch_history: bool,
    pub youtube_subscriptions: bool,
    pub google_chat: bool,
    pub gemini_apps: bool,
    /// Google Voice (`Voice/` subtree): texts, voicemails, calls, bills.
    pub google_voice: bool,
    /// When `google_voice` is on, also process the `Voice/Spam/` folder
    /// (download + render). Off by default — spam is bulky and only
    /// useful for parser hardening / practice corpora.
    pub google_voice_include_spam: bool,
}

impl SyncFlags {
    pub fn all() -> Self {
        Self {
            maps_reviews: true,
            maps_saved_places: true,
            maps_photos: true,
            youtube_watch_history: true,
            youtube_subscriptions: true,
            google_chat: true,
            gemini_apps: true,
            google_voice: true,
            google_voice_include_spam: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// The store this run writes into, opened and closed by the caller.
    /// A download never opens a store of its own: one writer per file
    /// (`datalib/backend/etl/README.md` § "One writer per file, by
    /// construction").
    pub db: RawDb,
    /// Root of the user's Takeout export (the directory that contains
    /// `Maps (your places)/`, `YouTube and YouTube Music/`,
    /// `Google Chat/`, etc.). May or may not be the literal
    /// `Takeout/` subdirectory of a Takeout zip.
    pub input_path: PathBuf,
    /// Host-wide fingerprint cache: the shared answer to "did this
    /// file change?". Every feed's resume cursor is a content hash
    /// read through it, so an unchanged export costs a `stat` per
    /// file and no re-reads.
    pub cache: FingerprintCache,
    /// Per-feed opt-in switches.
    pub sync: SyncFlags,
    pub progress: Progress,
    pub control: DownloadControl,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct FetchSummary {
    pub maps_reviews: usize,
    pub maps_saved_places: usize,
    pub maps_photos: usize,
    pub youtube_watch_history: usize,
    pub youtube_subscriptions: usize,
    pub chat_groups: usize,
    pub chat_users: usize,
    pub chat_messages: usize,
    pub chat_attachments: usize,
    pub gemini_activity: usize,
    pub gemini_attachments: usize,
    pub voice_messages: usize,
    pub voice_bills: usize,
    pub voice_greetings: usize,
    pub voice_attachments: usize,
    pub blobs_stored: usize,
    pub parse_errors: usize,
    /// Records deleted because the export no longer holds them.
    pub removed: usize,
    /// Export files that are gone since the last run, in the feeds read.
    pub files_removed: usize,
}

/// Whether the export holds `product_dir` at all. Only a product that is
/// here says what was deleted from it: one missing entirely was left out of
/// the Takeout request, so its records stay and its cursor is kept.
pub(crate) fn product_exported(scan: &fsscan::Scan, product_dir: &str) -> bool {
    scan.files
        .iter()
        .any(|f| fsscan::is_under(&f.rel, product_dir))
}

pub async fn fetch(opts: FetchOptions) -> Result<FetchSummary> {
    let db = opts.db.clone();

    let mut summary = FetchSummary::default();
    let root = &opts.input_path;
    let progress = &opts.progress;
    // One scan of the export, up front, so the walk and the hashing happen
    // once rather than nine times in nine slightly different shapes. The first
    // run hashes everything; later runs are `stat`-only, and a feed enabled
    // later costs nothing extra because its files are already in the cache.
    let scan = fsscan::scan(&opts.cache, root, &fsscan::ScanOptions::default(), |_| true).await?;
    for e in &scan.errors {
        warn!(event = "takeout_walk_error", path = %e.path.display(), error = %e.error, "an entry of the export could not be walked");
    }
    let scan = &scan;
    let mut problems = scan.walk_problems();

    if opts.sync.maps_reviews {
        match maps_reviews::ingest(&db, scan, progress).await {
            Ok(n) => {
                summary.maps_reviews = n.written;
                summary.removed += n.removed;
            }
            Err(e) => feed_failed(&mut problems, &mut summary, "maps_reviews", e),
        }
    }
    if opts.sync.maps_saved_places {
        match maps_saved_places::ingest(&db, scan, progress).await {
            Ok(n) => {
                summary.maps_saved_places = n.written;
                summary.removed += n.removed;
            }
            Err(e) => feed_failed(&mut problems, &mut summary, "maps_saved_places", e),
        }
    }
    if opts.sync.maps_photos {
        match maps_photos::ingest(&db, scan, progress).await {
            Ok(s) => {
                summary.maps_photos = s.rows;
                summary.blobs_stored += s.blobs;
                summary.removed += s.removed;
                summary.files_removed += s.files_removed;
                problems.extend(s.problems);
            }
            Err(e) => feed_failed(&mut problems, &mut summary, "maps_photos", e),
        }
    }
    if opts.sync.youtube_watch_history {
        match youtube_watch_history::ingest(&db, scan, progress).await {
            Ok(n) => {
                summary.youtube_watch_history = n.written;
                summary.removed += n.removed;
            }
            Err(e) => feed_failed(&mut problems, &mut summary, "youtube_watch_history", e),
        }
    }
    if opts.sync.youtube_subscriptions {
        match youtube_subscriptions::ingest(&db, scan, progress).await {
            Ok(n) => {
                summary.youtube_subscriptions = n.written;
                summary.removed += n.removed;
            }
            Err(e) => feed_failed(&mut problems, &mut summary, "youtube_subscriptions", e),
        }
    }
    if opts.sync.google_chat {
        match google_chat::ingest(&db, scan, progress).await {
            Ok(s) => {
                summary.chat_groups += s.groups;
                summary.chat_users += s.users;
                summary.chat_messages += s.messages;
                summary.chat_attachments += s.attachments;
                summary.blobs_stored += s.blobs_stored;
                summary.removed += s.removed;
                summary.files_removed += s.files_removed;
                problems.extend(s.problems);
            }
            Err(e) => feed_failed(&mut problems, &mut summary, "google_chat", e),
        }
    }
    if opts.sync.gemini_apps {
        match gemini_apps::ingest(&db, scan, progress).await {
            Ok(s) => {
                summary.gemini_activity += s.activity;
                summary.gemini_attachments += s.attachments;
                summary.blobs_stored += s.blobs_stored;
                summary.removed += s.removed;
            }
            Err(e) => feed_failed(&mut problems, &mut summary, "gemini_apps", e),
        }
    }
    if opts.sync.google_voice {
        match google_voice::ingest(&db, scan, opts.sync.google_voice_include_spam, progress).await {
            Ok(s) => {
                summary.voice_messages += s.messages;
                summary.voice_bills += s.bills;
                summary.voice_greetings += s.greetings;
                summary.voice_attachments += s.attachments;
                summary.blobs_stored += s.blobs_stored;
                summary.removed += s.removed;
                summary.files_removed += s.files_removed;
                problems.extend(s.problems);
            }
            Err(e) => feed_failed(&mut problems, &mut summary, "google_voice", e),
        }
    }
    if !opts.control.stop.requested() {
        download_problems::report_run(db.pool(), &problems).await;
    }

    Ok(summary)
}

/// What a snapshot file's read could not use, said on the file:
/// `ingest_snapshot` stamped it clean, and this restamps it with the one
/// row the file keeps until it is read again. `None` when the read found
/// nothing wrong, or did not happen because the file had not changed.
pub(crate) async fn record_unusable(
    db: &RawDb,
    scope: &str,
    file: Option<&fsscan::ScannedFile>,
    unusable: Option<(Reason, String)>,
) -> Result<()> {
    let (Some(file), Some((reason, detail))) = (file, unusable) else {
        return Ok(());
    };
    let mut tx = db
        .pool()
        .begin()
        .await
        .context("begin unusable-records tx")?;
    file_checkpoint::record_file_with_problem(
        &mut tx,
        scope,
        file,
        Some((Outcome::Dropped, Problem::record(reason, &detail))),
    )
    .await?;
    tx.commit().await.context("commit unusable-records tx")
}

/// The detail of a [`record_unusable`] row over records a read skipped.
pub(crate) fn skipped_records(skipped: &[String]) -> Option<(Reason, String)> {
    let first = skipped.first()?;
    Some((
        Reason::NoIdentity,
        format!(
            "{} records could not be used; first: {first}",
            skipped.len()
        ),
    ))
}

/// A feed that failed as a whole stamped nothing, so the next run tries
/// it again and its `phase:` row is the whole truth each run.
fn feed_failed(
    problems: &mut Vec<RunProblem>,
    summary: &mut FetchSummary,
    feed: &str,
    e: anyhow::Error,
) {
    problems.push(RunProblem::phase(feed, format!("{e:#}")));
    summary.parse_errors += 1;
}
