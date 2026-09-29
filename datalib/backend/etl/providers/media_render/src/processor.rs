//! The render wave for a media source: its planner and the
//! [`RenderProcessor`] it plans.

use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use datalib_etl::processor::PlanContext;
use datalib_etl_media_config::MediaRenderConfig;
use datalib_etl_render::one_page::skip_if_current;
use datalib_etl_render::processor::{plan_source_render, RenderCtx, RenderProcessor, SourceRender};

use crate::render::{document_uuid, parse, render_all, RENDER_VERSION};

/// Always planned: the driver's reverse lookup says whether the tables
/// the page reads moved, so a no-op run costs one `dolt_log()` query.
pub fn plan_render(
    ctx: PlanContext,
    config: MediaRenderConfig,
) -> Result<Vec<Box<dyn RenderProcessor>>> {
    Ok(plan_source_render(
        ctx,
        config.common.raw_path(),
        MediaRender,
    ))
}

struct MediaRender;

#[async_trait]
impl SourceRender for MediaRender {
    const PROVIDER: &'static str = "media";

    fn render_version(&self) -> u32 {
        RENDER_VERSION
    }

    async fn run(&self, raw_path: &Path, ctx: &RenderCtx<'_>) -> Result<String> {
        let page = document_uuid(ctx.name);
        if let Some(done) = skip_if_current(ctx, Self::PROVIDER, &page) {
            return Ok(done);
        }
        let Some(parsed) = parse::parse(raw_path, ctx.raw_range())
            .with_context(|| format!("media parse {}", raw_path.display()))?
        else {
            return Ok("nothing committed to render yet".to_string());
        };
        ctx.declare_bucket(&page, &parse::inputs())?;
        let mut on_doc = |md| ctx.emit_doc(md);
        let files = render_all(&parsed, ctx.root, ctx.name, &mut on_doc)?;
        ctx.consumed(&parsed.head);
        Ok(format!("files={files}"))
    }
}
