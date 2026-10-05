//! What a download could not do, collected while it runs and written
//! once, when it ends. The one way a download writes a `problems` row
//! that is not pinned to a stored record or a file's stamp.
//!
//! A row clears only when the run that would clear it tried the thing
//! again. So each kind of report says what the run covered, and
//! [`collecting`] sweeps exactly that: see `docs/dev/data_architecture_ingestion.md`
//! §"Error handling" for the rule and what each kind covers.

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::Poll;

use anyhow::Result;

use crate::download_problems::{
    self as rows, DownloadProblem, RecordProblem, RunProblem, RunProblemKind, SilentEntry,
    SkippedRecord, Sweep,
};
use crate::stop::StopFlag;

/// Run a download's `body` with a collector, and write what it collected
/// when the body returns, whichever way it returns. Writing never fails
/// the run: a store that cannot take the rows is said and passed over.
pub async fn collecting<T, Fut>(
    pool: &sqlx::SqlitePool,
    stop: &StopFlag,
    body: impl FnOnce(RunProblems) -> Fut,
) -> Result<T>
where
    Fut: Future<Output = Result<T>>,
{
    let problems = RunProblems::unwritten();
    let result = body(problems.clone()).await;
    let ended = if stop.requested() {
        Ended::Stopped
    } else if result.is_err() || problems.lock().cut_short {
        Ended::CutShort
    } else {
        Ended::Complete
    };
    if let Err(e) = problems.write(pool, ended).await {
        tracing::warn!(
            error = %format!("{e:#}"),
            "could not record what the run could not do; the Manage row will not show it"
        );
    }
    result
}

/// How far a run got, which decides what its listing and phase rows may
/// replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// Every listing and phase ran: this run's rows are the whole truth.
    Complete,
    /// It ended early by its own choice, or on an error: what it found
    /// is true, and what it did not reach keeps its row.
    CutShort,
    /// Told to stop. A failure seen after a stop is the stop's, and a
    /// phase the stop kept from running would read as one that recovered.
    Stopped,
}

/// A cheap, cloneable handle on one run's problems; every clone adds to
/// the same set.
#[derive(Clone, Default)]
pub struct RunProblems {
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for RunProblems {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunProblems").finish_non_exhaustive()
    }
}

type Untried = Box<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Default)]
struct State {
    config: Option<Vec<DownloadProblem>>,
    run: Vec<RunProblem>,
    cut_short: bool,
    records: BTreeMap<String, Records>,
    skipped: BTreeMap<String, Vec<SkippedRecord>>,
    silent: Option<Vec<SilentEntry>>,
}

#[derive(Default)]
struct Records {
    failed: Vec<RecordProblem>,
    tried: Tried,
}

#[derive(Default)]
enum Tried {
    /// Only the records that failed: nothing else is cleared.
    #[default]
    OnlyTheFailed,
    /// Every record of the table, if the run was not stopped.
    All,
    /// Every record of the table but the ones this says were not tried.
    AllBut(Untried),
}

impl RunProblems {
    /// A collector nothing will write: for a test of one piece of a
    /// download, or a caller with no store. A download gets its own from
    /// [`collecting`].
    pub fn unwritten() -> Self {
        Self::default()
    }

    /// A listing that did not come back as an enumeration, so what is
    /// stored under it was left alone.
    pub fn listing(&self, name: &str, detail: impl Into<String>) {
        self.push(RunProblem::listing(name, detail));
    }

    /// A whole phase that failed before it did its work.
    pub fn phase(&self, name: &str, detail: impl Into<String>) {
        self.push(RunProblem::phase(name, detail));
    }

    pub fn push(&self, problem: RunProblem) {
        self.lock().run.push(problem);
    }

    pub fn extend(&self, problems: impl IntoIterator<Item = RunProblem>) {
        self.lock().run.extend(problems);
    }

    /// Run one phase so that its failure, an error or a panic, costs only
    /// that phase: `None`, and a `phase:<name>` row.
    pub async fn run_phase<T>(
        &self,
        name: &str,
        run: impl Future<Output = Result<T>>,
    ) -> Option<T> {
        let detail = match catch_unwind(run).await {
            Ok(Ok(v)) => return Some(v),
            Ok(Err(e)) => format!("{e:#}"),
            Err(panic) => format!("panicked: {}", panic_message(&*panic)),
        };
        self.phase(name, detail);
        None
    }

    /// The run ended before it tried every listing and phase: a give-up,
    /// a budget spent. Its listing and phase rows are added to the last
    /// run's, where a complete run's replace them.
    pub fn cut_short(&self) {
        self.lock().cut_short = true;
    }

    /// The configured entries upstream does not have, once every one has
    /// been looked up: the whole set, an empty one included, so an entry
    /// the config no longer names or upstream now has loses its row.
    pub fn config(&self, problems: impl IntoIterator<Item = DownloadProblem>) {
        self.lock()
            .config
            .get_or_insert_with(Vec::new)
            .extend(problems);
    }

    /// One record of `table` this run tried to fetch and could not.
    pub fn record_failed(&self, table: &str, id: &str, detail: impl Into<String>) {
        self.records_failed([RecordProblem::new(table, id, detail)]);
    }

    pub fn records_failed(&self, problems: impl IntoIterator<Item = RecordProblem>) {
        let mut state = self.lock();
        for p in problems {
            state
                .records
                .entry(p.table.clone())
                .or_default()
                .failed
                .push(p);
        }
    }

    /// This run tried every record of `table`, so a row of an earlier
    /// run that is not among this run's failures is one that fetched.
    /// A stopped run did not try them all, and clears none.
    pub fn records_tried_all(&self, table: &str) {
        self.lock()
            .records
            .entry(table.to_string())
            .or_default()
            .tried = Tried::All;
    }

    /// As [`Self::records_tried_all`], but for the records `untried`
    /// names by id: the run has no verdict on those, and their rows
    /// stand. The run says exactly what it did not try, so this holds
    /// for a stopped run too.
    pub fn records_tried_all_but(
        &self,
        table: &str,
        untried: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) {
        self.lock()
            .records
            .entry(table.to_string())
            .or_default()
            .tried = Tried::AllBut(Box::new(untried));
    }

    /// What one `part` of the download (a feed, a file) read and chose
    /// not to store, the last time it read its input. Replaces that
    /// part's rows, so a part that read nothing this run says nothing.
    pub fn skipped(&self, part: &str, skipped: impl IntoIterator<Item = SkippedRecord>) {
        self.lock()
            .skipped
            .entry(part.to_string())
            .or_default()
            .extend(skipped);
    }

    /// The configured entries that have gone quiet, once every one has
    /// been checked: the whole set.
    pub fn silent(&self, silent: impl IntoIterator<Item = SilentEntry>) {
        self.lock()
            .silent
            .get_or_insert_with(Vec::new)
            .extend(silent);
    }

    /// How many listing or phase problems the run has so far, for a
    /// summary line.
    pub fn count(&self, kind: RunProblemKind) -> usize {
        self.lock().run.iter().filter(|p| p.kind == kind).count()
    }

    /// The listing and phase problems so far.
    pub fn run_problems(&self) -> Vec<RunProblem> {
        self.lock().run.clone()
    }

    async fn write(&self, pool: &sqlx::SqlitePool, ended: Ended) -> Result<()> {
        let state = std::mem::take(&mut *self.lock());
        let mut sweeps: Vec<Sweep<'_>> = Vec::new();
        let mut out: Vec<rows::Row> = Vec::new();
        fn whole<'a>(prefix: String) -> Sweep<'a> {
            Sweep { prefix, keep: None }
        }

        if let Some(config) = &state.config {
            // A stopped run may not have looked every entry up.
            if ended != Ended::Stopped {
                sweeps.push(whole(rows::CONFIG_SWEEP.to_string()));
            }
            out.extend(rows::config_rows(config));
        }
        match ended {
            Ended::Complete => {
                sweeps.extend(rows::run_prefixes().into_iter().map(whole));
                out.extend(rows::run_rows(&state.run));
            }
            Ended::CutShort => out.extend(rows::run_rows(&state.run)),
            Ended::Stopped => {}
        }
        for (table, records) in &state.records {
            let prefix = rows::record_prefix(table);
            match &records.tried {
                Tried::OnlyTheFailed => {}
                Tried::All if ended == Ended::Stopped => {}
                Tried::All => sweeps.push(whole(prefix)),
                Tried::AllBut(untried) => sweeps.push(Sweep {
                    prefix,
                    keep: Some(untried.as_ref()),
                }),
            }
            out.extend(rows::record_rows(&records.failed));
        }
        for (part, skipped) in &state.skipped {
            sweeps.push(whole(rows::skipped_prefix(part)));
            out.extend(rows::skipped_rows(part, skipped));
        }
        if let Some(silent) = &state.silent {
            sweeps.push(whole(rows::SILENT_SWEEP.to_string()));
            out.extend(rows::silent_rows(silent));
        }
        if sweeps.is_empty() && out.is_empty() {
            return Ok(());
        }
        rows::apply(pool, &sweeps, out).await
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Nothing panics while holding the lock, so a poisoned one still
        // holds a whole set.
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Unwind safety: a piece that panics mid-write drops its transaction, and
/// a dropped sqlx transaction rolls back, so the store is left as the last
/// piece that finished left it.
async fn catch_unwind<F: Future>(run: F) -> std::thread::Result<F::Output> {
    let mut run = std::pin::pin!(run);
    std::future::poll_fn(move |cx| {
        match std::panic::catch_unwind(AssertUnwindSafe(|| run.as_mut().poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(v)) => Poll::Ready(Ok(v)),
            Err(panic) => Poll::Ready(Err(panic)),
        }
    })
    .await
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
    use datalib_problems::{Problem, Reason, Severity};

    async fn store(dir: &tempfile::TempDir) -> sqlx::SqlitePool {
        crate::doltlite_raw::open(&dir.path().join("p.doltlite_db"), &[])
            .await
            .unwrap()
    }

    async fn keys(pool: &sqlx::SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT scope_key FROM problems ORDER BY scope_key")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    async fn run(pool: &sqlx::SqlitePool, body: impl FnOnce(&RunProblems)) {
        run_with(pool, &StopFlag::new(), body).await
    }

    async fn run_with(pool: &sqlx::SqlitePool, stop: &StopFlag, body: impl FnOnce(&RunProblems)) {
        collecting(pool, stop, |problems| async move {
            body(&problems);
            Ok(())
        })
        .await
        .unwrap();
    }

    /// A complete run's listing and phase rows replace the last run's,
    /// both kinds at once, and leave the other kinds alone. A key seen
    /// again keeps its `first_seen_at_utc`; two problems on one key are
    /// one row.
    #[tokio::test]
    async fn a_complete_runs_listings_and_phases_replace_the_last_runs() {
        let d = tempfile::tempdir().unwrap();
        let pool = store(&d).await;
        let rows = || async {
            sqlx::query_as::<_, (String, String, String, String)>(
                "SELECT scope_key, severity, reason, first_seen_at_utc FROM problems \
                 ORDER BY scope_key",
            )
            .fetch_all(&pool)
            .await
            .unwrap()
        };
        run(&pool, |p| {
            p.config([DownloadProblem::not_found(
                "only_labels",
                "x",
                "no such label",
            )]);
            p.listing("workouts", "HTTP 500");
            p.phase("weight", "cursor");
            p.listing("workouts", "a second failure on the same key");
        })
        .await;
        let first = rows().await;
        assert_eq!(
            first
                .iter()
                .map(|r| (r.0.as_str(), r.1.as_str(), r.2.as_str()))
                .collect::<Vec<_>>(),
            [
                ("config:only_labels:x", "warning", "not_found"),
                ("listing:workouts", "error", "fetch_failed"),
                ("phase:weight", "error", "fetch_failed"),
            ]
        );

        run(&pool, |p| p.listing("workouts", "HTTP 502")).await;
        let second = rows().await;
        assert_eq!(
            second.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            ["config:only_labels:x", "listing:workouts"],
            "the phase row went; the config row is not this run's to touch"
        );
        assert_eq!(
            second[1].3, first[1].3,
            "a key seen again keeps when it was first seen"
        );

        run(&pool, |_| {}).await;
        assert_eq!(keys(&pool).await, ["config:only_labels:x"]);
        pool.close().await;
    }

    /// A stopped run did not reach every listing, so the ones it missed
    /// would read as recovered, and a failure after a stop is the stop's.
    #[tokio::test]
    async fn a_stopped_run_leaves_the_listing_and_phase_rows_as_they_were() {
        let d = tempfile::tempdir().unwrap();
        let pool = store(&d).await;
        run(&pool, |p| p.listing("workouts", "HTTP 500")).await;

        let stop = StopFlag::new();
        stop.request();
        run_with(&pool, &stop, |p| p.phase("weight", "interrupted")).await;
        assert_eq!(keys(&pool).await, ["listing:workouts"]);
        pool.close().await;
    }

    /// A run that gave up, or ended on an error, found what it found and
    /// did not reach the rest: its rows are added and none is cleared.
    #[tokio::test]
    async fn a_run_cut_short_adds_its_rows_and_clears_none() {
        let d = tempfile::tempdir().unwrap();
        let pool = store(&d).await;
        run(&pool, |p| p.listing("workouts", "HTTP 500")).await;

        run(&pool, |p| {
            p.phase("fetch", "the retry loop gave up");
            p.cut_short();
        })
        .await;
        assert_eq!(keys(&pool).await, ["listing:workouts", "phase:fetch"]);

        let failed: Result<()> = collecting(&pool, &StopFlag::new(), |p| async move {
            p.listing("devices", "HTTP 502");
            anyhow::bail!("the credential was refused")
        })
        .await;
        assert!(failed.is_err());
        assert_eq!(
            keys(&pool).await,
            ["listing:devices", "listing:workouts", "phase:fetch"]
        );
        pool.close().await;
    }

    /// A run's configured-entry rows are the whole truth once it has
    /// looked every entry up: one the config no longer names, or that
    /// upstream now has, is gone. A run that never looked says nothing.
    #[tokio::test]
    async fn config_rows_are_replaced_by_a_run_that_resolved_its_config() {
        let d = tempfile::tempdir().unwrap();
        let pool = store(&d).await;
        run(&pool, |p| {
            p.config([
                DownloadProblem::not_found("only_labels", "Recieved", "no such label"),
                DownloadProblem::forbidden("channels", "C9", "private"),
                DownloadProblem::not_found("only_labels", "Recieved", "named twice"),
            ])
        })
        .await;
        assert_eq!(
            keys(&pool).await,
            ["config:channels:C9", "config:only_labels:Recieved"],
            "an entry the config names twice is one row, and costs no other row"
        );

        run(&pool, |_| {}).await;
        assert_eq!(keys(&pool).await.len(), 2, "a run that did not look");

        run(&pool, |p| p.config([])).await;
        assert!(keys(&pool).await.is_empty());
        pool.close().await;
    }

    /// A record's row clears when the run tried it again and it is not
    /// among the failures, and only then: another table's rows, and the
    /// ones the run says it did not try, stand.
    #[tokio::test]
    async fn a_records_row_stands_until_a_run_tries_it_again() {
        let d = tempfile::tempdir().unwrap();
        let pool = store(&d).await;
        run(&pool, |p| {
            p.record_failed("media_files", "bridge/a.mp4", "unreadable");
            p.record_failed("media_files", "holodeck/b.mp4", "unreadable");
            p.record_failed("contacts", "/dav/worf.vcf", "no UID");
        })
        .await;
        assert_eq!(keys(&pool).await.len(), 3);

        // The next run could not list `holodeck/`, and read `bridge/a.mp4`.
        run(&pool, |p| {
            p.records_tried_all_but("media_files", |id| id.starts_with("holodeck/"))
        })
        .await;
        assert_eq!(
            keys(&pool).await,
            [
                "record:contacts:/dav/worf.vcf",
                "record:media_files:holodeck/b.mp4"
            ]
        );

        // A stopped run did not try them all.
        let stop = StopFlag::new();
        stop.request();
        run_with(&pool, &stop, |p| p.records_tried_all("media_files")).await;
        assert_eq!(keys(&pool).await.len(), 2);

        run(&pool, |p| p.records_tried_all("media_files")).await;
        assert_eq!(keys(&pool).await, ["record:contacts:/dav/worf.vcf"]);
        pool.close().await;
    }

    /// A failure with no claim about the rest of its table is added, and
    /// replaces its own earlier row without a second one.
    #[tokio::test]
    async fn a_failed_record_alone_clears_nothing_else() {
        let d = tempfile::tempdir().unwrap();
        let pool = store(&d).await;
        run(&pool, |p| {
            p.record_failed("gmail_messages", "1a0b", "HTTP 403")
        })
        .await;
        run(&pool, |p| {
            p.record_failed("gmail_messages", "1a0c", "HTTP 500");
            p.record_failed("gmail_messages", "1a0b", "HTTP 404");
        })
        .await;
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT scope_key, severity, sample FROM problems ORDER BY scope_key")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            [
                (
                    "record:gmail_messages:1a0b".to_string(),
                    Severity::Error.as_str().to_string(),
                    "HTTP 404".to_string()
                ),
                (
                    "record:gmail_messages:1a0c".to_string(),
                    Severity::Error.as_str().to_string(),
                    "HTTP 500".to_string()
                ),
            ]
        );
        pool.close().await;
    }

    /// Takeout's 321 keyless saved places once reached only the log. A
    /// part's report replaces that part's rows and no other's, since a
    /// feed that skipped an unchanged file reports nothing and its rows
    /// must stand.
    #[tokio::test]
    async fn skipped_entries_are_rows_replaced_per_part() {
        let d = tempfile::tempdir().unwrap();
        let pool = store(&d).await;
        let keyless = |entry: &str| SkippedRecord {
            entry: entry.to_string(),
            problem: Problem::record(Reason::NoIdentity, entry),
        };
        let long = format!("https://maps.example/?q={}", "x".repeat(500));
        run(&pool, |p| {
            p.skipped(
                "places",
                [keyless(&long), keyless("Ten Forward"), keyless(&long)],
            );
            p.skipped("watch", [keyless("https://www.youtube.com/post/Ugk")]);
        })
        .await;
        let first = keys(&pool).await;
        assert_eq!(first.len(), 3, "a repeated entry is one row: {first:?}");
        assert!(first.iter().all(|k| k.len() <= 96), "{first:?}");

        run(&pool, |p| p.skipped("places", [keyless("Ten Forward")])).await;
        let second = keys(&pool).await;
        assert_eq!(second.len(), 2, "{second:?}");
        assert!(second.iter().any(|k| k.starts_with("skipped:watch:")));

        run(&pool, |p| p.skipped("places", [])).await;
        let third = keys(&pool).await;
        assert_eq!(third.len(), 1, "{third:?}");
        assert!(third[0].starts_with("skipped:watch:"));
        pool.close().await;
    }

    /// A device that went quiet is one warning, and it goes once the
    /// device speaks again.
    #[tokio::test]
    async fn a_silent_entry_is_one_warning_until_it_speaks() {
        let d = tempfile::tempdir().unwrap();
        let pool = store(&d).await;
        run(&pool, |p| {
            p.silent([SilentEntry {
                name: "cargo_bay_freezer".into(),
                detail: "no readings since 2369-03-01T00:00:00+00:00".into(),
            }])
        })
        .await;
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT scope_key, severity, reason FROM problems")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            [(
                "silent:cargo_bay_freezer".to_string(),
                Severity::Warning.as_str().to_string(),
                "silent".to_string()
            )]
        );
        run(&pool, |p| p.silent([])).await;
        assert!(keys(&pool).await.is_empty());
        pool.close().await;
    }

    /// A panic inside one Takeout feed once ended the whole step.
    #[tokio::test]
    async fn a_phase_that_panics_costs_only_itself() {
        let problems = RunProblems::unwritten();
        let got: Option<()> = problems
            .run_phase("youtube_watch_history", async {
                panic!("sliced through a character")
            })
            .await;
        assert!(got.is_none());
        assert_eq!(
            problems.run_problems(),
            [RunProblem::phase(
                "youtube_watch_history",
                "panicked: sliced through a character"
            )]
        );
        let failing =
            async { Err::<(), _>(anyhow::anyhow!("not JSON").context("parse Saved Places.json")) };
        assert!(problems
            .run_phase("maps_saved_places", failing)
            .await
            .is_none());
        assert_eq!(
            problems.run_problems()[1].detail,
            "parse Saved Places.json: not JSON"
        );
        assert_eq!(
            problems.run_phase("maps_reviews", async { Ok(2) }).await,
            Some(2)
        );
        assert_eq!(problems.count(RunProblemKind::Phase), 2);
    }
}
