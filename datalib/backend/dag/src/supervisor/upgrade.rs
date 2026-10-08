//! What a launch does before the loop takes any request
//! (`docs/dev/plans/upgrade_on_launch.md`): find the raw stores another
//! build wrote, so the host can migrate them with no download, and name
//! the derived stores in an old shape, so it can offer to re-render them.

use std::path::Path;

use datalib_store_meta::Meta;

use crate::graph::Graph;
use crate::step::{StepId, StepRun, StepSpec};

/// Which build is running: what a store's `_datalib_meta` is compared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub version: String,
    pub git_hash: Option<String>,
}

impl Build {
    pub fn this() -> Build {
        Build {
            version: datalib_runtime::build_id::DATALIB_VERSION.to_string(),
            git_hash: datalib_runtime::build_id::git_hash(),
        }
    }
}

/// A store another build wrote, or one from before stores said who wrote
/// them. Opening a store rewrites its `_datalib_meta`, so once migrated a
/// store matches until the next build.
pub fn written_by_another_build(meta: Option<&Meta>, this: &Build) -> bool {
    meta.is_none_or(|m| m.datalib_version != this.version || m.git_hash != this.git_hash)
}

/// An ingest step `datalib-step` runs: the steps whose raw store it knows
/// how to migrate. A custom command's store is its own business.
fn builtin_ingest(spec: &StepSpec) -> bool {
    let StepRun::Subprocess { argv, .. } = &spec.run else {
        return false;
    };
    spec.function.as_deref() == Some("ingest")
        && argv
            .first()
            .is_some_and(|prog| crate::config::is_datalib_step(prog))
}

/// The built-in ingest steps whose raw store exists and was written by
/// another build. A store whose `_datalib_meta` cannot be read is named
/// too: the migrate's own open is what says what is wrong with it.
pub async fn raw_stores_to_migrate(root: &Path, graph: &Graph, this: &Build) -> Vec<StepId> {
    let mut out = Vec::new();
    for spec in graph.steps.iter().filter(|s| builtin_ingest(s)) {
        let db = root
            .join(&spec.id)
            .join(datalib_runtime::layout::ENTITIES_DB);
        if !db.exists() {
            continue;
        }
        let behind = match datalib_store_meta::guard::read_at(&db).await {
            Ok(meta) => written_by_another_build(meta.as_ref(), this),
            Err(e) => {
                tracing::warn!(
                    step = %spec.id,
                    error = %format!("{e:#}"),
                    "upgrade: could not read who wrote the raw store; migrating it"
                );
                true
            }
        };
        if behind {
            out.push(spec.id.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(version: &str, git_hash: Option<&str>) -> Meta {
        Meta {
            datalib_version: version.into(),
            git_hash: git_hash.map(str::to_string),
            doltlite_version: None,
            schema_hash: String::new(),
            schema_version: 0,
            shared_schema_version: 0,
            store_kind: None,
            written_at_utc: String::new(),
        }
    }

    /// A release bump, a dev build from another commit and a store from
    /// before `_datalib_meta` all need the pass; the build that last opened
    /// the store does not.
    #[test]
    fn a_store_is_migrated_when_another_build_wrote_it() {
        let this = Build {
            version: "0.41.0".into(),
            git_hash: Some("aaaa111".into()),
        };
        assert!(!written_by_another_build(
            Some(&meta("0.41.0", Some("aaaa111"))),
            &this
        ));
        assert!(written_by_another_build(
            Some(&meta("0.40.0", Some("aaaa111"))),
            &this
        ));
        assert!(written_by_another_build(
            Some(&meta("0.41.0", Some("bbbb222"))),
            &this
        ));
        assert!(written_by_another_build(Some(&meta("0.41.0", None)), &this));
        assert!(written_by_another_build(None, &this));
    }
}
