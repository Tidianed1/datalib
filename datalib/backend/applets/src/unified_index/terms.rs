//! A search made only of identifiers (a uuid, an email address, a handle)
//! is answered from the grid's terms file, not from qmd: every row that
//! answers to each of them, best match first. The file and what it holds:
//! `datalib_etl_render::grid_terms`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use datalib_handle::Handle;
use datalib_schema::terms::{TermKind, META_GRID_COMMIT};
use datalib_unified_index::query::{extract_uuid_suffix, is_uuid_shape};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};

/// The free text as terms to look up, when every word of it is an
/// identifier; `None` when any word is not, so the search is qmd's.
pub fn identifiers(free_text: &str) -> Option<Vec<String>> {
    let words = datalib_query::tokenize(free_text);
    if words.is_empty() {
        return None;
    }
    words.iter().map(|w| identifier(w)).collect()
}

fn identifier(word: &str) -> Option<String> {
    // A quoted word is a phrase for qmd, and a quote would end the
    // terms file's own phrase early.
    if word.contains('"') {
        return None;
    }
    let id = extract_uuid_suffix(word);
    if is_uuid_shape(id) {
        return Some(id.to_lowercase());
    }
    let handle = if word.contains(':') {
        Handle::rebuild(word)
    } else if word.contains('@') {
        Handle::email(word)
    } else if word.starts_with('+') {
        Handle::tel(word)
    } else {
        None
    };
    handle.map(|h| h.as_str().to_string())
}

/// One term an identifier matched.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Hit {
    pub uuid: String,
    /// A [`TermKind`] code.
    pub kind: i64,
    pub value: String,
    pub touched_at_utc: Option<String>,
}

/// The rows every identifier matched, each at its best match: the most
/// telling kind first (`TermKind::affinity`), then newest. The score is
/// that affinity, and the words shown are the kind and the value matched.
pub fn rank(per_identifier: &[Vec<Hit>]) -> Vec<(String, (f64, String))> {
    let affinity = |h: &Hit| TermKind::from_code(h.kind).map_or(0, TermKind::affinity);
    let kind = |h: &Hit| TermKind::from_code(h.kind).map_or("term", TermKind::as_str);
    let Some((first, rest)) = per_identifier.split_first() else {
        return Vec::new();
    };
    let mut best: std::collections::HashMap<&str, &Hit> = std::collections::HashMap::new();
    for hit in first {
        let in_every = rest
            .iter()
            .all(|hits| hits.iter().any(|h| h.uuid == hit.uuid));
        if !in_every {
            continue;
        }
        let slot = best.entry(hit.uuid.as_str()).or_insert(hit);
        if affinity(hit) > affinity(slot) {
            *slot = hit;
        }
    }
    let mut ranked: Vec<&Hit> = best.into_values().collect();
    ranked.sort_by(|a, b| {
        affinity(b)
            .cmp(&affinity(a))
            .then_with(|| b.touched_at_utc.cmp(&a.touched_at_utc))
            .then_with(|| a.uuid.cmp(&b.uuid))
    });
    ranked
        .into_iter()
        .map(|h| {
            (
                h.uuid.clone(),
                (f64::from(affinity(h)), format!("{}: {}", kind(h), h.value)),
            )
        })
        .collect()
}

/// What the terms file says about `identifiers`, read in one transaction.
pub struct Found {
    pub per_identifier: Vec<Vec<Hit>>,
    /// The grid commit the file reflects. A ranking read at any other
    /// commit may be one pass out, so it is not kept.
    pub grid_commit: Option<String>,
}

/// `None` when the root has no terms file yet.
pub async fn lookup(root: &Path, identifiers: &[String]) -> Result<Option<Found>> {
    let path = datalib_runtime::layout::grid_terms_db(root);
    if !path.exists() {
        return Ok(None);
    }
    let mut conn = SqliteConnectOptions::new()
        .filename(format!(
            "file:{}?doltlite_engine=sqlite&mode=ro",
            path.display()
        ))
        .read_only(true)
        .create_if_missing(false)
        .busy_timeout(Duration::from_secs(5))
        .connect()
        .await
        .with_context(|| format!("open {}", path.display()))?;
    let read = async {
        sqlx::query("BEGIN").execute(&mut conn).await?;
        let grid_commit: Option<String> =
            sqlx::query_scalar("SELECT value FROM terms_meta WHERE key = ?")
                .bind(META_GRID_COMMIT)
                .fetch_optional(&mut conn)
                .await?;
        let mut per_identifier = Vec::with_capacity(identifiers.len());
        for id in identifiers {
            let hits: Vec<Hit> = sqlx::query_as(
                "SELECT r.uuid, t.kind, v.value, r.touched_at_utc FROM vals_fts \
                 JOIN vals v ON v.val_id = vals_fts.rowid \
                 JOIN terms t ON t.val_id = v.val_id \
                 JOIN rows r ON r.row_id = t.row_id WHERE vals_fts MATCH ?",
            )
            // A phrase, so the identifier is matched whole; an identifier
            // holds no double quote (`identifier`).
            .bind(format!("\"{id}\""))
            .fetch_all(&mut conn)
            .await?;
            per_identifier.push(hits);
        }
        sqlx::query("COMMIT").execute(&mut conn).await?;
        anyhow::Ok(Found {
            per_identifier,
            grid_commit,
        })
    }
    .await
    .context("read the terms file");
    conn.close().await.ok();
    read.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_uuids_and_handles_and_nothing_else() {
        assert_eq!(
            identifiers("00000000-0000-8B8A-896D-63addc7b31ad"),
            Some(vec!["00000000-0000-8b8a-896d-63addc7b31ad".to_string()])
        );
        assert_eq!(
            identifiers("away-team-00000000-0000-4000-8000-000000000001"),
            Some(vec!["00000000-0000-4000-8000-000000000001".to_string()])
        );
        assert_eq!(
            identifiers("Ann@Example.com +1 555 0100"),
            None,
            "a phone number in several words is not one identifier"
        );
        assert_eq!(
            identifiers("Ann@Example.com slack:T1/U2"),
            Some(vec![
                "email:ann@example.com".to_string(),
                "slack:T1/U2".to_string()
            ])
        );
        assert_eq!(identifiers("ann@example.com budget"), None);
        assert_eq!(identifiers("\"ann@example.com\""), None);
        assert_eq!(identifiers(""), None);
    }

    fn hit(uuid: &str, kind: &str, touched: &str) -> Hit {
        Hit {
            uuid: uuid.into(),
            kind: i64::from(TermKind::parse(kind).expect("a kind").code()),
            value: "v".into(),
            touched_at_utc: Some(touched.into()),
        }
    }

    #[test]
    fn rows_rank_by_their_best_kind_then_newest() {
        let ranked = rank(&[vec![
            hit("r-name", "name", "2026-03"),
            hit("r-old", "from", "2026-01"),
            hit("r-new", "from", "2026-02"),
            hit("r-id", "id", "2025-01"),
            hit("r-id", "name", "2026-04"),
        ]]);
        let order: Vec<&str> = ranked.iter().map(|(u, _)| u.as_str()).collect();
        assert_eq!(order, ["r-id", "r-new", "r-old", "r-name"]);
        assert_eq!(ranked[0].1, (5.0, "id: v".to_string()));
    }

    #[test]
    fn several_identifiers_keep_only_the_rows_matching_every_one() {
        let ranked = rank(&[
            vec![hit("a", "from", "1"), hit("b", "from", "1")],
            vec![hit("b", "container", "1")],
        ]);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].0, "b");
    }
}
