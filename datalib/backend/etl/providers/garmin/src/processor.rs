//! The ingest wave for the `garmin` source.

use std::path::PathBuf;

use anyhow::{Context, Result};
use async_trait::async_trait;

use datalib_etl::processor::{DataProcessor, PlanContext, RunCtx};
use datalib_etl_garmin_config::{GarminApi, GarminConfig};

use crate::auth::{expand_token_dir, Credentials};
use crate::ingest;

pub fn plan_ingest(ctx: PlanContext, config: GarminConfig) -> Result<Vec<Box<dyn DataProcessor>>> {
    let name = ctx.name;
    let raw_path = config.common.raw_path().to_path_buf();
    let mut procs: Vec<Box<dyn DataProcessor>> = Vec::new();
    if let Some(api) = config.api {
        procs.push(Box::new(GarminIngest {
            id: format!("garmin/{name}/download"),
            raw_path,
            api,
        }));
    }
    Ok(procs)
}

struct GarminIngest {
    id: String,
    raw_path: PathBuf,
    api: GarminApi,
}

/// The playback bearer: no request reaches Garmin, and the value is
/// never inspected, but a fixture run must not go looking for a token
/// file on the host.
pub const PLAYBACK_BEARER: &str = "playback";

#[async_trait]
impl DataProcessor for GarminIngest {
    fn id(&self) -> &str {
        &self.id
    }

    /// Every write is an upsert of whole rows and every prune is scoped
    /// to a window the run has already re-listed, so between checkpoints
    /// the store is the previous snapshot plus what this run fetched.
    fn streams_output(&self) -> bool {
        true
    }

    async fn run(&self, ctx: &RunCtx<'_>) -> Result<String> {
        let creds =
            if std::env::var_os(datalib_etl::http::PLAYBACK_ENV).is_some_and(|v| !v.is_empty()) {
                Credentials::fixed(PLAYBACK_BEARER)
            } else {
                Credentials::load(&expand_token_dir(self.api.token_dir.as_deref()))?
            };
        let today = datalib_time::parse_strict(ctx.now)
            .with_context(|| format!("garmin: run stamp {:?}", ctx.now))?
            .inner()
            .date_naive();
        let db = ingest::RawDb::open(&ingest::db_path_for(&self.raw_path)).await?;
        let (pool, cas_pool) = (db.pool().clone(), db.cas().pool().clone());
        ctx.run_store(pool, Some(cas_pool), |sealer| async {
            let s = ingest::fetch(ingest::FetchOptions {
                db,
                creds,
                api: self.api.clone(),
                today,
                progress: ctx.progress.clone(),
                control: ctx.control.clone(),
                sealer: Some(sealer),
            })
            .await?;
            Ok(s.line())
        })
        .await
    }
}
