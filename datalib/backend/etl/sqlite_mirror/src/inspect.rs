//! What a source SQLite database looks like, read from a [`Snapshot`], so
//! a provider can decide how to mirror it (which columns are a table's
//! key, say) before it does. The decision is the provider's, as a function
//! of this value; reading is the same for every source.

use anyhow::{Context, Result};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Row, SqliteConnection};
use std::str::FromStr;

use crate::mirror::Snapshot;
use crate::plan::quote_ident;

/// A complete UNIQUE index: partial and expression indexes constrain
/// something other than plain columns, so they are never read in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniqueIndex {
    pub name: String,
    pub columns: Vec<String>,
    /// Some row has NULL in one of `columns`. A UNIQUE index lets NULLs
    /// repeat and a key does not. Read only for a table with no declared
    /// key, since that is the only table an index could key; `false` for
    /// the rest.
    pub has_nulls: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub name: String,
    pub columns: Vec<String>,
    /// The declared `PRIMARY KEY` columns, in key order; empty for none.
    pub declared_key: Vec<String>,
    pub unique_indexes: Vec<UniqueIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceSchema {
    pub tables: Vec<TableInfo>,
}

/// Read the schema of `snap`, ordinary tables only.
pub async fn read_schema(snap: &Snapshot) -> Result<SourceSchema> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", snap.path().display()))
        .with_context(|| format!("sqlite uri for {}", snap.path().display()))?
        .read_only(true)
        .create_if_missing(false);
    let mut conn = SqliteConnection::connect_with(&opts)
        .await
        .with_context(|| format!("open {} read-only", snap.path().display()))?;
    let schema = read(&mut conn).await;
    let _ = conn.close().await;
    schema
}

async fn read(conn: &mut SqliteConnection) -> Result<SourceSchema> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' \
         AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
    )
    .fetch_all(&mut *conn)
    .await
    .context("list the source's tables")?;
    let mut tables = Vec::new();
    for name in names {
        let cols = sqlx::query("SELECT name, pk FROM pragma_table_info(?)")
            .bind(&name)
            .fetch_all(&mut *conn)
            .await
            .with_context(|| format!("table_info({name})"))?;
        let columns = cols.iter().map(|r| r.get::<String, _>("name")).collect();
        let mut keyed: Vec<(i64, String)> = cols
            .iter()
            .map(|r| (r.get::<i64, _>("pk"), r.get::<String, _>("name")))
            .filter(|(pk, _)| *pk > 0)
            .collect();
        keyed.sort();
        let declared_key: Vec<String> = keyed.into_iter().map(|(_, c)| c).collect();
        let unique_indexes = if declared_key.is_empty() {
            read_unique_indexes(conn, &name).await?
        } else {
            Vec::new()
        };
        tables.push(TableInfo {
            name,
            columns,
            declared_key,
            unique_indexes,
        });
    }
    Ok(SourceSchema { tables })
}

async fn read_unique_indexes(conn: &mut SqliteConnection, table: &str) -> Result<Vec<UniqueIndex>> {
    let listed = sqlx::query("SELECT name, \"unique\", partial FROM pragma_index_list(?)")
        .bind(table)
        .fetch_all(&mut *conn)
        .await
        .with_context(|| format!("index_list({table})"))?;
    let mut out = Vec::new();
    for ix in &listed {
        if ix.get::<i64, _>("unique") == 0 || ix.get::<i64, _>("partial") != 0 {
            continue;
        }
        let name: String = ix.get("name");
        let parts = sqlx::query("SELECT name FROM pragma_index_info(?)")
            .bind(&name)
            .fetch_all(&mut *conn)
            .await
            .with_context(|| format!("index_info({name})"))?;
        // A NULL column name is an expression, not a column.
        let columns: Vec<String> = parts
            .iter()
            .filter_map(|r| r.get::<Option<String>, _>("name"))
            .collect();
        if columns.is_empty() || columns.len() != parts.len() {
            continue;
        }
        let has_nulls = has_nulls(conn, table, &columns).await?;
        out.push(UniqueIndex {
            name,
            columns,
            has_nulls,
        });
    }
    Ok(out)
}

async fn has_nulls(conn: &mut SqliteConnection, table: &str, columns: &[String]) -> Result<bool> {
    let any_null = columns
        .iter()
        .map(|c| format!("{} IS NULL", quote_ident(c)))
        .collect::<Vec<_>>()
        .join(" OR ");
    // Audited: names come out of the source's own schema through
    // `quote_ident`.
    let sql = format!(
        "SELECT EXISTS (SELECT 1 FROM {} WHERE {any_null})",
        quote_ident(table)
    );
    let found: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_one(&mut *conn)
        .await
        .with_context(|| format!("check {table}({}) for NULLs", columns.join(", ")))?;
    Ok(found != 0)
}
