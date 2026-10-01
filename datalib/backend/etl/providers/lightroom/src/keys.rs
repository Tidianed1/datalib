//! Which UNIQUE index keys a Lightroom table that has no PRIMARY KEY.
//! Lightroom declares its cloud-sync tables' keys as a UNIQUE index named
//! `index_<Table>_primaryKey`; the engine asks [`key_index`] about any
//! table it would otherwise mirror keyless.

use datalib_etl_sqlite_mirror::UniqueIndex;

/// The table's only UNIQUE index, or the one named `…primaryKey` among
/// several. With several and no such name there is no answer.
pub fn key_index(indexes: &[UniqueIndex]) -> Option<&UniqueIndex> {
    if let [only] = indexes {
        return Some(only);
    }
    let mut named = indexes
        .iter()
        .filter(|i| i.name.to_ascii_lowercase().ends_with("primarykey"));
    match (named.next(), named.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ix(name: &str) -> UniqueIndex {
        UniqueIndex {
            name: name.to_string(),
            columns: vec!["a".to_string()],
        }
    }

    fn pick(indexes: &[UniqueIndex]) -> Option<&str> {
        key_index(indexes).map(|i| i.name.as_str())
    }

    #[test]
    fn the_only_unique_index_is_the_key_whatever_its_name() {
        assert_eq!(
            pick(&[ix("sqlite_autoindex_MigratedImages_1")]),
            Some("sqlite_autoindex_MigratedImages_1")
        );
    }

    #[test]
    fn the_one_named_primary_key_wins_among_several() {
        let indexes = [ix("index_T_changeCounter"), ix("index_T_primaryKey")];
        assert_eq!(pick(&indexes), Some("index_T_primaryKey"));
    }

    #[test]
    fn several_with_no_name_or_two_names_have_no_key() {
        assert_eq!(pick(&[ix("by_a"), ix("by_b")]), None);
        assert_eq!(pick(&[ix("a_primaryKey"), ix("b_primaryKey")]), None);
        assert_eq!(pick(&[]), None);
    }
}
