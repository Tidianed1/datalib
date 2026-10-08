// What a grid row answers to beyond its own columns' search keys: its ids,
// the people it names, its title, its names. One row of the terms file per
// term, so a pasted id or address is one lookup, whatever column holds it.
// The file, and why it is plain SQLite beside the grid index rather than a
// table in it: `docs/dev/plans/search_tabs.md` § "`grid_row_terms`".

/// What a term is to its row. Stored as its `as_str` spelling.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    strum::EnumString,
    strum::IntoStaticStr,
    strum::VariantArray,
)]
#[strum(serialize_all = "snake_case")]
pub enum TermKind {
    /// The row's own uuid.
    Id,
    /// The uuid of something the row is in: its conversation, its
    /// document, its Notion page.
    Container,
    /// The handle of the row's author.
    From,
    /// The title of the row's conversation or document.
    Title,
    /// A name the row shows: its author, its channel, its account.
    Name,
}

impl TermKind {
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// `None` for a spelling this build does not know.
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    /// How strongly a match in this kind says the row is the one meant:
    /// its own id beats its author, which beats what contains it, its
    /// title, and last a name it shows.
    pub fn affinity(self) -> u8 {
        match self {
            TermKind::Id => 5,
            TermKind::From => 4,
            TermKind::Container => 3,
            TermKind::Title => 2,
            TermKind::Name => 1,
        }
    }
}

/// The columns of a `grid_rows` row its terms come from.
#[derive(Debug, Clone, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct TermSource {
    pub uuid: String,
    pub conversation_uuid: String,
    pub markdown_uuid: Option<String>,
    pub notion_page_uuid: Option<String>,
    pub author_handle: Option<String>,
    pub conversation_name: Option<String>,
    pub author: Option<String>,
    pub channel: Option<String>,
    pub account: Option<String>,
    pub touched_at_utc: Option<String>,
}

impl TermSource {
    /// The `SELECT` list that reads one from `grid_rows`.
    pub const COLUMNS: &'static str = "uuid, conversation_uuid, markdown_uuid, notion_page_uuid, \
         author_handle, conversation_name, author, channel, account, touched_at_utc";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub kind: TermKind,
    pub value: String,
}

/// Every term `row` answers to, each `(kind, value)` once and in kind
/// order. An id is a `Container` only when it is not the row's own, and
/// an empty value is no term.
pub fn terms_of(row: &TermSource) -> Vec<Term> {
    let containers = [
        Some(row.conversation_uuid.as_str()),
        row.markdown_uuid.as_deref(),
        row.notion_page_uuid.as_deref(),
    ];
    let names = [
        row.author.as_deref(),
        row.channel.as_deref(),
        row.account.as_deref(),
    ];
    let candidates = std::iter::once((TermKind::Id, Some(row.uuid.as_str())))
        .chain(
            containers
                .into_iter()
                .filter(|id| *id != Some(row.uuid.as_str()))
                .map(|id| (TermKind::Container, id)),
        )
        .chain(std::iter::once((
            TermKind::From,
            row.author_handle.as_deref(),
        )))
        .chain(std::iter::once((
            TermKind::Title,
            row.conversation_name.as_deref(),
        )))
        .chain(names.into_iter().map(|name| (TermKind::Name, name)));
    let mut out: Vec<Term> = Vec::new();
    for (kind, value) in candidates {
        let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        if !out.iter().any(|t| t.kind == kind && t.value == value) {
            out.push(Term {
                kind,
                value: value.to_string(),
            });
        }
    }
    out
}

/// What [`terms_of`] derives and how the file lays it out. A file built
/// under another shape is rebuilt whole, so change it whenever either
/// changes.
pub const TERMS_SHAPE: &str = "1";

/// The terms file's tables. The FTS5 index holds only each term's value,
/// linked to its row in `terms` by rowid, and keeps `@ . - _ + :` inside a
/// token so an id or an address is one token
/// (`docs/dev/doltlite.md` § "Full-text search (FTS5)").
pub const TERMS_DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS terms (term_id INTEGER PRIMARY KEY, uuid TEXT NOT NULL, \
     kind TEXT NOT NULL, value TEXT NOT NULL, touched_at_utc TEXT)",
    "CREATE INDEX IF NOT EXISTS terms_by_uuid ON terms (uuid)",
    "CREATE VIRTUAL TABLE IF NOT EXISTS terms_fts USING fts5(value, content='', \
     contentless_delete=1, tokenize=\"unicode61 tokenchars '@.-_+:'\")",
    "CREATE TABLE IF NOT EXISTS terms_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
];

/// The `terms_meta` key naming the grid index commit the terms reflect.
pub const META_GRID_COMMIT: &str = "grid_commit";
/// The `terms_meta` key naming the [`TERMS_SHAPE`] the file was built under.
pub const META_SHAPE: &str = "shape";

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> TermSource {
        TermSource {
            uuid: "m-1".into(),
            conversation_uuid: "c-1".into(),
            markdown_uuid: Some("c-1".into()),
            author_handle: Some("email:ann@example.com".into()),
            conversation_name: Some("Away team roster".into()),
            author: Some("Ann".into()),
            channel: Some("".into()),
            ..TermSource::default()
        }
    }

    fn pairs(terms: &[Term]) -> Vec<(&str, &str)> {
        terms
            .iter()
            .map(|t| (t.kind.as_str(), t.value.as_str()))
            .collect()
    }

    #[test]
    fn a_message_answers_to_its_id_its_conversation_its_author_and_its_title() {
        assert_eq!(
            pairs(&terms_of(&row())),
            [
                ("id", "m-1"),
                ("container", "c-1"),
                ("from", "email:ann@example.com"),
                ("title", "Away team roster"),
                ("name", "Ann"),
            ]
        );
    }

    /// A conversation's own row is its conversation and its document: one
    /// id, not three.
    #[test]
    fn a_document_row_is_not_its_own_container() {
        let doc = TermSource {
            uuid: "c-1".into(),
            ..row()
        };
        let terms = terms_of(&doc);
        assert_eq!(
            terms.iter().filter(|t| t.value == "c-1").count(),
            1,
            "{terms:?}"
        );
        assert_eq!(terms[0].kind, TermKind::Id);
    }

    #[test]
    fn every_kind_reads_back_by_its_spelling() {
        use strum::VariantArray;
        for kind in TermKind::VARIANTS {
            assert_eq!(TermKind::parse(kind.as_str()), Some(*kind));
        }
        assert_eq!(TermKind::parse("bcc"), None);
    }
}
