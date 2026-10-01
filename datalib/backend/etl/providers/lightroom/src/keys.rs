//! The keys of the Lightroom catalog tables that have no PRIMARY KEY.
//! Lightroom declares those tables' keys as a UNIQUE index named
//! `index_<Table>_primaryKey`. The rule here is a pure function of the
//! catalog's [`SourceSchema`]; [`keyed_options`] applies it, handing the
//! mirror engine the key columns per table as `primary_keys`.

use std::collections::BTreeMap;

use datalib_etl_lightroom_config::glob_match;
use datalib_etl_sqlite_mirror::{SourceSchema, UniqueIndex};

use crate::ingest::MirrorOptions;

/// Why a table that could have been keyed was left keyless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    /// Several UNIQUE indexes and none named as the key.
    Ambiguous { table: String, indexes: Vec<String> },
    /// The chosen index has NULLs, which a key cannot.
    HasNulls { table: String, key: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyPlan {
    pub keys: BTreeMap<String, Vec<String>>,
    pub skipped: Vec<Skipped>,
}

/// The table's only UNIQUE index, or the one named `…primaryKey` among
/// several. A UNIQUE index is a constraint, not an identity, so with
/// several and no such name there is no answer.
fn key_index(indexes: &[UniqueIndex]) -> Option<&UniqueIndex> {
    if let [only] = indexes {
        return Some(only);
    }
    let mut named = indexes
        .iter()
        .filter(|i| i.name.to_ascii_lowercase().contains("primarykey"));
    match (named.next(), named.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

/// The key for each table that would otherwise be keyless. A table with
/// a declared key, or with a column the engine keys on first
/// (`id_global`), is left to the engine; a key naming a column the
/// filters drop is left out, since the engine refuses it.
pub fn plan_keys(
    schema: &SourceSchema,
    stable_key_columns: &[String],
    exclude_columns: &[String],
) -> KeyPlan {
    let mut plan = KeyPlan::default();
    for table in &schema.tables {
        let engine_keys_it = !table.declared_key.is_empty()
            || table.columns.iter().any(|c| stable_key_columns.contains(c));
        if engine_keys_it {
            continue;
        }
        let Some(index) = key_index(&table.unique_indexes) else {
            if table.unique_indexes.len() > 1 {
                plan.skipped.push(Skipped::Ambiguous {
                    table: table.name.clone(),
                    indexes: table
                        .unique_indexes
                        .iter()
                        .map(|i| i.name.clone())
                        .collect(),
                });
            }
            continue;
        };
        let dropped = index.columns.iter().any(|c| {
            exclude_columns
                .iter()
                .any(|p| glob_match(p, &format!("{}.{c}", table.name)))
        });
        if dropped {
            continue;
        }
        if index.has_nulls {
            plan.skipped.push(Skipped::HasNulls {
                table: table.name.clone(),
                key: index.columns.clone(),
            });
        } else {
            plan.keys.insert(table.name.clone(), index.columns.clone());
        }
    }
    plan
}

/// `options` with the catalog's keys added to `primary_keys`, each skipped
/// table warned about. An entry the user already set for a table wins.
pub fn keyed_options(options: &MirrorOptions, schema: &SourceSchema) -> MirrorOptions {
    let plan = plan_keys(
        schema,
        &options.stable_key_columns,
        &options.exclude_columns,
    );
    for skipped in &plan.skipped {
        match skipped {
            Skipped::Ambiguous { table, indexes } => tracing::warn!(
                table,
                indexes = %indexes.join(", "),
                "lightroom: several UNIQUE indexes and none is named as the key; \
                 mirroring the table keyless (pin one with primary_keys)"
            ),
            Skipped::HasNulls { table, key } => tracing::warn!(
                table,
                key = %key.join(", "),
                "lightroom: the UNIQUE index has NULLs in some rows, so it cannot be \
                 the key; mirroring the table keyless"
            ),
        }
    }
    let mut keys = plan.keys;
    keys.extend(options.primary_keys.clone());
    MirrorOptions {
        primary_keys: keys,
        ..options.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datalib_etl_sqlite_mirror::TableInfo;

    fn ix(name: &str, cols: &[&str], has_nulls: bool) -> UniqueIndex {
        UniqueIndex {
            name: name.to_string(),
            columns: cols.iter().map(|c| c.to_string()).collect(),
            has_nulls,
        }
    }

    fn cols(c: &[&str]) -> Vec<String> {
        c.iter().map(|c| c.to_string()).collect()
    }

    fn table(name: &str, columns: &[&str], declared: &[&str], idx: Vec<UniqueIndex>) -> TableInfo {
        TableInfo {
            name: name.to_string(),
            columns: cols(columns),
            declared_key: cols(declared),
            unique_indexes: idx,
        }
    }

    fn plan(tables: Vec<TableInfo>) -> KeyPlan {
        plan_keys(&SourceSchema { tables }, &cols(&["id_global"]), &[])
    }

    #[test]
    fn the_only_unique_index_is_the_key() {
        let p = plan(vec![table(
            "MigratedImages",
            &["localId", "ozCatalogId"],
            &[],
            vec![ix(
                "sqlite_autoindex_MigratedImages_1",
                &["localId", "ozCatalogId"],
                false,
            )],
        )]);
        assert_eq!(p.keys["MigratedImages"], cols(&["localId", "ozCatalogId"]));
    }

    #[test]
    fn the_one_named_primary_key_wins_among_several() {
        let p = plan(vec![table(
            "T",
            &["a", "b", "c"],
            &[],
            vec![
                ix("index_T_changeCounter", &["c"], false),
                ix("index_T_primaryKey", &["a", "b"], false),
            ],
        )]);
        assert_eq!(p.keys["T"], cols(&["a", "b"]));
    }

    #[test]
    fn several_unique_indexes_with_no_primary_key_name_are_skipped() {
        let p = plan(vec![table(
            "T",
            &["a", "b"],
            &[],
            vec![ix("by_a", &["a"], false), ix("by_b", &["b"], false)],
        )]);
        assert!(p.keys.is_empty());
        assert_eq!(
            p.skipped,
            [Skipped::Ambiguous {
                table: "T".into(),
                indexes: cols(&["by_a", "by_b"])
            }]
        );
    }

    #[test]
    fn two_indexes_named_primary_key_are_skipped() {
        let p = plan(vec![table(
            "T",
            &["a", "b"],
            &[],
            vec![
                ix("a_primaryKey", &["a"], false),
                ix("b_primaryKey", &["b"], false),
            ],
        )]);
        assert!(p.keys.is_empty() && p.skipped.len() == 1);
    }

    #[test]
    fn a_key_with_nulls_is_skipped() {
        let p = plan(vec![table(
            "T",
            &["a"],
            &[],
            vec![ix("T_primaryKey", &["a"], true)],
        )]);
        assert!(p.keys.is_empty());
        assert_eq!(
            p.skipped,
            [Skipped::HasNulls {
                table: "T".into(),
                key: cols(&["a"])
            }]
        );
    }

    #[test]
    fn tables_the_engine_keys_itself_are_left_alone() {
        let idx = vec![ix("index_T_primaryKey", &["a"], false)];
        let p = plan(vec![
            table("Declared", &["a"], &["a"], idx.clone()),
            table("HasGlobal", &["a", "id_global"], &[], idx.clone()),
            table("Keyless", &["a"], &[], idx),
        ]);
        assert_eq!(p.keys.keys().collect::<Vec<_>>(), ["Keyless"]);
    }

    #[test]
    fn a_table_with_no_unique_index_stays_keyless_without_a_warning() {
        let p = plan(vec![table("T", &["a"], &[], vec![])]);
        assert!(p.keys.is_empty() && p.skipped.is_empty());
    }

    #[test]
    fn a_key_naming_an_excluded_column_is_left_out() {
        let schema = SourceSchema {
            tables: vec![table(
                "T",
                &["a", "b"],
                &[],
                vec![ix("T_primaryKey", &["a", "b"], false)],
            )],
        };
        assert!(plan_keys(&schema, &[], &cols(&["T.b"])).keys.is_empty());
    }

    #[test]
    fn a_key_the_user_pinned_wins() {
        let schema = SourceSchema {
            tables: vec![table(
                "T",
                &["a", "b"],
                &[],
                vec![ix("T_primaryKey", &["a", "b"], false)],
            )],
        };
        let mut options = MirrorOptions::new("/dev/null");
        options.primary_keys.insert("T".into(), cols(&["a"]));
        assert_eq!(
            keyed_options(&options, &schema).primary_keys["T"],
            cols(&["a"])
        );
    }
}
