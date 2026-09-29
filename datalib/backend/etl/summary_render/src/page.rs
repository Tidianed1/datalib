//! The summary page and its grid row, from a [`Summary`].

use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use datalib_etl::title::Title;
use datalib_etl_render::grid_index::RenderedMarkdown;
use datalib_etl_render::one_page::write_page;
use datalib_etl_render::text::yaml_safe;
use datalib_id::{entity_id_str, IdNamespace};
use datalib_schema::grid_rows::GridRow;
use datalib_schema::problems::{Outcome, Problem, ProblemRow, Scope, Stage};
use datalib_schema::providers::Provider;

use crate::table::{cell, Breakdown};

/// The page's `upstream_entity_kind`: the id's kind component.
const KIND_PAGE: &str = "summary";

/// How one provider names its page.
pub struct Profile {
    pub provider: Provider,
    pub id_namespace: IdNamespace,
    /// The grid's Kind column.
    pub kind: &'static str,
    pub source_label: &'static str,
    /// The page is titled `<title_prefix> — <source id>`.
    pub title_prefix: &'static str,
    pub render_version: u32,
}

impl Profile {
    /// The page's `markdown_uuid`. One page per source with nothing
    /// upstream behind it, so its key is the source id. No stamp: the
    /// page's `created_at` is its earliest item, which moves when an
    /// older one arrives.
    pub fn document_uuid(&self, source_id: &str) -> String {
        entity_id_str(
            self.id_namespace,
            source_id,
            None,
            KIND_PAGE,
            source_id,
            None,
        )
    }

    fn title(&self, source_id: &str) -> String {
        format!("{} — {source_id}", self.title_prefix)
    }
}

/// What the page says about a source.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    /// How many of the things this source is about it holds: the page
    /// row's `item_count`.
    pub item_count: i64,
    /// The sentence under the title.
    pub lede: String,
    /// A two-column table under the lede: what was scanned, totals.
    pub facts: Vec<(String, String)>,
    pub breakdowns: Vec<Breakdown>,
    /// The earliest and latest dates in the source, as it wrote them.
    /// Only an offset-bearing one reaches the grid row's stamps.
    pub earliest: Option<String>,
    pub latest: Option<String>,
    /// What the page could not say — a table it expected and did not
    /// find. The page is still written; these go on its problem rows.
    pub problems: Vec<Problem>,
}

pub fn render_page(
    profile: &Profile,
    summary: &Summary,
    root: &Path,
    source_id: &str,
    on_doc_complete: &mut dyn FnMut(RenderedMarkdown) -> Result<()>,
) -> Result<()> {
    let m_uuid = profile.document_uuid(source_id);
    write_page(
        root,
        source_id,
        &m_uuid,
        markdown(profile, summary, source_id, &m_uuid),
        profile.render_version,
        |md_rel, problems| grid_rows(profile, summary, source_id, &m_uuid, md_rel, problems),
        on_doc_complete,
    )
}

/// A source's own stamp, when it carries an offset; a naive one (a
/// Lightroom `captureTime`) is shown on the page and nowhere else.
fn stamp(s: &Option<String>) -> Option<String> {
    s.as_deref().and_then(datalib_time::coerce_record_stamp)
}

pub fn markdown(profile: &Profile, summary: &Summary, source_id: &str, m_uuid: &str) -> String {
    let title = profile.title(source_id);
    let mut out = String::with_capacity(4 * 1024);
    out.push_str("---\n");
    let _ = writeln!(out, "markdown_uuid: {m_uuid}");
    let _ = writeln!(out, "source_id: {source_id}");
    let _ = writeln!(out, "provider: {}", profile.provider.as_str());
    let _ = writeln!(out, "title: {}", yaml_safe(&title));
    if let Some(ts) = stamp(&summary.earliest) {
        let _ = writeln!(out, "created_at: {}", yaml_safe(&ts));
    }
    if let Some(ts) = stamp(&summary.latest) {
        let _ = writeln!(out, "modified_at: {}", yaml_safe(&ts));
    }
    out.push_str("---\n\n");
    out.push_str(
        &Title {
            suffix: None,
            text: &title,
            markdown_uuid: Some(m_uuid),
            source_url: None,
        }
        .render(),
    );
    let _ = write!(out, "{}\n\n", summary.lede);
    if !summary.facts.is_empty() {
        out.push_str("| | |\n| --- | --- |\n");
        for (label, value) in &summary.facts {
            let _ = writeln!(out, "| {label} | {} |", cell(value));
        }
        out.push('\n');
    }
    for b in summary.breakdowns.iter().filter(|b| !b.tally.is_empty()) {
        out.push_str(&b.markdown());
    }
    out
}

/// The page's one row. A row that will not validate is dropped and
/// recorded on `problems` rather than failing the source's render.
fn grid_rows(
    profile: &Profile,
    summary: &Summary,
    source_id: &str,
    m_uuid: &str,
    md_rel: &str,
    problems: &mut Vec<ProblemRow>,
) -> Vec<GridRow> {
    let version = profile.render_version;
    problems.extend(summary.problems.iter().map(|p| {
        ProblemRow::new(
            source_id,
            Stage::Render,
            Scope::Markdown(m_uuid),
            None,
            Outcome::Nulled,
            p.clone(),
            Some(version),
        )
    }));
    let title = profile.title(source_id);
    let mut text = format!("{title}\n{}", summary.lede);
    for b in summary.breakdowns.iter().filter(|b| !b.tally.is_empty()) {
        text.push('\n');
        text.push_str(&b.line());
    }
    GridRow::builder()
        .uuid(m_uuid.to_string())
        .provider(profile.provider)
        .kind(profile.kind)
        .source_label(profile.source_label)
        .is_document(true)
        .item_count(Some(summary.item_count))
        .created_at(stamp(&summary.earliest))
        .modified_at(stamp(&summary.latest))
        .conversation_name(Some(title))
        .conversation_uuid(m_uuid.to_string())
        .entire_chat(format!("/chat/{m_uuid}"))
        .body(text)
        .qmd_path(Some(md_rel.to_string()))
        .markdown_uuid(Some(m_uuid.to_string()))
        .upstream_id(Some(source_id.to_string()))
        .upstream_entity_kind(Some(KIND_PAGE.to_string()))
        .build_or_record(source_id, m_uuid, version, problems)
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use datalib_schema::problems::Reason;

    const PROFILE: Profile = Profile {
        provider: Provider::Test,
        id_namespace: IdNamespace::Datalib,
        kind: "Test Summary",
        source_label: "Test",
        title_prefix: "Things",
        render_version: 1,
    };

    fn rows_of(summary: &Summary) -> (Vec<GridRow>, Vec<ProblemRow>) {
        let mut problems = Vec::new();
        let rows = grid_rows(
            &PROFILE,
            summary,
            "src",
            "m-1",
            "src/index.md",
            &mut problems,
        );
        (rows, problems)
    }

    #[test]
    fn the_page_row_is_the_document_and_counts_the_items() {
        let (rows, problems) = rows_of(&Summary {
            item_count: 42,
            lede: "42 things.".into(),
            ..Default::default()
        });
        assert!(problems.is_empty(), "{problems:?}");
        let [row] = rows.as_slice() else {
            panic!("one row, got {rows:?}")
        };
        assert!(row.is_document);
        assert_eq!(row.item_count, Some(42));
        assert_eq!(row.upstream_id.as_deref(), Some("src"));
        assert_eq!(row.upstream_entity_kind.as_deref(), Some(KIND_PAGE));
    }

    /// A Lightroom `captureTime` has no offset; it must not reach the
    /// row, where a naive stamp fails the row's validation.
    #[test]
    fn only_an_offset_bearing_date_becomes_the_rows_stamp() {
        let (rows, _) = rows_of(&Summary {
            earliest: Some("2364-03-12T09:15:00".into()),
            latest: Some("2364-03-14T06:00:00-07:00".into()),
            ..Default::default()
        });
        assert_eq!(rows[0].created_at, None);
        assert_eq!(
            rows[0].modified_at.as_deref(),
            Some("2364-03-14T06:00:00-07:00")
        );
    }

    #[test]
    fn a_summary_problem_lands_on_the_page() {
        let (_, problems) = rows_of(&Summary {
            problems: vec![Problem::field(
                "Adobe_images",
                Reason::NotFound,
                "no such table",
            )],
            ..Default::default()
        });
        let [p] = problems.as_slice() else {
            panic!("{problems:?}")
        };
        assert_eq!(p.scope_key, "m-1");
        assert_eq!(p.stage, Stage::Render);
        assert_eq!(p.field.as_deref(), Some("Adobe_images"));
    }
}
