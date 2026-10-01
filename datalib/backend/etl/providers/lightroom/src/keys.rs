//! The keys of the Lightroom catalog tables that have no PRIMARY KEY.
//! Lightroom declares those tables' keys as a UNIQUE index named
//! `index_<Table>_primaryKey`; this reads them out of the catalog and
//! hands the mirror engine the key columns per table, as `primary_keys`.

use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Row, SqliteConnection};

use datalib_etl_lightroom_config::glob_match;
use datalib_etl_sqlite_mirror::plan::quote_ident;

use crate::ingest::MirrorOptions;

/// A complete UNIQUE index: partial and expression indexes constrain
/// something other than plain columns, so they are never read in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniqueIndex {
    pub name: String,
    pub columns: Vec<String>,
}

/// One catalog table as the key rule sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableShape {
    pub name: String,
    pub columns: Vec<String>,
    pub has_declared_key: bool,
    pub unique_indexes: Vec<UniqueIndex>,
}

/// The columns of the table's only UNIQUE index, or of the one named
/// `…primaryKey` among several. A UNIQUE index is a constraint, not an
/// identity, so with several and no such name there is no answer.
pub fn key_of(indexes: &[UniqueIndex]) -> Option<&[String]> {
    if let [only] = indexes {
        return Some(&only.columns);
    }
    let mut named = indexes
        .iter()
        .filter(|i| i.name.to_ascii_lowercase().contains("primarykey"));
    match (named.next(), named.next()) {
        (Some(one), None) => Some(&one.columns),
        _ => None,
    }
}

/// The key for each table that would otherwise be keyless. A table with
/// a declared key, or with a column the engine keys on first
/// (`id_global`), is left to the engine; a key naming a column the
/// filters drop is left out, since the engine refuses it.
pub fn keys_for(
    tables: &[TableShape],
    stable_key_columns: &[String],
    exclude_columns: &[String],
) -> BTreeMap<String, Vec<String>> {
    tables
        .iter()
        .filter(|t| !t.has_declared_key)
        .filter(|t| !t.columns.iter().any(|c| stable_key_columns.contains(c)))
        .filter_map(|t| {
            let key = key_of(&t.unique_indexes)?;
            let dropped = key.iter().any(|c| {
                exclude_columns
                    .iter()
                    .any(|p| glob_match(p, &format!("{}.{c}", t.name)))
            });
            (!dropped).then(|| (t.name.clone(), key.to_vec()))
        })
        .collect()
}

/// `options` with the catalog's keys added to `primary_keys`. An entry
/// the user already set for a table wins.
///
/// The catalog is read as it is now and the engine snapshots it a moment
/// later, so a live catalog that gains a NULL in a key column in between
/// fails that run, loudly; a backup is a copy and cannot.
pub async fn with_catalog_keys(options: &MirrorOptions) -> Result<MirrorOptions> {
    let mut conn = open_read_only(&options.source_path).await?;
    let shapes = read_shapes(&mut conn).await?;
    let keys = keys_for(
        &shapes,
        &options.stable_key_columns,
        &options.exclude_columns,
    );
    for shape in &shapes {
        let undecided = !shape.has_declared_key
            && shape.unique_indexes.len() > 1
            && !keys.contains_key(&shape.name);
        if undecided {
            tracing::warn!(
                table = %shape.name,
                "lightroom: several UNIQUE indexes and none is named as the key; \
                 mirroring the table keyless (pin one with primary_keys)"
            );
        }
    }
    let mut checked = BTreeMap::new();
    for (table, key) in keys {
        if has_null(&mut conn, &table, &key).await? {
            tracing::warn!(
                table,
                key = %key.join(", "),
                "lightroom: the UNIQUE index has NULLs in some rows, so it cannot be \
                 the key; mirroring the table keyless"
            );
        } else {
            checked.insert(table, key);
        }
    }
    let _ = conn.close().await;
    checked.extend(options.primary_keys.clone());
    Ok(MirrorOptions {
        primary_keys: checked,
        ..options.clone()
    })
}

async fn open_read_only(path: &Path) -> Result<SqliteConnection> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .with_context(|| format!("sqlite uri for {}", path.display()))?
        .read_only(true)
        .create_if_missing(false);
    SqliteConnection::connect_with(&opts)
        .await
        .with_context(|| format!("open {} read-only", path.display()))
}

async fn read_shapes(conn: &mut SqliteConnection) -> Result<Vec<TableShape>> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
         ORDER BY name",
    )
    .fetch_all(&mut *conn)
    .await
    .context("list the catalog's tables")?;
    let mut shapes = Vec::new();
    for name in names {
        let cols = sqlx::query("SELECT name, pk FROM pragma_table_info(?)")
            .bind(&name)
            .fetch_all(&mut *conn)
            .await
            .with_context(|| format!("table_info({name})"))?;
        let columns = cols.iter().map(|r| r.get::<String, _>("name")).collect();
        let has_declared_key = cols.iter().any(|r| r.get::<i64, _>("pk") > 0);
        let listed = sqlx::query("SELECT name, \"unique\", partial FROM pragma_index_list(?)")
            .bind(&name)
            .fetch_all(&mut *conn)
            .await
            .with_context(|| format!("index_list({name})"))?;
        let mut unique_indexes = Vec::new();
        for ix in &listed {
            if ix.get::<i64, _>("unique") == 0 || ix.get::<i64, _>("partial") != 0 {
                continue;
            }
            let index: String = ix.get("name");
            let parts = sqlx::query("SELECT name FROM pragma_index_info(?)")
                .bind(&index)
                .fetch_all(&mut *conn)
                .await
                .with_context(|| format!("index_info({index})"))?;
            // A NULL column name is an expression, not a column.
            let columns: Vec<String> = parts
                .iter()
                .filter_map(|r| r.get::<Option<String>, _>("name"))
                .collect();
            if !columns.is_empty() && columns.len() == parts.len() {
                unique_indexes.push(UniqueIndex {
                    name: index,
                    columns,
                });
            }
        }
        shapes.push(TableShape {
            name,
            columns,
            has_declared_key,
            unique_indexes,
        });
    }
    Ok(shapes)
}

/// A UNIQUE index lets NULLs repeat; a primary key does not.
async fn has_null(conn: &mut SqliteConnection, table: &str, key: &[String]) -> Result<bool> {
    let any_null = key
        .iter()
        .map(|c| format!("{} IS NULL", quote_ident(c)))
        .collect::<Vec<_>>()
        .join(" OR ");
    // Audited: names come out of the catalog's own schema through
    // `quote_ident`.
    let sql = format!(
        "SELECT EXISTS (SELECT 1 FROM {} WHERE {any_null})",
        quote_ident(table)
    );
    let found: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_one(&mut *conn)
        .await
        .with_context(|| format!("check {table}({}) for NULLs", key.join(", ")))?;
    Ok(found != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ix(name: &str, cols: &[&str]) -> UniqueIndex {
        UniqueIndex {
            name: name.to_string(),
            columns: cols.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn cols(c: &[&str]) -> Vec<String> {
        c.iter().map(|c| c.to_string()).collect()
    }

    fn shape(name: &str, columns: &[&str], declared: bool, idx: Vec<UniqueIndex>) -> TableShape {
        TableShape {
            name: name.to_string(),
            columns: cols(columns),
            has_declared_key: declared,
            unique_indexes: idx,
        }
    }

    #[test]
    fn the_only_unique_index_is_the_key() {
        let all = [ix(
            "sqlite_autoindex_MigratedImages_1",
            &["localId", "ozCatalogId"],
        )];
        assert_eq!(
            key_of(&all),
            Some(cols(&["localId", "ozCatalogId"]).as_slice())
        );
    }

    #[test]
    fn the_one_named_primary_key_wins_among_several() {
        let all = [
            ix("index_T_changeCounter", &["changeCounter"]),
            ix("index_T_primaryKey", &["ozCatalogId", "ozAssetId"]),
        ];
        assert_eq!(
            key_of(&all),
            Some(cols(&["ozCatalogId", "ozAssetId"]).as_slice())
        );
    }

    #[test]
    fn several_unique_indexes_with_no_primary_key_name_have_no_key() {
        assert_eq!(
            key_of(&[ix("by_name", &["name"]), ix("by_guid", &["guid"])]),
            None
        );
    }

    #[test]
    fn two_indexes_named_primary_key_have_no_key() {
        assert_eq!(
            key_of(&[ix("a_primaryKey", &["a"]), ix("b_primaryKey", &["b"])]),
            None
        );
    }

    #[test]
    fn no_unique_index_is_no_key() {
        assert_eq!(key_of(&[]), None);
    }

    #[test]
    fn tables_the_engine_keys_itself_are_left_alone() {
        let idx = vec![ix("index_T_primaryKey", &["a"])];
        let tables = [
            shape("Declared", &["a"], true, idx.clone()),
            shape("HasGlobal", &["a", "id_global"], false, idx.clone()),
            shape("Keyless", &["a"], false, idx),
        ];
        let got = keys_for(&tables, &cols(&["id_global"]), &[]);
        assert_eq!(got.keys().collect::<Vec<_>>(), ["Keyless"]);
    }

    #[test]
    fn a_key_naming_an_excluded_column_is_left_out() {
        let tables = [shape(
            "T",
            &["a", "b"],
            false,
            vec![ix("T_primaryKey", &["a", "b"])],
        )];
        assert!(keys_for(&tables, &[], &cols(&["T.b"])).is_empty());
    }

    async fn catalog(stmts: &[&str]) -> (tempfile::TempDir, MirrorOptions) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.lrcat");
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let mut conn = SqliteConnection::connect_with(&opts).await.unwrap();
        for s in stmts {
            // Test: literal statements.
            sqlx::query(sqlx::AssertSqlSafe(*s))
                .execute(&mut conn)
                .await
                .unwrap();
        }
        conn.close().await.unwrap();
        let options = MirrorOptions {
            stable_key_columns: cols(&["id_global"]),
            ..MirrorOptions::new(path)
        };
        (dir, options)
    }

    #[tokio::test]
    async fn the_catalogs_keys_reach_primary_keys_after_the_null_check() {
        let (_dir, options) = catalog(&[
            "CREATE TABLE Synced (image INTEGER, payloadKey TEXT, payloadData TEXT)",
            "CREATE UNIQUE INDEX index_Synced_primaryKey ON Synced(image, payloadKey)",
            "INSERT INTO Synced VALUES (1,'a','x')",
            "CREATE TABLE WithNull (a INTEGER, b TEXT)",
            "CREATE UNIQUE INDEX index_WithNull_primaryKey ON WithNull(a, b)",
            "INSERT INTO WithNull VALUES (1,NULL),(2,NULL)",
            "CREATE TABLE Two (a INTEGER, b INTEGER)",
            "CREATE UNIQUE INDEX by_a ON Two(a)",
            "CREATE UNIQUE INDEX by_b ON Two(b)",
            "CREATE TABLE Declared (id INTEGER PRIMARY KEY, a INTEGER)",
            "CREATE UNIQUE INDEX d_a ON Declared(a)",
        ])
        .await;
        let got = with_catalog_keys(&options).await.unwrap().primary_keys;
        let want: BTreeMap<String, Vec<String>> =
            BTreeMap::from([("Synced".to_string(), cols(&["image", "payloadKey"]))]);
        assert_eq!(got, want);
    }

    #[tokio::test]
    async fn a_key_the_user_pinned_wins() {
        let (_dir, mut options) = catalog(&[
            "CREATE TABLE Synced (image INTEGER, payloadKey TEXT)",
            "CREATE UNIQUE INDEX index_Synced_primaryKey ON Synced(image, payloadKey)",
        ])
        .await;
        options
            .primary_keys
            .insert("Synced".into(), cols(&["image"]));
        let got = with_catalog_keys(&options).await.unwrap().primary_keys;
        assert_eq!(got["Synced"], cols(&["image"]));
    }
}
