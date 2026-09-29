//! A source whose whole raw store renders to one page: the time-series
//! sources' plots and the summary pages alike. The page is the source's
//! only bucket, keyed by its own `markdown_uuid`, and it declares the
//! tables it reads whole.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use datalib_schema::grid_rows::GridRow;
use datalib_schema::problems::ProblemRow;

use crate::grid_index::RenderedMarkdown;
use crate::processor::RenderCtx;

/// Whether a one-page source can skip this run: the driver pinned a
/// commit and found nothing the page reads changed since the last
/// render, which costs one `dolt_log()` query. When it can, the pin is
/// recorded as read and the run's summary line comes back.
pub fn skip_if_current(ctx: &RenderCtx<'_>, provider: &str, page_uuid: &str) -> Option<String> {
    let range = ctx.raw_range();
    let (Some(pin), false) = (range.pin, range.is_stale(page_uuid)) else {
        return None;
    };
    tracing::info!(
        event = "one_page_render_skipped",
        provider,
        source = %ctx.name,
        head = %pin,
        "nothing the page reads changed since the last render",
    );
    ctx.consumed(pin);
    Some(format!("up to date at {pin}"))
}

/// Write the page to `<source>/render_markdown/index.md` and hand it,
/// with the rows `build_rows` makes for it, to the render store.
/// `build_rows` gets the page's root-relative path and the page's
/// problem list.
pub fn write_page(
    root: &Path,
    source_id: &str,
    m_uuid: &str,
    body: String,
    render_version: u32,
    build_rows: impl FnOnce(&str, &mut Vec<ProblemRow>) -> Vec<GridRow>,
    on_doc_complete: &mut dyn FnMut(RenderedMarkdown) -> Result<()>,
) -> Result<()> {
    let page_dir = datalib_etl::layout::render_markdown_root(root, source_id);
    fs::create_dir_all(&page_dir).with_context(|| format!("mkdir -p {}", page_dir.display()))?;
    let md_path = page_dir.join("index.md");
    fs::write(&md_path, body).with_context(|| format!("write {}", md_path.display()))?;

    let md_rel = md_path
        .strip_prefix(root)
        .unwrap_or(&md_path)
        .to_string_lossy()
        .into_owned();
    let mut problems: Vec<ProblemRow> = Vec::new();
    let rows = build_rows(&md_rel, &mut problems);

    on_doc_complete(RenderedMarkdown {
        markdown_uuid: m_uuid.to_string(),
        source_id: source_id.to_string(),
        // Not the raw HEAD: it moves on every ingest, and a row whose
        // content did not change may carry nothing per-run.
        upstream_cursor: None,
        bucket_key: Some(m_uuid.to_string()),
        md_path,
        render_version,
        rows,
        sections: Vec::new(),
        edges: Vec::new(),
        problems,
    })
    .with_context(|| format!("on_doc_complete {m_uuid}"))
}
