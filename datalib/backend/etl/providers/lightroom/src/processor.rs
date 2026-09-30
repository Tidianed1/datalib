//! Program-A `DataProcessor` for the `lightroom` source.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;

use datalib_etl::download_problems;
use datalib_etl::processor::{DataProcessor, PlanContext, RunCtx};
use datalib_etl::raw_layout;
use datalib_etl_lightroom_config::LightroomConfig;

use crate::ingest::{self, backups, unpack, MirrorOptions};

/// The engine's options for this config. `source_path` is the catalog,
/// else the backups folder, which the engine never reads itself: each
/// backup in it is mirrored as its own source.
pub fn mirror_options(config: &LightroomConfig) -> Result<MirrorOptions> {
    let source_path = config
        .catalog
        .as_ref()
        .or(config.backups.as_ref())
        .ok_or_else(|| anyhow!("lightroom: set `catalog.path`, `backups.path`, or both"))?
        .path();
    Ok(MirrorOptions {
        source_path,
        snapshot: config.snapshot,
        include_tables: config.include_tables.clone(),
        exclude_tables: config.exclude_tables.clone(),
        exclude_columns: config.effective_excluded_columns(),
        stable_key_columns: config.stable_key_columns.clone(),
        primary_keys: config.primary_keys.clone(),
        gc: config.gc,
        sidecar_tables: Vec::new(),
    })
}

pub fn plan_ingest(
    ctx: PlanContext,
    config: LightroomConfig,
) -> Result<Vec<Box<dyn DataProcessor>>> {
    let name = ctx.name;
    Ok(vec![Box::new(LightroomIngest {
        id: format!("lightroom/{name}/download"),
        raw_path: config.common.raw_path().to_path_buf(),
        catalog: config.catalog.as_ref().map(|p| p.path()),
        backups: config.backups.as_ref().map(|p| p.path()),
        options: mirror_options(&config)?,
    })])
}

/// The mirror processor. Owns its doltlite store end to end (open,
/// register the interrupt hook, mirror, commit + close via
/// `session.finish`).
struct LightroomIngest {
    id: String,
    raw_path: PathBuf,
    catalog: Option<PathBuf>,
    backups: Option<PathBuf>,
    options: MirrorOptions,
}

#[async_trait]
impl DataProcessor for LightroomIngest {
    fn id(&self) -> &str {
        &self.id
    }

    async fn run(&self, ctx: &RunCtx<'_>) -> Result<String> {
        let entity_db = raw_layout::entities_db(&self.raw_path);
        let pool = ingest::mirror::open_mirror(&entity_db).await?;
        let session = ctx.open_store(pool.clone(), entity_db).await;
        let (summary, problems) = match (&self.backups, &self.catalog) {
            (Some(dir), catalog) => {
                let now = chrono::DateTime::parse_from_rfc3339(ctx.now)
                    .with_context(|| format!("the run's now {:?} is not RFC 3339", ctx.now))?
                    .with_timezone(&chrono::Local)
                    .naive_local();
                let inputs = backups::Inputs {
                    backups: dir,
                    catalog: catalog.as_deref(),
                    now,
                };
                let run = backups::ingest(
                    &pool,
                    inputs,
                    &self.options,
                    ctx.progress,
                    &ctx.control.stop,
                    ctx.name,
                )
                .await?;
                (run.summary(), run.problems)
            }
            (None, Some(catalog)) => {
                let stats =
                    unpack::mirror_file(&pool, catalog, &self.options, ctx.progress).await?;
                (stats.summary(), Vec::new())
            }
            (None, None) => unreachable!("validate() refuses a config with neither"),
        };
        // Every run, so a backup that is placed or removed stops being a
        // problem.
        download_problems::report_records(&pool, &problems).await;
        session.finish(ctx, summary).await
    }
}
