//! A folder of Lightroom backups, mirrored oldest first, one commit per
//! backup dated when it was taken. `INGEST.md` §"A folder of backups"
//! has the rules this implements.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{NaiveDateTime, TimeZone};
use sqlx::sqlite::SqlitePool;

use datalib_etl::doltlite_raw as dr;
use datalib_etl::download_problems::RecordProblem;
use datalib_etl::progress::Progress;
use datalib_etl::scope_config;
use datalib_etl::stop::StopFlag;
use datalib_etl_sqlite_mirror::{MirrorOptions, MirrorStats};

use super::unpack::{self, is_catalog, is_zip};

/// The store's record of which backups it holds, one row per backup,
/// written in the commit that mirrored it.
pub const LEDGER: &str = "lightroom_backups";

const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS lightroom_backups (
    backup TEXT PRIMARY KEY,
    taken_at TEXT NOT NULL,
    file TEXT NOT NULL
)";

/// `scope_config`'s key for the filters the newest commit was mirrored
/// under.
const SCOPE: &str = "backups";

/// How Lightroom names a backup's folder: `2026-09-27 1650`, sometimes
/// with a note a person added after it.
const NAME_DATE_FORMAT: &str = "%Y-%m-%d %H%M";
const NAME_DATE_LEN: usize = "2026-09-27 1650".len();

/// `taken_at` as the ledger stores it.
const LEDGER_DATE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// One entry directly under the backups folder: a folder and the files
/// in it, or a catalog file sitting there on its own.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    /// The entry's name, which is also its key in [`LEDGER`].
    pub name: String,
    pub taken_at: NaiveDateTime,
    /// What to mirror: the `.zip` Lightroom wrote, else a bare `.lrcat`.
    pub file: PathBuf,
}

/// What a run will do, decided from the folder and the ledger alone.
#[derive(Debug, Default)]
pub struct Plan {
    /// Every backup found, oldest first, whether or not the store has it.
    pub found: Vec<Backup>,
    /// The ones to mirror now, oldest first.
    pub ingest: Vec<Backup>,
    /// `(entry name, why it cannot be mirrored)`.
    pub refused: Vec<(String, String)>,
}

pub fn plan(entries: Vec<Entry>, ledger: &BTreeMap<String, NaiveDateTime>) -> Plan {
    let mut out = Plan::default();
    for entry in entries {
        match read_entry(&entry) {
            Ok(Some(b)) => out.found.push(b),
            Ok(None) => {}
            Err(why) => out.refused.push((entry.name, why)),
        }
    }
    out.found
        .sort_by(|a, b| (a.taken_at, &a.name).cmp(&(b.taken_at, &b.name)));

    let newest_held = ledger.values().max().copied();
    for b in &out.found {
        if ledger.contains_key(&b.name) {
            continue;
        }
        match newest_held {
            // History is one line: a backup taken before the newest one
            // already committed has nowhere to go.
            Some(newest) if b.taken_at < newest => out.refused.push((
                b.name.clone(),
                format!(
                    "taken {}, before {}, the newest backup already in the store; \
                     backups are added in the order they were taken, so this one \
                     was left out",
                    b.taken_at.format(LEDGER_DATE_FORMAT),
                    newest.format(LEDGER_DATE_FORMAT),
                ),
            )),
            _ => out.ingest.push(b.clone()),
        }
    }
    out
}

/// `Ok(None)` for an entry with no catalog in it, which is not a backup.
fn read_entry(entry: &Entry) -> Result<Option<Backup>, String> {
    let zips: Vec<&PathBuf> = entry.files.iter().filter(|f| is_zip(f)).collect();
    let catalogs: Vec<&PathBuf> = entry.files.iter().filter(|f| is_catalog(f)).collect();
    // A folder that has both is one Lightroom wrote and someone unpacked
    // since. The zip is what Lightroom wrote; the unpacked copy may have
    // been opened, and so changed, after.
    let file = match (zips.as_slice(), catalogs.as_slice()) {
        ([zip], _) => (*zip).clone(),
        ([], [catalog]) => (*catalog).clone(),
        ([], []) => return Ok(None),
        ([], many) | (many, _) => {
            return Err(format!(
                "holds {} catalogs; a backup folder holds one",
                many.len()
            ))
        }
    };
    let taken_at = taken_at(&entry.name).ok_or_else(|| {
        "its name does not start with the date and time Lightroom names a backup by, \
         like `2026-09-27 1650`"
            .to_string()
    })?;
    Ok(Some(Backup {
        name: entry.name.clone(),
        taken_at,
        file,
    }))
}

fn taken_at(name: &str) -> Option<NaiveDateTime> {
    let head = name.get(..NAME_DATE_LEN)?;
    NaiveDateTime::parse_from_str(head, NAME_DATE_FORMAT).ok()
}

/// The commit date for a backup: its folder's time, read in this
/// machine's time zone, which is the one Lightroom named it in.
fn commit_date(taken_at: NaiveDateTime) -> Option<String> {
    let date = chrono::Local
        .from_local_datetime(&taken_at)
        .earliest()
        .map(|t| t.to_rfc3339());
    if date.is_none() {
        tracing::warn!(
            %taken_at,
            "lightroom: a backup's time falls in a daylight-saving gap; committing it dated now"
        );
    }
    date
}

pub fn list_entries(dir: &Path) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    let listing = std::fs::read_dir(dir)
        .with_context(|| format!("read the backups folder {}", dir.display()))?;
    for item in listing {
        let item = item.with_context(|| format!("read {}", dir.display()))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = item.path();
        let files = if path.is_dir() {
            std::fs::read_dir(&path)
                .with_context(|| format!("read {}", path.display()))?
                .map(|f| f.map(|f| f.path()))
                .collect::<std::io::Result<Vec<_>>>()
                .with_context(|| format!("read {}", path.display()))?
        } else {
            vec![path]
        };
        out.push(Entry { name, files });
    }
    Ok(out)
}

#[derive(Debug, Default)]
pub struct BackupsRun {
    pub found: usize,
    pub mirrored: Vec<String>,
    pub problems: Vec<RecordProblem>,
    /// The last backup mirrored.
    pub last: Option<MirrorStats>,
}

impl BackupsRun {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "backups_found={} backups_mirrored={} backups_refused={}",
            self.found,
            self.mirrored.len(),
            self.problems.len()
        );
        if let Some(last) = &self.last {
            s.push(' ');
            s.push_str(&last.summary());
        }
        s
    }
}

/// Commits each backup it mirrors, and leaves the scope record for the
/// caller's closing commit, after it reports [`BackupsRun::problems`].
pub async fn ingest(
    pool: &SqlitePool,
    dir: &Path,
    options: &MirrorOptions,
    progress: &Progress,
    stop: &StopFlag,
    label: &str,
) -> Result<BackupsRun> {
    sqlx::query(LEDGER_DDL)
        .execute(pool)
        .await
        .context("create the backups ledger")?;
    let ledger = read_ledger(pool).await?;
    let plan = plan(list_entries(dir)?, &ledger);
    if plan.found.is_empty() {
        bail!(
            "found no Lightroom backups in {}: expected folders named like `2026-09-27 1650`, \
             each holding a .zip or a .lrcat",
            dir.display()
        );
    }

    let mut options = MirrorOptions {
        sidecar_tables: [options.sidecar_tables.as_slice(), &[LEDGER.to_string()]].concat(),
        ..options.clone()
    };
    let scope = scope_of(&options);
    let recorded = scope_config::load(pool, SCOPE).await?;
    let filters_changed = recorded.as_ref().is_some_and(|r| r != &scope);

    let mut run = BackupsRun {
        found: plan.found.len(),
        problems: plan
            .refused
            .iter()
            .map(|(name, why)| RecordProblem::new(LEDGER, name, why))
            .collect(),
        ..Default::default()
    };

    // The filters shape every commit from here on, but the store's newest
    // commit was made under the old ones. With no new backup to carry
    // them, mirror the newest again so HEAD shows the catalog as the
    // filters now say.
    let mut todo: Vec<(Backup, bool)> = plan.ingest.iter().map(|b| (b.clone(), false)).collect();
    let mut remirror_missing = false;
    if filters_changed && todo.is_empty() {
        let newest = ledger.iter().max_by_key(|(_, t)| **t).map(|(n, _)| n);
        match plan.found.iter().find(|b| Some(&b.name) == newest) {
            Some(b) => todo.push((b.clone(), true)),
            None => {
                remirror_missing = true;
                run.problems.push(RecordProblem::new(
                    LEDGER,
                    newest.map(String::as_str).unwrap_or(""),
                    "the table and column filters changed, and this backup, the newest in \
                     the store, is no longer on disk to mirror again under them; the store \
                     keeps the old filters until the next backup",
                ));
            }
        }
    }

    let mut stopped = false;
    for (backup, again) in todo {
        if stop.requested() {
            stopped = true;
            break;
        }
        let stats = unpack::mirror_file(pool, &backup.file, &options, progress)
            .await
            .with_context(|| format!("mirror backup {}", backup.name))?;
        options.gc = false;
        let file = backup
            .file
            .strip_prefix(dir)
            .unwrap_or(&backup.file)
            .display()
            .to_string();
        sqlx::query(
            "INSERT OR REPLACE INTO lightroom_backups (backup, taken_at, file) VALUES (?, ?, ?)",
        )
        .bind(&backup.name)
        .bind(backup.taken_at.format(LEDGER_DATE_FORMAT).to_string())
        .bind(&file)
        .execute(pool)
        .await
        .context("record the backup in the ledger")?;

        let (what, date) = if again {
            // Dated now: the filters changed now, not when the backup
            // was taken.
            ("backup mirrored again under new filters", None)
        } else {
            ("backup", commit_date(backup.taken_at))
        };
        let msg = format!(
            "download {label}: {what} {}: {}",
            backup.name,
            stats.summary()
        );
        // Not announced as a checkpoint: nothing reads this store while
        // the step runs, so the runner has no use for the version.
        dr::commit_run_dated(pool, &msg, date.as_deref()).await?;
        run.mirrored.push(backup.name.clone());
        run.last = Some(stats);
    }

    // Record the filters once HEAD was mirrored under them. An absent
    // record is taken as a match: it is a store from before the record,
    // or a first run.
    if !stopped && !remirror_missing && recorded.as_ref() != Some(&scope) {
        scope_config::store(pool, SCOPE, &scope).await?;
    }
    Ok(run)
}

async fn read_ledger(pool: &SqlitePool) -> Result<BTreeMap<String, NaiveDateTime>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT backup, taken_at FROM lightroom_backups")
            .fetch_all(pool)
            .await
            .context("read the backups ledger")?;
    rows.into_iter()
        .map(|(name, t)| {
            let t = NaiveDateTime::parse_from_str(&t, LEDGER_DATE_FORMAT)
                .with_context(|| format!("ledger row {name:?} has taken_at {t:?}"))?;
            Ok((name, t))
        })
        .collect()
}

/// The options that decide what a mirrored catalog looks like. Not
/// `snapshot` or `gc`: those change how a run reads and stores, not what
/// lands.
fn scope_of(o: &MirrorOptions) -> serde_json::Value {
    serde_json::json!({
        "include_tables": o.include_tables,
        "exclude_tables": o.exclude_tables,
        "exclude_columns": o.exclude_columns,
        "stable_key_columns": o.stable_key_columns,
        "primary_keys": o.primary_keys,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, LEDGER_DATE_FORMAT).unwrap()
    }

    fn folder(name: &str, files: &[&str]) -> Entry {
        Entry {
            name: name.into(),
            files: files
                .iter()
                .map(|f| PathBuf::from(format!("/B/{name}/{f}")))
                .collect(),
        }
    }

    fn names(bs: &[Backup]) -> Vec<&str> {
        bs.iter().map(|b| b.name.as_str()).collect()
    }

    /// The layout of a real backups folder, which a person has partly
    /// unpacked and annotated by hand.
    fn lightroom_folder() -> Vec<Entry> {
        vec![
            folder("2026-09-02 0911", &["Lightroom Catalog-v13-3.zip"]),
            folder(
                "2018-03-07 2110",
                &[
                    ".DS_Store",
                    "Lightroom Catalog-2-3.lrcat",
                    "Lightroom Catalog-2-3.lrcat-shm",
                    "Lightroom Catalog-2-3.lrcat-wal",
                    "Lightroom Catalog-2-3.lrcat.zip",
                ],
            ),
            folder(
                "2019-12-14 0731 - Before restoring captions",
                &["Lightroom Catalog-2-3.lrcat.zip"],
            ),
            folder("2016-10-01 0856", &["Lightroom Catalog.lrcat"]),
        ]
    }

    #[test]
    fn backups_are_mirrored_in_the_order_they_were_taken() {
        let p = plan(lightroom_folder(), &BTreeMap::new());
        assert_eq!(
            names(&p.ingest),
            [
                "2016-10-01 0856",
                "2018-03-07 2110",
                "2019-12-14 0731 - Before restoring captions",
                "2026-09-02 0911",
            ]
        );
        assert!(p.refused.is_empty(), "{:?}", p.refused);
        assert_eq!(p.ingest[0].taken_at, at("2016-10-01T08:56:00"));
    }

    #[test]
    fn the_zip_lightroom_wrote_beats_an_unpacked_copy_beside_it() {
        let p = plan(lightroom_folder(), &BTreeMap::new());
        let b = p
            .ingest
            .iter()
            .find(|b| b.name == "2018-03-07 2110")
            .unwrap();
        assert!(b.file.ends_with("Lightroom Catalog-2-3.lrcat.zip"));
        let bare = p
            .ingest
            .iter()
            .find(|b| b.name == "2016-10-01 0856")
            .unwrap();
        assert!(bare.file.ends_with("Lightroom Catalog.lrcat"));
    }

    #[test]
    fn backups_the_store_holds_are_skipped() {
        let ledger = BTreeMap::from([
            ("2016-10-01 0856".to_string(), at("2016-10-01T08:56:00")),
            ("2018-03-07 2110".to_string(), at("2018-03-07T21:10:00")),
        ]);
        let p = plan(lightroom_folder(), &ledger);
        assert_eq!(p.found.len(), 4);
        assert_eq!(
            names(&p.ingest),
            [
                "2019-12-14 0731 - Before restoring captions",
                "2026-09-02 0911"
            ]
        );
    }

    /// History is one line, so a backup that turns up after a newer one
    /// was committed cannot be slotted in; it is reported, not appended
    /// on top as though the catalog had gone back in time.
    #[test]
    fn a_backup_older_than_the_newest_held_is_refused() {
        let ledger = BTreeMap::from([("2019-01-28 1032".to_string(), at("2019-01-28T10:32:00"))]);
        let p = plan(lightroom_folder(), &ledger);
        assert_eq!(
            names(&p.ingest),
            [
                "2019-12-14 0731 - Before restoring captions",
                "2026-09-02 0911"
            ]
        );
        let refused: Vec<&str> = p.refused.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(refused, ["2016-10-01 0856", "2018-03-07 2110"]);
    }

    #[test]
    fn a_folder_that_cannot_be_placed_is_refused_and_one_with_no_catalog_ignored() {
        let p = plan(
            vec![
                folder("Old catalogs", &["Lightroom Catalog.lrcat.zip"]),
                folder("2020-01-01 0000", &["a.zip", "b.zip"]),
                folder("2020-02-02 0000", &["notes.txt"]),
                Entry {
                    name: "2021-05-06 0700 Catalog.zip".into(),
                    files: vec!["/B/2021-05-06 0700 Catalog.zip".into()],
                },
            ],
            &BTreeMap::new(),
        );
        assert_eq!(names(&p.ingest), ["2021-05-06 0700 Catalog.zip"]);
        let refused: Vec<&str> = p.refused.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(refused, ["Old catalogs", "2020-01-01 0000"]);
    }

    #[test]
    fn a_commit_date_carries_the_local_offset() {
        let d = commit_date(at("2016-10-01T08:56:00")).unwrap();
        assert!(d.starts_with("2016-10-01T08:56:00"), "{d}");
        assert!(d.len() > "2016-10-01T08:56:00".len(), "no offset in {d}");
    }
}
