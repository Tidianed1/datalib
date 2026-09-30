//! A folder of Lightroom backups, built from the TNG catalog, mirrored
//! into one store: each backup a commit, oldest first, dated when it was
//! taken.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use sqlx::sqlite::SqlitePool;

use datalib_etl::doltlite_raw as dr;
use datalib_etl::progress::Progress;
use datalib_etl::stop::StopFlag;
use datalib_etl_lightroom::ingest::backups::{self, BackupsRun};
use datalib_etl_lightroom::ingest::{mirror, MirrorOptions};

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Backups")).unwrap();
        Self { dir }
    }

    fn backups(&self) -> PathBuf {
        self.dir.path().join("Backups")
    }

    fn store(&self) -> PathBuf {
        self.dir.path().join("entities.doltlite_db")
    }

    /// Write a backup into `Backups/<folder>/`: the TNG catalog with
    /// `edits` applied, zipped as Lightroom does when `zip` names the
    /// archive, else as a bare `.lrcat`.
    async fn backup(&self, folder: &str, catalog: &str, zip: Option<&str>, edits: &[&str]) {
        let scratch = tempfile::tempdir().unwrap();
        let lrcat = scratch.path().join(catalog);
        std::fs::copy(fixture_catalog(), &lrcat).unwrap();
        let mut perms = std::fs::metadata(&lrcat).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&lrcat, perms).unwrap();
        let pool = mirror::open_sqlite(&lrcat, false).await.unwrap();
        for e in edits {
            // Test: `edits` are literal catalog edits written by the test.
            sqlx::query(sqlx::AssertSqlSafe(*e))
                .execute(&pool)
                .await
                .unwrap();
        }
        pool.close().await;

        let dest = self.backups().join(folder);
        std::fs::create_dir_all(&dest).unwrap();
        match zip {
            Some(zip_name) => {
                let mut w =
                    zip::ZipWriter::new(std::fs::File::create(dest.join(zip_name)).unwrap());
                let opts = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated);
                w.start_file(catalog, opts).unwrap();
                w.write_all(&std::fs::read(&lrcat).unwrap()).unwrap();
                w.finish().unwrap();
            }
            None => {
                std::fs::copy(&lrcat, dest.join(catalog)).unwrap();
            }
        }
    }

    /// One sync: mirror what is new, report, and make the run's last
    /// commit, as the processor does.
    async fn sync(&self, options: &MirrorOptions) -> Result<BackupsRun> {
        let pool = mirror::open_mirror(&self.store()).await?;
        let run = backups::ingest(
            &pool,
            &self.backups(),
            options,
            &Progress::noop(),
            &StopFlag::default(),
            "lightroom",
        )
        .await;
        if let Ok(run) = &run {
            datalib_etl::download_problems::report_records(&pool, &run.problems).await;
            dr::commit_run(&pool, &format!("download lightroom: {}", run.summary())).await?;
        }
        pool.close().await;
        run
    }

    async fn read(&self) -> SqlitePool {
        mirror::open_sqlite(&self.store(), false).await.unwrap()
    }
}

fn fixture_catalog() -> PathBuf {
    PathBuf::from(
        std::env::var("SQLITE_MIRROR_TNG_CATALOG")
            .expect("SQLITE_MIRROR_TNG_CATALOG must point at the generated .lrcat fixture"),
    )
}

fn options() -> MirrorOptions {
    MirrorOptions {
        source_path: PathBuf::new(),
        snapshot: true,
        include_tables: vec!["*".into()],
        exclude_tables: Vec::new(),
        exclude_columns: Vec::new(),
        stable_key_columns: vec!["id_global".into()],
        primary_keys: Default::default(),
        gc: false,
        sidecar_tables: Vec::new(),
    }
}

/// `(message, date)` of every commit, oldest first, without the store's
/// initialization commit.
async fn log(pool: &SqlitePool) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT message, date FROM dolt_log WHERE message != 'Initialize data repository'",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    rows.reverse();
    rows
}

async fn head(pool: &SqlitePool) -> String {
    sqlx::query_scalar("SELECT commit_hash FROM dolt_log LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn rating_of_picard(pool: &SqlitePool) -> Option<i64> {
    sqlx::query_scalar("SELECT rating FROM Adobe_images WHERE id_global = 'IMAGE-0101-PICARD'")
        .fetch_one(pool)
        .await
        .unwrap()
}

const RERATE: &str = "UPDATE Adobe_images SET rating = 1 WHERE id_global = 'IMAGE-0101-PICARD'";
const KEYWORD: &str = "INSERT INTO AgLibraryKeyword (id_local, id_global, lc_name, name) \
                       VALUES (9001, 'KEYWORD-9001-HOLODECK', 'holodeck', 'Holodeck')";

/// The folder's backups land oldest first, one commit each, dated at the
/// time in the folder's name — read in local time, stored as UTC — so
/// `dolt_log` and `dolt_history_` read as the catalog's own history.
#[tokio::test]
async fn each_backup_is_a_commit_dated_when_it_was_taken() -> Result<()> {
    let f = Fixture::new();
    // Written newest first, so directory order is not what orders them.
    f.backup(
        "2023-01-02 0800",
        "TngCatalog-v13.lrcat",
        Some("TngCatalog-v13.zip"),
        &[RERATE, KEYWORD],
    )
    .await;
    f.backup(
        "2022-06-15 1400 - before keywords",
        "TngCatalog-2.lrcat",
        None,
        &[RERATE],
    )
    .await;
    f.backup(
        "2021-03-01 0900",
        "TngCatalog.lrcat",
        Some("TngCatalog.lrcat.zip"),
        &[],
    )
    .await;

    let run = f.sync(&options()).await?;
    assert_eq!(
        run.mirrored,
        [
            "2021-03-01 0900",
            "2022-06-15 1400 - before keywords",
            "2023-01-02 0800"
        ]
    );
    assert!(run.problems.is_empty(), "{:?}", run.problems);

    let pool = f.read().await;
    let log = log(&pool).await;
    let backups: Vec<&(String, String)> = log
        .iter()
        .filter(|(m, _)| m.contains(": backup "))
        .collect();
    assert_eq!(backups.len(), 3, "{log:?}");
    // TZ=UTC+7 in the BUILD file: seven hours behind UTC.
    let dates: Vec<&str> = backups.iter().map(|(_, d)| d.as_str()).collect();
    assert_eq!(
        dates,
        [
            "2021-03-01 16:00:00",
            "2022-06-15 21:00:00",
            "2023-01-02 15:00:00"
        ]
    );
    assert!(backups[1]
        .0
        .contains("backup 2022-06-15 1400 - before keywords"));

    let history: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT commit_date, rating FROM dolt_history_Adobe_images \
          WHERE id_global = 'IMAGE-0101-PICARD' ORDER BY commit_date",
    )
    .fetch_all(&pool)
    .await?;
    let ratings: Vec<Option<i64>> = history.iter().map(|(_, r)| *r).collect();
    assert_eq!(ratings.first(), Some(&Some(5)), "{history:?}");
    assert_eq!(ratings.last(), Some(&Some(1)), "{history:?}");

    let keywords: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM AgLibraryKeyword WHERE id_global = 'KEYWORD-9001-HOLODECK'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(keywords, 1, "HEAD is the newest backup");

    let ledger: Vec<(String, String, String)> =
        sqlx::query_as("SELECT backup, taken_at, file FROM lightroom_backups ORDER BY taken_at")
            .fetch_all(&pool)
            .await?;
    assert_eq!(
        ledger,
        [
            (
                "2021-03-01 0900".into(),
                "2021-03-01T09:00:00".into(),
                "2021-03-01 0900/TngCatalog.lrcat.zip".into()
            ),
            (
                "2022-06-15 1400 - before keywords".into(),
                "2022-06-15T14:00:00".into(),
                "2022-06-15 1400 - before keywords/TngCatalog-2.lrcat".into()
            ),
            (
                "2023-01-02 0800".into(),
                "2023-01-02T08:00:00".into(),
                "2023-01-02 0800/TngCatalog-v13.zip".into()
            ),
        ]
    );
    pool.close().await;
    Ok(())
}

/// A second sync over the same folder has nothing to do, and commits
/// nothing.
#[tokio::test]
async fn a_folder_with_nothing_new_commits_nothing() -> Result<()> {
    let f = Fixture::new();
    f.backup(
        "2021-03-01 0900",
        "TngCatalog.lrcat",
        Some("TngCatalog.lrcat.zip"),
        &[],
    )
    .await;
    f.sync(&options()).await?;
    let before = head(&f.read().await).await;

    let run = f.sync(&options()).await?;
    assert!(run.mirrored.is_empty());
    assert_eq!(run.found, 1);
    assert_eq!(head(&f.read().await).await, before);
    Ok(())
}

/// A newer backup is appended; one older than the newest committed is a
/// problem row, and history is left alone.
#[tokio::test]
async fn a_new_backup_is_appended_and_an_older_one_refused() -> Result<()> {
    let f = Fixture::new();
    f.backup(
        "2022-06-15 1400",
        "TngCatalog.lrcat",
        Some("TngCatalog.zip"),
        &[],
    )
    .await;
    f.sync(&options()).await?;

    f.backup(
        "2023-01-02 0800",
        "TngCatalog.lrcat",
        Some("TngCatalog.zip"),
        &[RERATE],
    )
    .await;
    f.backup(
        "2020-01-01 0000",
        "TngCatalog.lrcat",
        Some("TngCatalog.zip"),
        &[KEYWORD],
    )
    .await;
    let run = f.sync(&options()).await?;
    assert_eq!(run.mirrored, ["2023-01-02 0800"]);
    let refused: Vec<&str> = run.problems.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(refused, ["2020-01-01 0000"]);

    let pool = f.read().await;
    let problems: Vec<String> =
        sqlx::query_scalar("SELECT scope_key FROM problems WHERE scope_key LIKE 'record:%'")
            .fetch_all(&pool)
            .await?;
    assert_eq!(problems, ["record:lightroom_backups:2020-01-01 0000"]);
    let keywords: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM AgLibraryKeyword WHERE id_global = 'KEYWORD-9001-HOLODECK'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(keywords, 0, "the refused backup never reached the mirror");
    assert_eq!(rating_of_picard(&pool).await, Some(1));
    pool.close().await;
    Ok(())
}

/// A filter changed with no new backup to carry it: the newest backup is
/// mirrored again, so HEAD shows the catalog as the filters now say.
#[tokio::test]
async fn a_changed_filter_mirrors_the_newest_backup_again() -> Result<()> {
    let f = Fixture::new();
    f.backup(
        "2021-03-01 0900",
        "TngCatalog.lrcat",
        Some("TngCatalog.zip"),
        &[],
    )
    .await;
    f.sync(&options()).await?;
    let pool = f.read().await;
    assert!(table_exists(&pool, "AgOzSpaceIds").await);
    pool.close().await;

    let narrowed = MirrorOptions {
        exclude_tables: vec!["AgOz*".into()],
        ..options()
    };
    let run = f.sync(&narrowed).await?;
    assert_eq!(run.mirrored, ["2021-03-01 0900"]);
    let pool = f.read().await;
    assert!(!table_exists(&pool, "AgOzSpaceIds").await);
    assert!(log(&pool)
        .await
        .iter()
        .any(|(m, _)| m.contains("mirrored again under new filters")));
    let before = head(&pool).await;
    pool.close().await;

    let run = f.sync(&narrowed).await?;
    assert!(run.mirrored.is_empty(), "the new filters are recorded now");
    assert_eq!(head(&f.read().await).await, before);
    Ok(())
}

#[tokio::test]
async fn a_folder_with_no_backups_fails_the_run() {
    let f = Fixture::new();
    std::fs::create_dir(f.backups().join("not a backup")).unwrap();
    let err = f.sync(&options()).await.unwrap_err().to_string();
    assert!(err.contains("found no Lightroom backups"), "{err}");
}

async fn table_exists(pool: &SqlitePool, name: &str) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
        == 1
}
