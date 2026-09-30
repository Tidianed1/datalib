//! Program-A `DataProcessor` for the `lightroom` source.

use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;

use datalib_etl::download_problems;
use datalib_etl::processor::{DataProcessor, PlanContext, RunCtx};
use datalib_etl::raw_layout;
use datalib_etl_lightroom_config::{LightroomConfig, LightroomMethod};

use crate::ingest::{self, backups, unpack, MirrorOptions};

/// The engine's options for this config. `source_path` is the catalog,
/// or for a backups folder the folder, which the engine never reads
/// itself: each backup in it is mirrored as its own source.
pub fn mirror_options(config: &LightroomConfig) -> Result<MirrorOptions> {
    let source_path = match config.method()? {
        LightroomMethod::Catalog(p) | LightroomMethod::Backups(p) => p.path(),
    };
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
        backups: matches!(config.method()?, LightroomMethod::Backups(_)),
        options: mirror_options(&config)?,
    })])
}

/// The mirror processor. Owns its doltlite store end to end (open,
/// register the interrupt hook, mirror, commit + close via
/// `session.finish`).
struct LightroomIngest {
    id: String,
    raw_path: PathBuf,
    /// `options.source_path` is a folder of backups, not a catalog.
    backups: bool,
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
        let (summary, problems) = if self.backups {
            let run = backups::ingest(
                &pool,
                &self.options.source_path,
                &self.options,
                ctx.progress,
                &ctx.control.stop,
                ctx.name,
            )
            .await?;
            (run.summary(), run.problems)
        } else {
            let stats = unpack::mirror_file(
                &pool,
                &self.options.source_path,
                &self.options,
                ctx.progress,
            )
            .await?;
            (stats.summary(), Vec::new())
        };
        // Every run, so a backup that is placed or removed stops being a
        // problem.
        download_problems::report_records(&pool, &problems).await;
        session.finish(ctx, summary).await
    }
}
