//! `GET /api/pipeline/history?tree=<id>`: the commit history of every
//! doltlite store under one declared tree — a step's, or a group's with
//! its steps' under it. `GET /api/pipeline/history/changes?store=<path>
//! &from=<hash>&to=<hash>`: what differs between two commits of one of
//! those stores. Reads the stores; never writes them.

use std::path::Path;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::{usage, AppState};

/// How many commits to walk when the request does not say. Each one
/// costs a diff against its parent, so the walk is bounded rather than
/// the whole log read back for a store that checkpoints every few
/// seconds.
const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 5_000;

#[derive(Debug, Deserialize)]
pub struct HistoryParams {
    tree: String,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct TreeHistory {
    pub tree: String,
    /// One per `.doltlite_db` file found, in path order.
    pub stores: Vec<StoreEntry>,
}

#[derive(Debug, Serialize)]
pub struct StoreEntry {
    /// The file, data-root-relative.
    pub path: String,
    #[serde(flatten)]
    pub history: datalib_history::StoreHistory,
}

pub async fn tree_history(
    State(s): State<AppState>,
    Query(p): Query<HistoryParams>,
) -> Result<Json<TreeHistory>, (StatusCode, String)> {
    let declared = usage::declared_trees(&s.config_path());
    if !declared.contains(&p.tree) {
        return Err((
            StatusCode::NOT_FOUND,
            format!("no group or step writes {}", p.tree),
        ));
    }
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut stores = Vec::new();
    for rel in stores_under(&s.root, &p.tree, &declared) {
        let history = datalib_history::read(&s.root.join(&rel), limit)
            .await
            .map_err(|e| {
                tracing::warn!("history: {rel}: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{rel}: {e:#}"))
            })?;
        stores.push(StoreEntry { path: rel, history });
    }
    Ok(Json(TreeHistory {
        tree: p.tree,
        stores,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ChangesParams {
    /// Data-root-relative, as `/api/pipeline/history` names it.
    store: String,
    from: String,
    to: String,
}

pub async fn store_changes(
    State(s): State<AppState>,
    Query(p): Query<ChangesParams>,
) -> Result<Json<datalib_history::Changes>, (StatusCode, String)> {
    let declared = usage::declared_trees(&s.config_path());
    // Only a store the history lists can be asked about: that is what
    // keeps `store` from naming any other file under the root.
    let listed = p.store.rsplit_once('/').is_some_and(|(tree, _)| {
        declared.iter().any(|t| t == tree)
            && stores_under(&s.root, tree, &declared).contains(&p.store)
    });
    if !listed {
        return Err((
            StatusCode::NOT_FOUND,
            format!("no step keeps a store at {}", p.store),
        ));
    }
    match datalib_history::changes_between(&s.root.join(&p.store), &p.from, &p.to).await {
        Ok(Some(changes)) => Ok(Json(changes)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            format!("{} has no commit {} or no commit {}", p.store, p.from, p.to),
        )),
        Err(e) => {
            tracing::warn!("history changes: {}: {e:#}", p.store);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{}: {e:#}", p.store),
            ))
        }
    }
}

/// Every `.doltlite_db` directly inside the tree's directory, then inside
/// each declared tree under it — a group's steps. Root-relative, sorted.
/// Not a recursive walk: a render tree holds thousands of markdown files
/// and no store below its top level.
fn stores_under(root: &Path, tree: &str, declared: &[String]) -> Vec<String> {
    let prefix = format!("{tree}/");
    let dirs = std::iter::once(tree).chain(
        declared
            .iter()
            .map(String::as_str)
            .filter(|t| t.starts_with(&prefix)),
    );
    let mut found = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.ends_with(".doltlite_db") && entry.path().is_file() {
                found.push(format!("{dir}/{name}"));
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_finds_its_steps_stores_and_nothing_deeper() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for p in [
            "slack/ingest/entities.doltlite_db",
            "slack/ingest/blobs.sqlite",
            "slack/render_markdown/indexed_markdown.doltlite_db",
            "slack/render_markdown/deep/nested.doltlite_db",
            "slack/stray.doltlite_db",
            "other/ingest/entities.doltlite_db",
        ] {
            let path = root.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"").unwrap();
        }
        let declared: Vec<String> = ["slack", "slack/ingest", "slack/render_markdown", "other"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            stores_under(root, "slack", &declared),
            [
                "slack/ingest/entities.doltlite_db",
                "slack/render_markdown/indexed_markdown.doltlite_db",
                "slack/stray.doltlite_db",
            ]
        );
        assert_eq!(
            stores_under(root, "slack/ingest", &declared),
            ["slack/ingest/entities.doltlite_db"]
        );
        assert!(stores_under(root, "missing", &declared).is_empty());
    }
}
