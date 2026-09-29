//! The render wave for an apple_photos source: its planner and the
//! [`RenderProcessor`] it plans.

use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use datalib_etl::processor::PlanContext;
use datalib_etl_apple_photos_config::ApplePhotosRenderConfig;
use datalib_etl_render::one_page::skip_if_current;
use datalib_etl_render::processor::{plan_source_render, RenderCtx, RenderProcessor, SourceRender};

use crate::render::{document_uuid, parse, render_all, RENDER_VERSION};

/// Always planned: the driver's reverse lookup says whether the tables
/// the page reads moved, so a no-op run costs one `dolt_log()` query.
pub fn plan_render(
    ctx: PlanContext,
    config: ApplePhotosRenderConfig,
) -> Result<Vec<Box<dyn RenderProcessor>>> {
    Ok(plan_source_render(
        ctx,
        config.common.raw_path(),
        ApplePhotosRender,
    ))
}

struct ApplePhotosRender;

#[async_trait]
impl SourceRender for ApplePhotosRender {
    const PROVIDER: &'static str = "apple_photos";

    fn render_version(&self) -> u32 {
        RENDER_VERSION
    }

    async fn run(&self, raw_path: &Path, ctx: &RenderCtx<'_>) -> Result<String> {
        let page = document_uuid(ctx.name);
        if let Some(done) = skip_if_current(ctx, Self::PROVIDER, &page) {
            return Ok(done);
        }
        let Some(parsed) = parse::parse(raw_path, ctx.raw_range())
            .with_context(|| format!("apple_photos parse {}", raw_path.display()))?
        else {
            return Ok("nothing committed to render yet".to_string());
        };
        ctx.declare_bucket(&page, &parsed.inputs)?;
        let mut on_doc = |md| ctx.emit_doc(md);
        let assets = render_all(&parsed, ctx.root, ctx.name, &mut on_doc)?;
        ctx.consumed(&parsed.head);
        Ok(format!("assets={assets}"))
    }
}
