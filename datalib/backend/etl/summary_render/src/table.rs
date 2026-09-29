//! The breakdown tables a summary page is made of: a count, and bytes
//! where the source knows them, per value of one attribute.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use datalib_etl_render::html::escape_text;
use datalib_etl_render::text::{human_bytes, thousands};

/// Counts (and bytes) per label, gathered one item at a time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally {
    counts: BTreeMap<String, (i64, i64)>,
}

impl Tally {
    pub fn add(&mut self, label: impl Into<String>, count: i64, bytes: i64) {
        let slot = self.counts.entry(label.into()).or_default();
        slot.0 += count;
        slot.1 += bytes;
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    pub fn len(&self) -> usize {
        self.counts.len()
    }

    /// `(label, count, bytes)`, in label order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, i64, i64)> {
        self.counts.iter().map(|(k, &(n, b))| (k.as_str(), n, b))
    }
}

impl<S: Into<String>> FromIterator<(S, i64)> for Tally {
    fn from_iter<I: IntoIterator<Item = (S, i64)>>(iter: I) -> Self {
        let mut t = Tally::default();
        for (label, n) in iter {
            t.add(label, n, 0);
        }
        t
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Most first; ties by label.
    ByCount,
    /// By label, for a key with an order of its own (a year, a rating).
    ByLabel,
}

/// One table on the page.
#[derive(Debug, Clone)]
pub struct Breakdown {
    pub heading: &'static str,
    /// The first column's header: `Extension`, `Year`, `Camera`.
    pub key: &'static str,
    /// The count column's header: `Files`, `Photos`.
    pub noun: &'static str,
    pub tally: Tally,
    /// Whether the tally's bytes mean anything, and so get a column.
    pub bytes: bool,
    pub order: Order,
    /// At most this many rows, the rest folded into one. `None` shows
    /// every row.
    pub limit: Option<usize>,
}

impl Breakdown {
    fn rows(&self) -> Vec<(&str, i64, i64)> {
        let mut rows: Vec<(&str, i64, i64)> = self.tally.iter().collect();
        if self.order == Order::ByCount {
            rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        }
        rows
    }

    pub fn markdown(&self) -> String {
        let mut out = format!("## {}\n\n", self.heading);
        let rows = self.rows();
        let shown = self.limit.unwrap_or(rows.len()).min(rows.len());
        if shown < rows.len() {
            let _ = writeln!(out, "*The {shown} most common of {}.*\n", rows.len());
        }
        if self.bytes {
            let _ = writeln!(out, "| {} | {} | Size |", self.key, self.noun);
            out.push_str("| --- | ---: | ---: |\n");
        } else {
            let _ = writeln!(out, "| {} | {} |", self.key, self.noun);
            out.push_str("| --- | ---: |\n");
        }
        let (head, rest) = rows.split_at(shown);
        let folded = (!rest.is_empty()).then(|| {
            (
                format!("{} more", rest.len()),
                rest.iter().map(|r| r.1).sum::<i64>(),
                rest.iter().map(|r| r.2).sum::<i64>(),
            )
        });
        let cells = head
            .iter()
            .map(|&(label, n, b)| (cell(label), n, b))
            .chain(folded.map(|(label, n, b)| (format!("*{label}*"), n, b)));
        for (label, n, b) in cells {
            if self.bytes {
                let _ = writeln!(out, "| {label} | {} | {} |", thousands(n), human_bytes(b));
            } else {
                let _ = writeln!(out, "| {label} | {} |", thousands(n));
            }
        }
        out.push('\n');
        out
    }

    /// One line for the grid row's text: the heading and its first few
    /// labels, so a search for a camera or an extension lands here.
    pub fn line(&self) -> String {
        let labels: Vec<String> = self
            .rows()
            .iter()
            .take(self.limit.unwrap_or(usize::MAX))
            .map(|(label, n, _)| format!("{label} ({n})"))
            .collect();
        format!("{}: {}", self.heading, labels.join(", "))
    }
}

/// A value from the store, safe inside a table cell: no markup, no
/// column break, no line break.
pub fn cell(s: &str) -> String {
    let s = if s.is_empty() { "(blank)" } else { s };
    escape_text(s)
        .replace('|', "\\|")
        .replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tally(rows: &[(&str, i64)]) -> Tally {
        rows.iter().map(|&(l, n)| (l, n)).collect()
    }

    #[test]
    fn a_long_tail_folds_into_one_row_that_keeps_the_total() {
        let b = Breakdown {
            heading: "By extension",
            key: "Extension",
            noun: "Files",
            tally: tally(&[("txt", 5), ("md", 3), ("rs", 3), ("toml", 1), ("lock", 1)]),
            bytes: false,
            order: Order::ByCount,
            limit: Some(2),
        };
        let md = b.markdown();
        assert!(md.contains("*The 2 most common of 5.*"), "{md}");
        assert!(
            md.contains("| txt | 5 |\n| md | 3 |\n| *3 more* | 5 |"),
            "{md}"
        );
    }

    #[test]
    fn a_label_ordered_table_keeps_its_order() {
        let b = Breakdown {
            heading: "By year",
            key: "Year",
            noun: "Photos",
            tally: tally(&[("2365", 1), ("2364", 9)]),
            bytes: false,
            order: Order::ByLabel,
            limit: None,
        };
        assert!(b.markdown().contains("| 2364 | 9 |\n| 2365 | 1 |"));
    }

    /// A keyword or an album title is whatever a person typed.
    #[test]
    fn a_cell_cannot_break_the_table_or_inject_markup() {
        assert_eq!(cell("a|b\nc"), "a\\|b c");
        assert_eq!(cell("<script>"), "&lt;script&gt;");
        assert_eq!(cell(""), "(blank)");
    }
}
