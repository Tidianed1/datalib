//! The grid's terms file, `unified_index/grid_index/terms.sqlite`: every
//! term each `grid_rows` row answers to (`datalib_schema::terms`), kept in
//! step with the grid index after each `grid_index` pass. It is a pure
//! function of `grid_rows` at one commit: the file records the commit it
//! reflects and moves to the next by `dolt_diff`, or is built whole when it
//! cannot. Plain SQLite, so it keeps no history of itself.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use datalib_schema::terms::{
    terms_of, TermSource, META_GRID_COMMIT, META_SHAPE, TERMS_DDL, TERMS_SHAPE,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::SqliteConnection;

/// What a sync has to do, from what the file records and the grid's head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// The file already reflects the head.
    Current,
    /// Build the file whole: it records no commit, or another shape.
    Whole,
    /// Apply the rows that changed since this commit.
    Since(String),
}

pub fn plan(recorded_commit: Option<&str>, recorded_shape: Option<&str>, head: &str) -> Plan {
    match (recorded_commit, recorded_shape) {
        (Some(commit), Some(TERMS_SHAPE)) if commit == head => Plan::Current,
        (Some(commit), Some(TERMS_SHAPE)) => Plan::Since(commit.to_string()),
        _ => Plan::Whole,
    }
}

/// What one sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Synced {
    /// `current`, `whole` or `since`.
    pub plan: &'static str,
    /// Grid rows whose terms were written again (or removed).
    pub rows: usize,
    /// Terms written.
    pub terms: usize,
}

/// Bring the terms file at `terms_path` to the grid index's head. A store
/// without doltlite has no head, and no terms.
pub async fn sync(grid: &SqlitePool, terms_path: &Path) -> Result<Synced> {
    let Some(head) = datalib_etl::doltlite_raw::head_commit(grid).await? else {
        return Ok(Synced::default());
    };
    let terms = open_terms(terms_path).await?;
    let result = sync_open(grid, &terms, &head).await;
    // On the error path too: dropping the pool only schedules the close.
    terms.close().await;
    result
}

async fn sync_open(grid: &SqlitePool, terms: &SqlitePool, head: &str) -> Result<Synced> {
    let mut conn = terms.acquire().await.context("acquire the terms file")?;
    let shape = meta(&mut conn, META_SHAPE).await?;
    if shape.as_deref().is_some_and(|s| s != TERMS_SHAPE) {
        tracing::info!(
            was = shape.as_deref().unwrap_or(""),
            now = TERMS_SHAPE,
            "the terms file is in another shape; building it again"
        );
        for table in ["terms_fts", "terms", "terms_meta"] {
            // Audited: the table names are literals.
            sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE IF EXISTS {table}")))
                .execute(&mut *conn)
                .await?;
        }
        create(&mut conn).await?;
    }
    let recorded = meta(&mut conn, META_GRID_COMMIT).await?;
    let shape = meta(&mut conn, META_SHAPE).await?;
    let plan = plan(recorded.as_deref(), shape.as_deref(), head);
    match plan {
        Plan::Current => Ok(Synced {
            plan: "current",
            ..Synced::default()
        }),
        Plan::Since(from) => {
            match datalib_etl::doltlite_raw::changed_keys(grid, "grid_rows", &from, head).await {
                Ok(changed) => {
                    let uuids: Vec<String> = changed.into_iter().map(|k| k.key).collect();
                    let rows = rows_by_uuid(grid, &uuids).await?;
                    let terms = write(&mut conn, Some(&uuids), &rows, head).await?;
                    Ok(Synced {
                        plan: "since",
                        rows: uuids.len(),
                        terms,
                    })
                }
                // A commit the grid no longer has, say after a rebuild:
                // the whole file is the one answer still right.
                Err(e) => {
                    tracing::warn!(
                        from = %from,
                        error = %format!("{e:#}"),
                        "could not diff the grid index from the commit the terms \
                         reflect; building the terms file whole"
                    );
                    whole(grid, &mut conn, head).await
                }
            }
        }
        Plan::Whole => whole(grid, &mut conn, head).await,
    }
}

async fn whole(grid: &SqlitePool, conn: &mut SqliteConnection, head: &str) -> Result<Synced> {
    // Audited: the column list is a literal.
    let rows: Vec<TermSource> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM grid_rows",
        TermSource::COLUMNS
    )))
    .fetch_all(grid)
    .await
    .context("read grid_rows for the terms")?;
    let terms = write(conn, None, &rows, head).await?;
    Ok(Synced {
        plan: "whole",
        rows: rows.len(),
        terms,
    })
}

/// The rows behind `uuids` that the grid still holds; a removed row has
/// none, and only loses its terms.
async fn rows_by_uuid(grid: &SqlitePool, uuids: &[String]) -> Result<Vec<TermSource>> {
    let mut out = Vec::new();
    for chunk in uuids.chunks(CHUNK) {
        // Audited: a placeholder per value, every value bound.
        let sql = format!(
            "SELECT {} FROM grid_rows WHERE uuid IN ({})",
            TermSource::COLUMNS,
            placeholders(chunk.len(), 1)
        );
        let mut q = sqlx::query_as::<_, TermSource>(sqlx::AssertSqlSafe(sql));
        for uuid in chunk {
            q = q.bind(uuid);
        }
        out.extend(q.fetch_all(grid).await.context("read changed grid_rows")?);
    }
    Ok(out)
}

/// One transaction: drop the terms of `replace` (every term, for `None`),
/// write `rows`' terms, and record `head`. Returns the terms written.
async fn write(
    conn: &mut SqliteConnection,
    replace: Option<&[String]>,
    rows: &[TermSource],
    head: &str,
) -> Result<usize> {
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
    let written = async {
        match replace {
            None => {
                sqlx::query("INSERT INTO terms_fts (terms_fts) VALUES ('delete-all')")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("DELETE FROM terms").execute(&mut *conn).await?;
            }
            Some(uuids) => {
                for chunk in uuids.chunks(CHUNK) {
                    let p = placeholders(chunk.len(), 1);
                    for sql in [
                        format!(
                            "DELETE FROM terms_fts WHERE rowid IN \
                             (SELECT term_id FROM terms WHERE uuid IN ({p}))"
                        ),
                        format!("DELETE FROM terms WHERE uuid IN ({p})"),
                    ] {
                        // Audited: a placeholder per value, every value bound.
                        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
                        for uuid in chunk {
                            q = q.bind(uuid);
                        }
                        q.execute(&mut *conn).await?;
                    }
                }
            }
        }
        // New rows take keys above every one left, so the index picks up
        // exactly these.
        let floor: i64 = sqlx::query_scalar("SELECT coalesce(max(term_id), 0) FROM terms")
            .fetch_one(&mut *conn)
            .await?;
        let terms: Vec<(&TermSource, datalib_schema::terms::Term)> = rows
            .iter()
            .flat_map(|row| terms_of(row).into_iter().map(move |t| (row, t)))
            .collect();
        for chunk in terms.chunks(CHUNK) {
            // Audited: four placeholders per term, every value bound.
            let sql = format!(
                "INSERT INTO terms (uuid, kind, value, touched_at_utc) VALUES {}",
                placeholders(chunk.len(), 4)
            );
            let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
            for (row, term) in chunk {
                q = q
                    .bind(&row.uuid)
                    .bind(term.kind.as_str())
                    .bind(&term.value)
                    .bind(&row.touched_at_utc);
            }
            q.execute(&mut *conn).await?;
        }
        sqlx::query(
            "INSERT INTO terms_fts (rowid, value) SELECT term_id, value FROM terms \
             WHERE term_id > ?",
        )
        .bind(floor)
        .execute(&mut *conn)
        .await?;
        for (key, value) in [(META_GRID_COMMIT, head), (META_SHAPE, TERMS_SHAPE)] {
            sqlx::query(
                "INSERT INTO terms_meta (key, value) VALUES (?, ?) \
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            )
            .bind(key)
            .bind(value)
            .execute(&mut *conn)
            .await?;
        }
        anyhow::Ok(terms.len())
    }
    .await;
    match written {
        Ok(n) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            Ok(n)
        }
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            Err(e).context("write the terms file")
        }
    }
}

/// Values per statement, under SQLite's bound-parameter limit at four a
/// term.
const CHUNK: usize = 500;

fn placeholders(rows: usize, per_row: usize) -> String {
    let one = if per_row == 1 {
        "?".to_string()
    } else {
        format!("({})", vec!["?"; per_row].join(", "))
    };
    vec![one; rows].join(", ")
}

async fn open_terms(path: &Path) -> Result<SqlitePool> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let opts = SqliteConnectOptions::new()
        .filename(format!("file:{}?doltlite_engine=sqlite", path.display()))
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(30));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(opts)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    let mut conn = pool.acquire().await?;
    create(&mut conn).await?;
    drop(conn);
    Ok(pool)
}

async fn create(conn: &mut SqliteConnection) -> Result<()> {
    for ddl in TERMS_DDL {
        sqlx::query(*ddl)
            .execute(&mut *conn)
            .await
            .with_context(|| format!("create: {ddl}"))?;
    }
    Ok(())
}

async fn meta(conn: &mut SqliteConnection, key: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT value FROM terms_meta WHERE key = ?")
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid_index::{apply_one, delete_markdown, open_index, RenderedMarkdown, WriteLock};
    use datalib_schema::grid_rows::GridRow;
    use datalib_schema::providers::Provider;

    /// A document of one row: its uuid, its author's handle, its title.
    fn doc(root: &Path, uuid: &str, handle: &str, title: &str) -> RenderedMarkdown {
        let row = GridRow::builder()
            .uuid(uuid)
            .provider(Provider::Claude)
            .kind("Chat")
            .source_label("Claude")
            .is_document(true)
            .created_at(Some("2026-01-01T09:00:00+00:00".to_string()))
            .conversation_uuid(uuid)
            .conversation_name(Some(title.to_string()))
            .author_handle(Some(handle.to_string()))
            .entire_chat(format!("/chat/{uuid}"))
            .body("")
            .markdown_uuid(Some(uuid.to_string()))
            .build()
            .unwrap();
        RenderedMarkdown {
            markdown_uuid: uuid.to_string(),
            source_id: "enterprise".into(),
            upstream_cursor: None,
            bucket_key: None,
            md_path: root.join(format!("enterprise/{uuid}.md")),
            render_version: 1,
            rows: vec![row],
            sections: Vec::new(),
            edges: Vec::new(),
            contacts: Vec::new(),
            problems: Vec::new(),
        }
    }

    struct Grid {
        _dir: tempfile::TempDir,
        root: std::path::PathBuf,
        pool: SqlitePool,
        terms: std::path::PathBuf,
    }

    impl Grid {
        async fn new() -> Grid {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().to_path_buf();
            let pool = open_index(&datalib_core::layout::grid_index_db(&root))
                .await
                .unwrap();
            let terms = datalib_core::layout::grid_terms_db(&root);
            Grid {
                _dir: dir,
                root,
                pool,
                terms,
            }
        }

        async fn seal(&self, put: &[RenderedMarkdown], remove: &[&str]) {
            let lock = WriteLock::new(self.pool.clone());
            for md in put {
                apply_one(&lock, &self.root, md).await.unwrap();
            }
            for md in remove {
                delete_markdown(&lock, md).await.unwrap();
            }
            datalib_etl::doltlite_raw::commit_run(&self.pool, "pass")
                .await
                .unwrap();
        }

        /// The `(uuid, kind)` of every term matching `value` exactly.
        async fn matching(&self, value: &str) -> Vec<(String, String)> {
            let terms = open_terms(&self.terms).await.unwrap();
            let mut hits: Vec<(String, String)> = sqlx::query_as(
                "SELECT t.uuid, t.kind FROM terms_fts JOIN terms t ON t.term_id = terms_fts.rowid \
                 WHERE terms_fts MATCH ?",
            )
            .bind(format!("\"{value}\""))
            .fetch_all(&terms)
            .await
            .unwrap();
            terms.close().await;
            hits.sort();
            hits
        }

        async fn set_meta(&self, key: &str, value: &str) {
            let terms = open_terms(&self.terms).await.unwrap();
            sqlx::query("UPDATE terms_meta SET value = ? WHERE key = ?")
                .bind(value)
                .bind(key)
                .execute(&terms)
                .await
                .unwrap();
            terms.close().await;
        }
    }

    fn hit(uuid: &str, kind: &str) -> (String, String) {
        (uuid.to_string(), kind.to_string())
    }

    #[tokio::test]
    async fn the_first_sync_builds_the_file_whole_and_the_next_finds_it_current() {
        let g = Grid::new().await;
        let (a, b) = (
            doc(&g.root, "c-a", "email:ann@example.com", "Away team"),
            doc(&g.root, "c-b", "email:bo@example.com", "Bridge"),
        );
        g.seal(&[a, b], &[]).await;

        let first = sync(&g.pool, &g.terms).await.unwrap();
        assert_eq!((first.plan, first.rows), ("whole", 2));
        assert_eq!(
            g.matching("email:ann@example.com").await,
            [hit("c-a", "from")]
        );
        assert_eq!(g.matching("c-b").await, [hit("c-b", "id")]);
        assert_eq!(sync(&g.pool, &g.terms).await.unwrap().plan, "current");
    }

    /// The point of recording the commit: a pass rewrites the terms of the
    /// rows that changed, and no others.
    #[tokio::test]
    async fn a_later_sync_rewrites_only_the_rows_that_changed() {
        let g = Grid::new().await;
        g.seal(
            &[
                doc(&g.root, "c-a", "email:ann@example.com", "Away team"),
                doc(&g.root, "c-b", "email:bo@example.com", "Bridge"),
                doc(&g.root, "c-k", "email:kit@example.com", "Kept"),
            ],
            &[],
        )
        .await;
        sync(&g.pool, &g.terms).await.unwrap();

        g.seal(
            &[
                doc(&g.root, "c-a", "email:ann@example.org", "Away team"),
                doc(&g.root, "c-c", "email:cy@example.com", "Cargo"),
            ],
            &["c-b"],
        )
        .await;
        let next = sync(&g.pool, &g.terms).await.unwrap();
        assert_eq!((next.plan, next.rows), ("since", 3));
        assert_eq!(g.matching("email:ann@example.com").await, Vec::new());
        assert_eq!(
            g.matching("email:ann@example.org").await,
            [hit("c-a", "from")]
        );
        assert_eq!(g.matching("c-b").await, Vec::new());
        assert_eq!(
            g.matching("email:cy@example.com").await,
            [hit("c-c", "from")]
        );
        assert_eq!(
            g.matching("email:kit@example.com").await,
            [hit("c-k", "from")]
        );
    }

    /// A commit the grid no longer has, or a file in another shape, cannot
    /// be moved forward; it is built again.
    #[tokio::test]
    async fn an_unknown_commit_or_another_shape_builds_the_file_whole() {
        let g = Grid::new().await;
        g.seal(&[doc(&g.root, "c-a", "email:ann@example.com", "Away")], &[])
            .await;
        sync(&g.pool, &g.terms).await.unwrap();

        g.set_meta(META_GRID_COMMIT, &"0".repeat(40)).await;
        let again = sync(&g.pool, &g.terms).await.unwrap();
        assert_eq!((again.plan, again.rows), ("whole", 1));

        g.set_meta(META_SHAPE, "0").await;
        assert_eq!(sync(&g.pool, &g.terms).await.unwrap().plan, "whole");
        assert_eq!(
            g.matching("email:ann@example.com").await,
            [hit("c-a", "from")]
        );
    }

    #[test]
    fn the_plan_follows_what_the_file_records() {
        assert_eq!(plan(Some("h"), Some(TERMS_SHAPE), "h"), Plan::Current);
        assert_eq!(
            plan(Some("a"), Some(TERMS_SHAPE), "h"),
            Plan::Since("a".into())
        );
        assert_eq!(plan(None, None, "h"), Plan::Whole);
        assert_eq!(plan(Some("h"), Some("0"), "h"), Plan::Whole);
    }
}
