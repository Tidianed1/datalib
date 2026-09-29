//! Reading a raw store for its summary: opening it pinned, and reading
//! a mirrored application database whose tables may not all be there.

use std::future::Future;
use std::path::Path;

use anyhow::{Context, Result};
use datalib_etl::doltlite_raw::Reader;
use datalib_etl_render::inputs::{Input, RawRange};
use sqlx::sqlite::SqliteRow;

/// Read the store at `raw_path` at the driver's pin (else HEAD) with
/// `read`, on the current tokio runtime. `None` when nothing is
/// committed there yet: a store with no commit is unreadable, not
/// empty, and the page must not claim it holds nothing.
pub fn pinned<T>(
    raw_path: &Path,
    range: RawRange<'_>,
    read: impl AsyncFnOnce(&Reader) -> Result<T>,
) -> Result<Option<T>> {
    let db_path = datalib_etl::doltlite_raw::db_path_for(raw_path);
    anyhow::ensure!(
        db_path.exists(),
        "raw store not found at {} — run the download step first",
        db_path.display()
    );
    // The render phase is driven by `futures`' executor, which enters no
    // tokio context of its own; every provider's parse runs its reads
    // this way.
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let Some(reader) = datalib_etl::doltlite_raw::open_reader(&db_path, range.pin)
                .await
                .with_context(|| format!("open {} for render", db_path.display()))?
            else {
                return Ok(None);
            };
            let read = read(&reader).await;
            reader.close().await;
            read.map(Some)
        })
    })
}

/// `query` against a table a mirrored store may lack — Lightroom's EXIF
/// tables are absent from a trimmed catalog, and a person can exclude
/// any table — as `None` rather than an error. Anything else that goes
/// wrong still fails.
pub async fn if_present<T, F>(table: &str, query: F) -> Result<Option<T>>
where
    F: Future<Output = std::result::Result<T, sqlx::Error>>,
{
    match query.await {
        Ok(v) => Ok(Some(v)),
        Err(e) if datalib_etl::pin::is_missing_table(&e, table) => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {table}")),
    }
}

/// A store mirrored table for table from an application's SQLite file
/// (`etl/sqlite_mirror`), read at one commit. Its tables keep the
/// application's names — `Adobe_images`, `ZASSET` — which the pinned
/// views do not cover, so every read names `dolt_at_<table>` itself.
pub struct Mirror<'a> {
    reader: &'a Reader,
    found: Vec<&'static str>,
}

impl<'a> Mirror<'a> {
    pub fn new(reader: &'a Reader) -> Self {
        Self {
            reader,
            found: Vec::new(),
        }
    }

    /// The rows of `sql(from)`, where `from` reads `table` at the pin;
    /// `None` when this store has no such table.
    pub async fn rows(
        &mut self,
        table: &'static str,
        sql: impl FnOnce(&str) -> String,
    ) -> Result<Option<Vec<SqliteRow>>> {
        let from = self.reader.pin().table(table);
        // Audited: `table` is a literal at every call site, the hash in
        // `from` is one `Pin::at` validated, and `sql` is a literal
        // template around it.
        let query = sqlx::query(sqlx::AssertSqlSafe(sql(&from))).fetch_all(self.reader.pool());
        let rows = if_present(table, query).await?;
        if rows.is_some() && !self.found.contains(&table) {
            self.found.push(table);
        }
        Ok(rows)
    }

    /// Every table a read found, whole. A table the store lacks is left
    /// out: the driver's reverse lookup diffs each input, and a table
    /// with no diff to read would stop it narrowing at all. The
    /// applications these mirror create their tables up front, so the
    /// set does not grow under a page.
    pub fn inputs(&self) -> Vec<Input> {
        self.found.iter().map(|t| Input::whole_table(*t)).collect()
    }
}
