//! Problems a run has that are not about one record: a configured entry
//! the upstream does not have, a listing that did not come back as an
//! enumeration, a phase that failed wholesale.
//!
//! Distinct from the per-item transient failures `FetchSummary` counts as
//! `errors` and `record_object_error` pins to the record. These are about
//! the run's shape, so they are keyed by what failed rather than by a
//! record, and a run's set replaces the last one's whole: a corrected
//! config, or a listing that answers again, clears its row.
//!
//! Reporting one must not fail the run. A misspelling in a five-entry
//! list costs that entry and nothing else, the same way a config entry
//! the loader cannot use costs that entry and nothing else.

use serde::{Deserialize, Serialize};
use strum::{EnumString, IntoStaticStr, VariantArray};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    VariantArray,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ProblemReason {
    /// The upstream has nothing by that name.
    NotFound,
    /// It exists, but this credential cannot read it.
    Forbidden,
}

impl ProblemReason {
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// `None` for a spelling this build does not know.
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadProblem {
    /// The config key that named it, e.g. `only_extract_labels`.
    pub setting: String,
    /// The value, spelled the way the config spelled it.
    pub value: String,
    pub reason: ProblemReason,
    /// What upstream said, or what the reader should do about it.
    pub detail: String,
}

impl DownloadProblem {
    pub fn not_found(setting: &str, value: &str, detail: impl Into<String>) -> Self {
        Self {
            setting: setting.to_string(),
            value: value.to_string(),
            reason: ProblemReason::NotFound,
            detail: detail.into(),
        }
    }

    pub fn forbidden(setting: &str, value: &str, detail: impl Into<String>) -> Self {
        Self {
            setting: setting.to_string(),
            value: value.to_string(),
            reason: ProblemReason::Forbidden,
            detail: detail.into(),
        }
    }
}

/// What a configured list of names resolved to.
#[derive(Debug)]
pub struct Resolution<T> {
    pub resolved: Vec<T>,
    pub problems: Vec<DownloadProblem>,
}

impl<T> Default for Resolution<T> {
    fn default() -> Self {
        Self {
            resolved: Vec::new(),
            problems: Vec::new(),
        }
    }
}

impl<T> Resolution<T> {
    /// Every configured entry missed.
    ///
    /// The caller has to decide what that means, because it depends on
    /// what an empty result does downstream. For a *filter* it is
    /// usually fatal: an empty filter means "everything", so falling
    /// through would mirror the whole account the config was narrowing.
    pub fn nothing_resolved(&self) -> bool {
        self.resolved.is_empty() && !self.problems.is_empty()
    }
}

/// Resolve a configured list against what upstream actually has,
/// keeping the entries that resolve and recording the ones that do not.
///
/// `lookup` returns `Err(detail)` for a miss, where `detail` says what
/// the reader should do — usually the list of valid names.
///
/// Never returns an error itself. One misspelling costs that entry, the
/// same way a config entry the loader cannot use costs that entry and
/// nothing else.
pub fn resolve_configured<T, F>(setting: &str, specs: &[String], mut lookup: F) -> Resolution<T>
where
    F: FnMut(&str) -> Result<T, String>,
{
    let mut out = Resolution::default();
    for spec in specs {
        match lookup(spec) {
            Ok(v) => out.resolved.push(v),
            Err(detail) => out
                .problems
                .push(DownloadProblem::not_found(setting, spec, detail)),
        }
    }
    out
}

/// One `problems` row per configured entry upstream does not have, in
/// the raw store, keyed `config:<setting>:<value>`, which is what
/// reaches the screen; the step's log says how many
/// (`datalib_problems::log_recorded`). The rows are the whole truth every run: a
/// run's list replaces the last one's, so an entry the config no longer
/// names, or that upstream now has, is gone. Recording never fails the
/// run; a store that cannot take the rows is said and passed over.
pub async fn report(pool: &sqlx::SqlitePool, problems: &[DownloadProblem]) {
    use datalib_problems::{Outcome, Problem, Reason, Severity};
    let rows: Vec<(String, Outcome, Problem)> = problems
        .iter()
        .map(|p| {
            let reason = match p.reason {
                ProblemReason::NotFound => Reason::NotFound,
                ProblemReason::Forbidden => Reason::Forbidden,
            };
            (
                format!("{CONFIG_PREFIX}{}:{}", p.setting, p.value),
                Outcome::Dropped,
                Problem::field(&p.setting, reason, &p.detail).severity(Severity::Warning),
            )
        })
        .collect();
    if let Err(e) = replace_prefixed(pool, &[CONFIG_PREFIX], &rows).await {
        tracing::warn!(
            error = %format!("{e:#}"),
            "download_problem: could not record the configured entries that did not resolve; \
             the Manage row will not show them"
        );
    }
}

/// The sweep key's prefix of a configured entry's row: every row
/// [`report`] writes, and only those, so a run's report can replace the
/// last one's whole.
const CONFIG_PREFIX: &str = "config:";

/// What a run could not do as a whole. Each variant is a sweep-key
/// prefix, so [`report_run`] can replace the last run's rows of every
/// kind at once.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    VariantArray,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum RunProblemKind {
    /// A listing that did not come back as an enumeration — not an
    /// array, nothing at all, or a page walk that stopped on an error —
    /// so absence from it means nothing and the stored rows were left
    /// alone. What is stored is stale until it lists again.
    Listing,
    /// A whole phase of the run failed before it did its work; the
    /// other phases ran.
    Phase,
}

impl RunProblemKind {
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// `None` for a spelling this build does not know.
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    fn key_prefix(self) -> String {
        format!("{}:", self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunProblem {
    pub kind: RunProblemKind,
    /// The listing or phase, as the provider names it: `workouts`,
    /// `devices`, `weight`.
    pub name: String,
    /// What went wrong, in the words of the error.
    pub detail: String,
    /// Whether the credential was refused: a listing the service
    /// would not give this token is a warning the reader can act on
    /// (ask for access), where one that failed is an error.
    #[serde(default)]
    pub forbidden: bool,
}

impl RunProblem {
    pub fn listing(name: &str, detail: impl Into<String>) -> Self {
        Self {
            kind: RunProblemKind::Listing,
            name: name.to_string(),
            detail: detail.into(),
            forbidden: false,
        }
    }

    pub fn phase(name: &str, detail: impl Into<String>) -> Self {
        Self {
            kind: RunProblemKind::Phase,
            name: name.to_string(),
            detail: detail.into(),
            forbidden: false,
        }
    }

    /// A listing the service refused this credential: an org, a
    /// workspace, a scope the token does not reach.
    pub fn forbidden(name: &str, detail: impl Into<String>) -> Self {
        Self {
            kind: RunProblemKind::Listing,
            name: name.to_string(),
            detail: detail.into(),
            forbidden: true,
        }
    }

    pub fn key(&self) -> String {
        format!("{}{}", self.kind.key_prefix(), self.name)
    }
}

/// As [`report`], for what a run could not do as a whole: one `problems`
/// row each, keyed `listing:<name>` / `phase:<name>`. Every row of both kinds
/// is replaced each run — call it with an empty slice on a clean run so
/// the last run's rows go. An error, because the reader has nothing
/// current for that listing or phase; two problems on one key keep the
/// first's detail.
pub async fn report_run(pool: &sqlx::SqlitePool, problems: &[RunProblem]) {
    use datalib_problems::{Outcome, Problem, Reason, Severity};
    let mut seen = std::collections::HashSet::new();
    let rows: Vec<(String, Outcome, Problem)> = problems
        .iter()
        .filter(|p| seen.insert(p.key()))
        .map(|p| {
            let problem = if p.forbidden {
                Problem::record(Reason::Forbidden, &p.detail).severity(Severity::Warning)
            } else {
                Problem::record(Reason::FetchFailed, &p.detail).severity(Severity::Error)
            };
            (p.key(), Outcome::Dropped, problem)
        })
        .collect();
    let prefixes: Vec<String> = RunProblemKind::VARIANTS
        .iter()
        .map(|k| k.key_prefix())
        .collect();
    let prefixes: Vec<&str> = prefixes.iter().map(String::as_str).collect();
    if let Err(e) = replace_prefixed(pool, &prefixes, &rows).await {
        tracing::warn!(
            error = %format!("{e:#}"),
            "run_problem: could not record what the run could not do; \
             the Manage row will not show it"
        );
    }
}

/// One record a download could not fetch, named by the id **upstream**
/// uses for it.
///
/// The other way to report this is
/// [`crate::doltlite_raw::record_object_error`], and it is the right one
/// wherever the record has a `_bookkeeping` sidecar to stamp. This is
/// for the case that has none: a fetch that fails never reaches the
/// point of minting an id in our own keyspace, so the only name the run
/// has for it is the one upstream gave. Gmail is the example —
/// `gmail_messages` maps Gmail's id to the row it produced, and a
/// message that would not fetch produced none.
#[derive(Debug, Clone)]
pub struct RecordProblem {
    /// The raw table the id belongs to, e.g. `gmail_messages`.
    pub table: String,
    /// Upstream's id.
    pub id: String,
    /// What upstream said.
    pub detail: String,
}

/// The sweep key's prefix of a per-record row: every row
/// [`report_records`] writes, and only those, so a run's report
/// replaces the last one's whole and a record that fetches this time
/// stops being a problem.
pub const RECORD_PREFIX: &str = "record:";

impl RecordProblem {
    pub fn new(table: &str, id: &str, detail: impl Into<String>) -> Self {
        Self {
            table: table.to_string(),
            id: id.to_string(),
            detail: detail.into(),
        }
    }

    fn key(&self) -> String {
        format!("{RECORD_PREFIX}{}:{}", self.table, self.id)
    }
}

/// Records this run could not fetch. Replaces the previous run's set,
/// so one that succeeds this time drops off by itself.
/// The last run's `record:<table>:<id_prefix>…` rows, to carry into this
/// run's [`report_records`] set those this run did not try again: a
/// whole-set replace would otherwise clear them without a retry.
pub async fn earlier_records(
    pool: &sqlx::SqlitePool,
    table: &str,
    id_prefix: &str,
) -> anyhow::Result<Vec<RecordProblem>> {
    use anyhow::Context as _;
    let key_prefix = format!("{RECORD_PREFIX}{table}:");
    // `INSTR(x, ?) = 1` rather than `LIKE`: `_` in a path is a wildcard
    // to LIKE.
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT scope_key, sample FROM problems WHERE scope_kind = ? AND INSTR(scope_key, ?) = 1",
    )
    .bind(datalib_problems::ScopeKind::Entity.as_str())
    .bind(format!("{key_prefix}{id_prefix}"))
    .fetch_all(pool)
    .await
    .with_context(|| format!("read the last run's {table} record problems"))?;
    Ok(rows
        .into_iter()
        .filter_map(|(key, sample)| {
            let id = key.strip_prefix(&key_prefix)?;
            Some(RecordProblem::new(table, id, sample))
        })
        .collect())
}

pub async fn report_records(pool: &sqlx::SqlitePool, problems: &[RecordProblem]) {
    use datalib_problems::{Outcome, Problem, Reason, Severity};
    let rows: Vec<(String, Outcome, Problem)> = problems
        .iter()
        .map(|p| {
            (
                p.key(),
                Outcome::Dropped,
                Problem::record(Reason::FetchFailed, &p.detail).severity(Severity::Error),
            )
        })
        .collect();
    if let Err(e) = replace_prefixed(pool, &[RECORD_PREFIX], &rows).await {
        tracing::warn!(
            error = %format!("{e:#}"),
            "record_problem: could not record the records that would not fetch; \
             the Manage row will not show them"
        );
    }
}

/// An entry the download read and chose not to store: no usable key, or
/// a kind the mirror does not hold. Named by `entry`, whatever names it
/// stably in the export (a URL, the entry's own text); the key hashes it,
/// so the length and contents of `entry` never reach the sweep key.
#[derive(Debug, Clone)]
pub struct SkippedRecord {
    pub entry: String,
    pub problem: datalib_problems::Problem,
}

/// The sweep key's prefix of a [`report_skipped`] row.
pub const SKIPPED_PREFIX: &str = "skipped:";

/// What one `part` of a download (a feed, a file) skipped the last time
/// it read its input. Replaces only that part's rows, so a part that did
/// not re-read anything this run must not call it: its rows still hold.
pub async fn report_skipped(pool: &sqlx::SqlitePool, part: &str, skipped: &[SkippedRecord]) {
    use datalib_problems::Outcome;
    let mut seen = std::collections::HashSet::new();
    let rows: Vec<(String, Outcome, datalib_problems::Problem)> = skipped
        .iter()
        .map(|s| {
            let hash = blake3::hash(s.entry.as_bytes()).to_hex();
            (format!("{SKIPPED_PREFIX}{part}:{}", &hash[..16]), s)
        })
        .filter(|(key, _)| seen.insert(key.clone()))
        .map(|(key, s)| (key, Outcome::Dropped, s.problem.clone()))
        .collect();
    let prefix = format!("{SKIPPED_PREFIX}{part}:");
    if let Err(e) = replace_prefixed(pool, &[&prefix], &rows).await {
        tracing::warn!(
            error = %format!("{e:#}"),
            part,
            "could not record the entries that were skipped; the Manage row will not show them"
        );
    }
}

/// A configured entry upstream has sent nothing new for a while, named
/// the way the config names it.
#[derive(Debug, Clone)]
pub struct SilentEntry {
    pub name: String,
    /// Since when, in words the reader can act on.
    pub detail: String,
}

/// The sweep key's prefix of a [`report_silent`] row.
const SILENT_PREFIX: &str = "silent:";

/// Entries that have gone quiet. A warning, not an error: nothing
/// stored was lost. Replaces the previous run's set, so an entry that
/// speaks again drops off by itself.
pub async fn report_silent(pool: &sqlx::SqlitePool, silent: &[SilentEntry]) {
    use datalib_problems::{Outcome, Problem, Reason, Severity};
    let rows: Vec<(String, Outcome, Problem)> = silent
        .iter()
        .map(|s| {
            (
                format!("{SILENT_PREFIX}{}", s.name),
                Outcome::Ok,
                Problem::record(Reason::Silent, &s.detail).severity(Severity::Warning),
            )
        })
        .collect();
    if let Err(e) = replace_prefixed(pool, &[SILENT_PREFIX], &rows).await {
        tracing::warn!(
            error = %format!("{e:#}"),
            "silent_entry: could not record the entries that went quiet; \
             the Manage row will not show them"
        );
    }
}

/// Delete every entity-scoped row whose key starts with one of
/// `prefixes`, then write `rows`, in one transaction. A key that was
/// there before keeps its `first_seen_at_utc`, so the screen can say
/// how long a listing has been failing.
async fn replace_prefixed(
    pool: &sqlx::SqlitePool,
    prefixes: &[&str],
    rows: &[(String, datalib_problems::Outcome, datalib_problems::Problem)],
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use datalib_problems::{ProblemRow, Scope, ScopeKind, Stage};
    use datalib_table::BulkUpsertable as _;
    let mut tx = pool.begin().await.context("begin")?;
    let mut first_seen: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for prefix in prefixes {
        // `INSTR(x, ?) = 1` rather than `LIKE`: `_` in a value is a
        // wildcard to LIKE.
        let earlier: Vec<(String, String)> = sqlx::query_as(
            "SELECT scope_key, first_seen_at_utc FROM problems \
             WHERE scope_kind = ? AND INSTR(scope_key, ?) = 1",
        )
        .bind(ScopeKind::Entity.as_str())
        .bind(prefix)
        .fetch_all(&mut *tx)
        .await
        .with_context(|| format!("read the last run's {prefix} problems"))?;
        first_seen.extend(earlier);
        sqlx::query("DELETE FROM problems WHERE scope_kind = ? AND INSTR(scope_key, ?) = 1")
            .bind(ScopeKind::Entity.as_str())
            .bind(prefix)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("clear the last run's {prefix} problems"))?;
    }
    let (now, tz_offset) = datalib_time::IsoOffsetTimestamp::now_local().to_utc_and_offset();
    let mut stored = Vec::with_capacity(rows.len());
    for (key, outcome, problem) in rows {
        let row = ProblemRow {
            first_seen_at_utc: first_seen.get(key).cloned().unwrap_or_else(|| now.clone()),
            last_seen_at_utc: now.clone(),
            tz_offset: Some(tz_offset.clone()),
            ..ProblemRow::new(
                "",
                Stage::Fetch,
                Scope::Entity(key),
                None,
                *outcome,
                problem.clone(),
                None,
            )
        };
        let sql = crate::bulk::insert_sql::<ProblemRow>();
        // Audited: `sql` is built from `ProblemRow`'s associated consts,
        // never from row data; all values bound.
        row.bind_into(sqlx::query(sqlx::AssertSqlSafe(sql)))
            .execute(&mut *tx)
            .await
            .with_context(|| format!("record {key}"))?;
        stored.push(row);
    }
    tx.commit().await.context("commit")?;
    datalib_problems::note_recorded(&stored);
    Ok(())
}

/// One row to write: its sweep key, what became of the thing, and why.
pub(crate) type Row = (String, datalib_problems::Outcome, datalib_problems::Problem);

pub(crate) fn config_rows(problems: &[DownloadProblem]) -> Vec<Row> {
    use datalib_problems::{Outcome, Problem, Reason, Severity};
    problems
        .iter()
        .map(|p| {
            let reason = match p.reason {
                ProblemReason::NotFound => Reason::NotFound,
                ProblemReason::Forbidden => Reason::Forbidden,
            };
            (
                format!("{CONFIG_PREFIX}{}:{}", p.setting, p.value),
                Outcome::Dropped,
                Problem::field(&p.setting, reason, &p.detail).severity(Severity::Warning),
            )
        })
        .collect()
}

pub(crate) fn run_rows(problems: &[RunProblem]) -> Vec<Row> {
    use datalib_problems::{Outcome, Problem, Reason, Severity};
    problems
        .iter()
        .map(|p| {
            let problem = if p.forbidden {
                Problem::record(Reason::Forbidden, &p.detail).severity(Severity::Warning)
            } else {
                Problem::record(Reason::FetchFailed, &p.detail).severity(Severity::Error)
            };
            (p.key(), Outcome::Dropped, problem)
        })
        .collect()
}

pub(crate) fn run_prefixes() -> Vec<String> {
    RunProblemKind::VARIANTS
        .iter()
        .map(|k| k.key_prefix())
        .collect()
}

pub(crate) fn record_rows(problems: &[RecordProblem]) -> Vec<Row> {
    use datalib_problems::{Outcome, Problem, Reason, Severity};
    problems
        .iter()
        .map(|p| {
            (
                p.key(),
                Outcome::Dropped,
                Problem::record(Reason::FetchFailed, &p.detail).severity(Severity::Error),
            )
        })
        .collect()
}

pub(crate) fn record_prefix(table: &str) -> String {
    format!("{RECORD_PREFIX}{table}:")
}

pub(crate) fn skipped_rows(part: &str, skipped: &[SkippedRecord]) -> Vec<Row> {
    skipped
        .iter()
        .map(|s| {
            let hash = blake3::hash(s.entry.as_bytes()).to_hex();
            (
                format!("{}{}", skipped_prefix(part), &hash[..16]),
                datalib_problems::Outcome::Dropped,
                s.problem.clone(),
            )
        })
        .collect()
}

pub(crate) fn skipped_prefix(part: &str) -> String {
    format!("{SKIPPED_PREFIX}{part}:")
}

pub(crate) fn silent_rows(silent: &[SilentEntry]) -> Vec<Row> {
    use datalib_problems::{Outcome, Problem, Reason, Severity};
    silent
        .iter()
        .map(|s| {
            (
                format!("{SILENT_PREFIX}{}", s.name),
                Outcome::Ok,
                Problem::record(Reason::Silent, &s.detail).severity(Severity::Warning),
            )
        })
        .collect()
}

pub(crate) const CONFIG_SWEEP: &str = CONFIG_PREFIX;
pub(crate) const SILENT_SWEEP: &str = SILENT_PREFIX;

/// The rows a run has a verdict on: every entity-scoped row whose key
/// starts with `prefix`, less the ones `keep` says the run did not try
/// again (it is given the key with the prefix taken off).
pub(crate) struct Sweep<'a> {
    pub prefix: String,
    pub keep: Option<&'a (dyn Fn(&str) -> bool + Send + Sync)>,
}

/// SQLite's default bound-parameter limit is far above this; one
/// statement per chunk keeps a big sweep from being one statement per row.
const KEY_CHUNK: usize = 500;

/// Delete what `sweeps` cover, then write `rows`, in one transaction. A
/// key that was there before keeps its `first_seen_at_utc`, so the screen
/// can say how long something has been failing; two rows on one key keep
/// the first, since the key is the row's identity.
pub(crate) async fn apply(
    pool: &sqlx::SqlitePool,
    sweeps: &[Sweep<'_>],
    rows: Vec<Row>,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use datalib_problems::{ProblemRow, Scope, ScopeKind, Stage};
    use datalib_table::BulkUpsertable as _;
    use std::collections::{HashMap, HashSet};

    let mut seen = HashSet::new();
    let rows: Vec<Row> = rows
        .into_iter()
        .filter(|(key, _, _)| seen.insert(key.clone()))
        .collect();

    let mut tx = pool.begin().await.context("begin")?;
    let mut first_seen: HashMap<String, String> = HashMap::new();
    let mut gone: Vec<String> = Vec::new();
    for sweep in sweeps {
        // `INSTR(x, ?) = 1` rather than `LIKE`: `_` in a value is a
        // wildcard to LIKE.
        let earlier: Vec<(String, String)> = sqlx::query_as(
            "SELECT scope_key, first_seen_at_utc FROM problems \
             WHERE scope_kind = ? AND INSTR(scope_key, ?) = 1",
        )
        .bind(ScopeKind::Entity.as_str())
        .bind(&sweep.prefix)
        .fetch_all(&mut *tx)
        .await
        .with_context(|| format!("read the last run's {} problems", sweep.prefix))?;
        for (key, first) in earlier {
            let kept = sweep
                .keep
                .is_some_and(|keep| keep(key.strip_prefix(sweep.prefix.as_str()).unwrap_or(&key)));
            if !kept {
                gone.push(key.clone());
            }
            first_seen.insert(key, first);
        }
    }
    let unswept: Vec<&str> = rows
        .iter()
        .map(|(key, _, _)| key.as_str())
        .filter(|key| !first_seen.contains_key(*key))
        .collect();
    for chunk in unswept.chunks(KEY_CHUNK) {
        // Audited: only `?` placeholders are built, one per key; every
        // key is bound.
        let sql = format!(
            "SELECT scope_key, first_seen_at_utc FROM problems \
             WHERE scope_kind = ? AND scope_key IN ({})",
            vec!["?"; chunk.len()].join(",")
        );
        let mut q = sqlx::query_as::<_, (String, String)>(sqlx::AssertSqlSafe(sql))
            .bind(ScopeKind::Entity.as_str());
        for key in chunk {
            q = q.bind(*key);
        }
        let earlier = q
            .fetch_all(&mut *tx)
            .await
            .context("read the rows this run writes again")?;
        gone.extend(earlier.iter().map(|(key, _)| key.clone()));
        first_seen.extend(earlier);
    }
    for chunk in gone.chunks(KEY_CHUNK) {
        // Audited: as above.
        let sql = format!(
            "DELETE FROM problems WHERE scope_kind = ? AND scope_key IN ({})",
            vec!["?"; chunk.len()].join(",")
        );
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(ScopeKind::Entity.as_str());
        for key in chunk {
            q = q.bind(key);
        }
        q.execute(&mut *tx)
            .await
            .context("clear the rows this run has a verdict on")?;
    }
    let (now, tz_offset) = datalib_time::IsoOffsetTimestamp::now_local().to_utc_and_offset();
    let mut stored = Vec::with_capacity(rows.len());
    for (key, outcome, problem) in &rows {
        let row = ProblemRow {
            first_seen_at_utc: first_seen.get(key).cloned().unwrap_or_else(|| now.clone()),
            last_seen_at_utc: now.clone(),
            tz_offset: Some(tz_offset.clone()),
            ..ProblemRow::new(
                "",
                Stage::Fetch,
                Scope::Entity(key),
                None,
                *outcome,
                problem.clone(),
                None,
            )
        };
        let sql = crate::bulk::insert_sql::<ProblemRow>();
        // Audited: `sql` is built from `ProblemRow`'s associated consts,
        // never from row data; all values bound.
        row.bind_into(sqlx::query(sqlx::AssertSqlSafe(sql)))
            .execute(&mut *tx)
            .await
            .with_context(|| format!("record {key}"))?;
        stored.push(row);
    }
    tx.commit().await.context("commit")?;
    datalib_problems::note_recorded(&stored);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Takeout's 321 keyless saved places once reached only the log, one
    /// identical line each. A part's report replaces that part's rows and
    /// no other's, since a feed that skipped an unchanged file reports
    /// nothing and its rows must stand.
    #[tokio::test]
    async fn skipped_entries_are_rows_replaced_per_part() {
        use datalib_problems::{Problem, Reason, Severity};
        let d = tempfile::tempdir().unwrap();
        let pool = crate::doltlite_raw::open(&d.path().join("s.doltlite_db"), &[])
            .await
            .unwrap();
        let keys = || async {
            sqlx::query_as::<_, (String, String)>(
                "SELECT scope_key, severity FROM problems ORDER BY scope_key",
            )
            .fetch_all(&pool)
            .await
            .unwrap()
        };
        let keyless = |entry: &str| SkippedRecord {
            entry: entry.to_string(),
            problem: Problem::record(Reason::NoIdentity, entry),
        };
        let long = format!("https://maps.example/?q={}", "x".repeat(500));

        report_skipped(
            &pool,
            "places",
            &[keyless(&long), keyless("Ten Forward"), keyless(&long)],
        )
        .await;
        report_skipped(
            &pool,
            "watch",
            &[keyless("https://www.youtube.com/post/Ugk")],
        )
        .await;
        let first = keys().await;
        assert_eq!(first.len(), 3, "a repeated entry is one row: {first:?}");
        assert!(first.iter().all(|(k, _)| k.len() <= 96), "{first:?}");
        assert!(first.iter().all(|(_, s)| s == Severity::Error.as_str()));

        report_skipped(&pool, "places", &[keyless("Ten Forward")]).await;
        let second = keys().await;
        assert_eq!(second.len(), 2, "{second:?}");
        assert_eq!(
            second
                .iter()
                .filter(|(k, _)| k.starts_with("skipped:watch:"))
                .count(),
            1
        );

        report_skipped(&pool, "places", &[]).await;
        let third = keys().await;
        assert_eq!(third.len(), 1, "{third:?}");
        assert!(third[0].0.starts_with("skipped:watch:"));
        pool.close().await;
    }

    /// A record that would not fetch is a `problems` row keyed by the id
    /// upstream uses, and the next run's report replaces it — so one
    /// that fetches this time stops being a problem without anyone
    /// deleting anything.
    #[tokio::test]
    async fn a_record_that_would_not_fetch_is_reported_until_it_does() {
        use datalib_problems::Severity;
        let d = tempfile::tempdir().unwrap();
        let pool = crate::doltlite_raw::open(&d.path().join("r.doltlite_db"), &[])
            .await
            .unwrap();
        let rows = |pool: &sqlx::SqlitePool| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<_, (String, String, String)>(
                    "SELECT scope_key, severity, sample FROM problems ORDER BY scope_key",
                )
                .fetch_all(&pool)
                .await
                .unwrap()
            }
        };

        report_records(
            &pool,
            &[
                RecordProblem::new("gmail_messages", "1a0b526fbb117cd1", "HTTP 403"),
                RecordProblem::new("gmail_messages", "1a0ac00e42c0c4b8", "HTTP 500"),
            ],
        )
        .await;
        let first = rows(&pool).await;
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].0, "record:gmail_messages:1a0ac00e42c0c4b8");
        assert_eq!(
            first[0].1,
            Severity::Error.as_str(),
            "the record is missing from the mirror, not stale"
        );
        assert_eq!(first[0].2, "HTTP 500");

        // The next run gets one of them.
        report_records(
            &pool,
            &[RecordProblem::new(
                "gmail_messages",
                "1a0b526fbb117cd1",
                "HTTP 403",
            )],
        )
        .await;
        let second = rows(&pool).await;
        assert_eq!(
            second.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            ["record:gmail_messages:1a0b526fbb117cd1"],
            "the one that fetched is no longer a problem"
        );

        // And a clean run clears the lot.
        report_records(&pool, &[]).await;
        assert!(rows(&pool).await.is_empty());
    }

    /// A device that went quiet is one warning, whatever its windows
    /// said, and it goes once the device speaks again.
    #[tokio::test]
    async fn a_silent_entry_is_one_warning_until_it_speaks() {
        use datalib_problems::Severity;
        let d = tempfile::tempdir().unwrap();
        let pool = crate::doltlite_raw::open(&d.path().join("s.doltlite_db"), &[])
            .await
            .unwrap();
        let rows = |pool: &sqlx::SqlitePool| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<_, (String, String, String)>(
                    "SELECT scope_key, severity, reason FROM problems ORDER BY scope_key",
                )
                .fetch_all(&pool)
                .await
                .unwrap()
            }
        };
        let quiet = SilentEntry {
            name: "cargo_bay_freezer".into(),
            detail: "no readings since 2369-03-01T00:00:00+00:00".into(),
        };
        report_silent(&pool, &[quiet]).await;
        assert_eq!(
            rows(&pool).await,
            [(
                "silent:cargo_bay_freezer".to_string(),
                Severity::Warning.as_str().to_string(),
                "silent".to_string()
            )]
        );
        report_silent(&pool, &[]).await;
        assert!(rows(&pool).await.is_empty());
    }

    /// A run's report replaces the last one's: an entry the config no
    /// longer names, or that upstream now has, is gone the next run.
    #[tokio::test]
    async fn a_reports_rows_are_the_whole_truth_for_that_run() {
        let d = tempfile::tempdir().unwrap();
        let pool = crate::doltlite_raw::open(&d.path().join("p.doltlite_db"), &[])
            .await
            .unwrap();
        let keys = |pool: &sqlx::SqlitePool| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>("SELECT scope_key FROM problems ORDER BY scope_key")
                    .fetch_all(&pool)
                    .await
                    .unwrap()
            }
        };
        report(
            &pool,
            &[
                DownloadProblem::not_found("only_labels", "Recieved", "no such label"),
                DownloadProblem::forbidden("channels", "C9", "private"),
            ],
        )
        .await;
        assert_eq!(
            keys(&pool).await,
            ["config:channels:C9", "config:only_labels:Recieved"]
        );
        report(
            &pool,
            &[DownloadProblem::not_found(
                "only_labels",
                "Recieved",
                "no such label",
            )],
        )
        .await;
        assert_eq!(keys(&pool).await, ["config:only_labels:Recieved"]);
        report(&pool, &[]).await;
        assert!(keys(&pool).await.is_empty());
        pool.close().await;
    }

    /// A run's listing and phase rows replace the last run's, both kinds
    /// at once, and leave the configured-entry rows alone. A key seen
    /// again keeps its `first_seen_at_utc`.
    #[tokio::test]
    async fn run_problems_replace_their_own_kinds_and_keep_first_seen() {
        let d = tempfile::tempdir().unwrap();
        let pool = crate::doltlite_raw::open(&d.path().join("p.doltlite_db"), &[])
            .await
            .unwrap();
        let rows = |pool: &sqlx::SqlitePool| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<_, (String, String, String, String)>(
                    "SELECT scope_key, severity, reason, first_seen_at_utc FROM problems \
                     ORDER BY scope_key",
                )
                .fetch_all(&pool)
                .await
                .unwrap()
            }
        };
        report(
            &pool,
            &[DownloadProblem::not_found(
                "only_labels",
                "x",
                "no such label",
            )],
        )
        .await;
        report_run(
            &pool,
            &[
                RunProblem::listing("workouts", "HTTP 500"),
                RunProblem::phase("weight", "cursor"),
                RunProblem::listing("workouts", "a second failure on the same key"),
            ],
        )
        .await;
        let first = rows(&pool).await;
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
        let workouts_first_seen = first[1].3.clone();

        report_run(&pool, &[RunProblem::listing("workouts", "HTTP 502")]).await;
        let second = rows(&pool).await;
        assert_eq!(
            second.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            ["config:only_labels:x", "listing:workouts"],
            "the phase row went; the config row is not this report's to touch"
        );
        assert_eq!(
            second[1].3, workouts_first_seen,
            "a key seen again keeps when it was first seen"
        );

        report_run(&pool, &[]).await;
        assert_eq!(
            rows(&pool)
                .await
                .iter()
                .map(|r| r.0.as_str())
                .collect::<Vec<_>>(),
            ["config:only_labels:x"]
        );
        pool.close().await;
    }

    /// strum and serde are independent derives producing independent
    /// strings; the agreement is a real check, not a tautology.
    #[test]
    fn strum_and_serde_agree_on_every_variant() {
        for r in ProblemReason::VARIANTS {
            let serde = serde_json::to_string(r).unwrap();
            let serde = serde.trim_matches('"');
            assert_eq!(serde, r.as_str(), "{r:?}");
            assert_eq!(ProblemReason::parse(serde), Some(*r));
        }
        for k in RunProblemKind::VARIANTS {
            let serde = serde_json::to_string(k).unwrap();
            let serde = serde.trim_matches('"');
            assert_eq!(serde, k.as_str(), "{k:?}");
            assert_eq!(RunProblemKind::parse(serde), Some(*k));
        }
    }

    #[test]
    fn keeps_the_hits_and_records_the_misses() {
        let specs = vec!["a".to_string(), "nope".to_string(), "b".to_string()];
        let out = resolve_configured("things", &specs, |s| match s {
            "a" | "b" => Ok(s.to_uppercase()),
            _ => Err("known: a, b".to_string()),
        });
        assert_eq!(out.resolved, vec!["A", "B"]);
        assert_eq!(out.problems.len(), 1);
        assert_eq!(out.problems[0].value, "nope");
        assert_eq!(out.problems[0].setting, "things");
        assert!(!out.nothing_resolved());
    }

    /// The distinction the callers branch on. An empty configured list
    /// resolves to nothing and that is fine — it means "no filter". A
    /// list where every entry missed also resolves to nothing, and for a
    /// filter that would silently widen the scope to everything.
    #[test]
    fn tells_an_empty_config_apart_from_a_wholly_unresolvable_one() {
        let none = resolve_configured("things", &[], |s: &str| Ok::<_, String>(s.to_string()));
        assert!(
            !none.nothing_resolved(),
            "no filter configured is not a miss"
        );

        let specs = vec!["nope".to_string()];
        let all_missed = resolve_configured("things", &specs, |_| {
            Err::<String, _>("known: a".to_string())
        });
        assert!(all_missed.nothing_resolved());
        assert!(all_missed.resolved.is_empty());
    }

    #[test]
    fn an_unknown_spelling_is_none_rather_than_a_guess() {
        assert_eq!(ProblemReason::parse("teleported"), None);
    }
}
