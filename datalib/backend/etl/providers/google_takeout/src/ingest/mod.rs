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

use datalib_etl::download_problems::{self, RunProblem, RunProblemKind};
use datalib_etl::fingerprint_cache::FingerprintCache;
use datalib_etl::fsscan;
use futures::FutureExt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use anyhow::Result;
use datalib_etl::control::DownloadControl;
use datalib_etl::progress::Progress;
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
    /// Feeds that failed as a whole, each a `phase:<feed>` problem row.
    pub feeds_failed: usize,
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
        if let Some(n) = feed(
            "maps_reviews",
            maps_reviews::ingest(&db, scan, progress),
            &mut problems,
        )
        .await
        {
            summary.maps_reviews = n.written;
            summary.removed += n.removed;
        }
    }
    if opts.sync.maps_saved_places {
        if let Some(n) = feed(
            "maps_saved_places",
            maps_saved_places::ingest(&db, scan, progress),
            &mut problems,
        )
        .await
        {
            summary.maps_saved_places = n.written;
            summary.removed += n.removed;
        }
    }
    if opts.sync.maps_photos {
        if let Some(s) = feed(
            "maps_photos",
            maps_photos::ingest(&db, scan, progress),
            &mut problems,
        )
        .await
        {
            summary.maps_photos = s.rows;
            summary.blobs_stored += s.blobs;
            summary.removed += s.removed;
            summary.files_removed += s.files_removed;
        }
    }
    if opts.sync.youtube_watch_history {
        if let Some(n) = feed(
            "youtube_watch_history",
            youtube_watch_history::ingest(&db, scan, progress),
            &mut problems,
        )
        .await
        {
            summary.youtube_watch_history = n.written;
            summary.removed += n.removed;
        }
    }
    if opts.sync.youtube_subscriptions {
        if let Some(n) = feed(
            "youtube_subscriptions",
            youtube_subscriptions::ingest(&db, scan, progress),
            &mut problems,
        )
        .await
        {
            summary.youtube_subscriptions = n.written;
            summary.removed += n.removed;
        }
    }
    if opts.sync.google_chat {
        if let Some(s) = feed(
            "google_chat",
            google_chat::ingest(&db, scan, progress),
            &mut problems,
        )
        .await
        {
            summary.chat_groups += s.groups;
            summary.chat_users += s.users;
            summary.chat_messages += s.messages;
            summary.chat_attachments += s.attachments;
            summary.blobs_stored += s.blobs_stored;
            summary.removed += s.removed;
            summary.files_removed += s.files_removed;
        }
    }
    if opts.sync.gemini_apps {
        if let Some(s) = feed(
            "gemini_apps",
            gemini_apps::ingest(&db, scan, progress),
            &mut problems,
        )
        .await
        {
            summary.gemini_activity += s.activity;
            summary.gemini_attachments += s.attachments;
            summary.blobs_stored += s.blobs_stored;
            summary.removed += s.removed;
        }
    }
    if opts.sync.google_voice {
        let voice = google_voice::ingest(&db, scan, opts.sync.google_voice_include_spam, progress);
        if let Some(s) = feed("google_voice", voice, &mut problems).await {
            summary.voice_messages += s.messages;
            summary.voice_bills += s.bills;
            summary.voice_greetings += s.greetings;
            summary.voice_attachments += s.attachments;
            summary.blobs_stored += s.blobs_stored;
            summary.removed += s.removed;
            summary.files_removed += s.files_removed;
            problems.extend(s.held_back);
        }
    }
    summary.feeds_failed = problems
        .iter()
        .filter(|p| p.kind == RunProblemKind::Phase)
        .count();
    if !opts.control.stop.requested() {
        download_problems::report_run(db.pool(), &problems).await;
    }

    Ok(summary)
}

/// A file that is not in the layout its reader knows: `what` says how.
/// Nothing is stored or deleted on its word, and the feed fails where a
/// person sees it, so a newer export Google reshaped cannot empty a table.
pub(crate) fn unknown_layout(file: &str, what: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{file} {what}, so it is not in a layout this reader knows; nothing was stored or deleted"
    )
}

/// A file that lists entries none of which could be read is a layout
/// that moved, not a product emptied upstream.
pub(crate) fn require_some_read(file: &str, listed: usize, read: usize) -> Result<()> {
    if listed > 0 && read == 0 {
        return Err(unknown_layout(
            file,
            &format!("lists {listed} entries and none could be read"),
        ));
    }
    Ok(())
}

/// Reports what a snapshot feed skipped, if it read its file this run:
/// `None` means the file was unchanged and last run's rows still hold.
pub(crate) async fn report_skipped_if_read(
    db: &RawDb,
    part: &str,
    skipped: Option<Vec<download_problems::SkippedRecord>>,
) {
    if let Some(skipped) = skipped {
        download_problems::report_skipped(db.pool(), part, &skipped).await;
    }
}

/// Runs one feed so that its failure, an error or a panic, costs only
/// that feed: it becomes a `phase:<feed>` problem and the others run.
async fn feed<T>(
    name: &str,
    run: impl Future<Output = Result<T>>,
    problems: &mut Vec<RunProblem>,
) -> Option<T> {
    // Unwind safety: a feed that panics mid-write drops its transaction,
    // and a dropped sqlx transaction rolls back, so the store is left as
    // the last feed that finished left it.
    let detail = match AssertUnwindSafe(run).catch_unwind().await {
        Ok(Ok(v)) => return Some(v),
        Ok(Err(e)) => format!("{e:#}"),
        Err(panic) => format!("panicked: {}", panic_message(&*panic)),
    };
    problems.push(RunProblem::phase(name, detail));
    None
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("a panic with no message")
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    /// A panic inside one feed once ended the whole Takeout step.
    #[test]
    fn a_feed_that_panics_costs_only_itself() {
        let mut problems = Vec::new();
        let got: Option<()> = block_on(feed(
            "youtube_watch_history",
            async { panic!("sliced through a character") },
            &mut problems,
        ));
        assert!(got.is_none());
        assert_eq!(
            problems,
            [RunProblem::phase(
                "youtube_watch_history",
                "panicked: sliced through a character"
            )]
        );
        assert_eq!(
            block_on(feed("maps_reviews", async { Ok(2) }, &mut problems)),
            Some(2)
        );
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn a_feed_that_errs_names_the_error_chain() {
        let mut problems = Vec::new();
        let failing =
            async { Err::<(), _>(anyhow::anyhow!("not JSON").context("parse Saved Places.json")) };
        assert!(block_on(feed("maps_saved_places", failing, &mut problems)).is_none());
        assert_eq!(
            problems,
            [RunProblem::phase(
                "maps_saved_places",
                "parse Saved Places.json: not JSON"
            )]
        );
    }
}
