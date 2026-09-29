//! A media store as one page: how many files of which kind, how big,
//! when the pictures were taken and on what, whose music it is.

use std::path::Path;

use anyhow::Result;
use datalib_etl_render::grid_index::RenderedMarkdown;
use datalib_etl_summary_render::{render_page, Profile};
use datalib_id::IdNamespace;
use datalib_schema::providers::Provider;

pub mod parse;
pub mod summary;

/// Bump when the page or its grid row changes shape enough that an
/// existing page must be rendered again.
pub const RENDER_VERSION: u32 = 1;

pub const PROFILE: Profile = Profile {
    provider: Provider::Media,
    id_namespace: IdNamespace::Media,
    kind: "Media Library",
    source_label: "Media",
    title_prefix: "Media",
    render_version: RENDER_VERSION,
};

pub fn document_uuid(source_id: &str) -> String {
    PROFILE.document_uuid(source_id)
}

/// Write the page for `parsed` and hand it to `on_doc_complete`;
/// returns the page's item count.
pub fn render_all(
    parsed: &parse::Parsed,
    root: &Path,
    source_id: &str,
    on_doc_complete: &mut dyn FnMut(RenderedMarkdown) -> Result<()>,
) -> Result<i64> {
    let summary = summary::summarize(parsed);
    render_page(&PROFILE, &summary, root, source_id, on_doc_complete)?;
    Ok(summary.item_count)
}
