//! Rewrite a TOML array of strings where it stands, keeping how it was
//! written: on one line, or one id per line with its indentation, its
//! trailing comma and the comments and blank lines between the ids. The
//! UI edits the fan-ins' `inputs` the same way
//! (`datalib/ui/src/config/tomlText.ts`).

use anyhow::{Context as _, Result};

enum Entry {
    Id {
        /// As written, quotes and all.
        token: String,
        /// The string it spells; `None` for anything that is not one.
        value: Option<String>,
        /// The whitespace before it when it starts a line; `None` when it
        /// follows something else on its line.
        indent: Option<String>,
        /// A comment after it on its line, with the space before the `#`.
        note: String,
    },
    Comment {
        indent: String,
        text: String,
    },
    Blank,
}

struct Scanned {
    entries: Vec<Entry>,
    /// A comment on the `[` line, with the space before it.
    head: String,
    multiline: bool,
    trailing_comma: bool,
    /// The indentation of a `]` on its own line; `None` when it closes the
    /// last line of entries.
    close: Option<String>,
}

/// `written` is one whole array, `[` to `]`, as the file has it. The
/// values of `after` that it already holds keep their place, and the
/// rest go at the end.
pub fn edit_string_array(written: &str, after: &[String]) -> Result<String> {
    let scanned = scan(written).with_context(|| format!("read the array {written:?}"))?;
    let before: Vec<&str> = scanned
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Id { value: Some(v), .. } => Some(v.as_str()),
            _ => None,
        })
        .collect();
    if before == after {
        return Ok(written.to_string());
    }
    let added: Vec<Entry> = after
        .iter()
        .filter(|v| !before.contains(&v.as_str()))
        .map(|v| Entry::Id {
            token: quote(v),
            value: Some(v.clone()),
            indent: None,
            note: String::new(),
        })
        .collect();
    let kept = scanned.entries.iter().filter(|e| match e {
        Entry::Id { value: Some(v), .. } => after.contains(v),
        _ => true,
    });
    let entries: Vec<&Entry> = kept.chain(&added).collect();
    Ok(render(&scanned, &entries))
}

fn scan(written: &str) -> Option<Scanned> {
    let inner = written.strip_prefix('[')?;
    let mut s = Scanned {
        entries: Vec::new(),
        head: String::new(),
        multiline: false,
        trailing_comma: false,
        close: None,
    };
    // Only whitespace so far on a line after the first.
    let mut line_start = false;
    let mut space = String::new();
    // The index of the id on the current line, for a note after it.
    let mut on_this_line: Option<usize> = None;
    let mut i = 0;
    while let Some(c) = inner[i..].chars().next() {
        match c {
            ']' => {
                s.close = line_start.then_some(space);
                return Some(s);
            }
            ' ' | '\t' => {
                space.push(c);
                i += 1;
            }
            '\r' => i += 1,
            '\n' => {
                if line_start {
                    s.entries.push(Entry::Blank);
                }
                s.multiline = true;
                line_start = true;
                space.clear();
                on_this_line = None;
                i += 1;
            }
            '#' => {
                let stop = inner[i..].find('\n').map_or(inner.len(), |n| i + n);
                let comment = inner[i..stop].trim_end_matches('\r').to_string();
                match on_this_line {
                    Some(at) => {
                        if let Entry::Id { note, .. } = &mut s.entries[at] {
                            *note = format!("{space}{comment}");
                        }
                    }
                    None if !s.multiline => s.head = format!("{space}{comment}"),
                    None => s.entries.push(Entry::Comment {
                        indent: std::mem::take(&mut space),
                        text: comment,
                    }),
                }
                line_start = false;
                space.clear();
                i = stop;
            }
            ',' => {
                s.trailing_comma = true;
                line_start = false;
                space.clear();
                i += 1;
            }
            _ => {
                let stop = i + token_len(&inner[i..]);
                let token = inner[i..stop].to_string();
                on_this_line = Some(s.entries.len());
                s.entries.push(Entry::Id {
                    value: value_of(&token),
                    token,
                    indent: line_start.then(|| space.clone()),
                    note: String::new(),
                });
                s.trailing_comma = false;
                line_start = false;
                space.clear();
                i = stop;
            }
        }
    }
    None
}

/// How long the value at the start of `rest` is: through its closing
/// quote for a string, else up to the next separator.
fn token_len(rest: &str) -> usize {
    for q in ["\"\"\"", "'''", "\"", "'"] {
        if !rest.starts_with(q) {
            continue;
        }
        let bytes = rest.as_bytes();
        let mut j = q.len();
        while j < bytes.len() && !bytes[j..].starts_with(q.as_bytes()) {
            j += if q.starts_with('"') && bytes[j] == b'\\' {
                2
            } else {
                1
            };
        }
        return (j + q.len()).min(rest.len());
    }
    rest.find([' ', '\t', '\r', '\n', ',', '#', ']'])
        .unwrap_or(rest.len())
}

fn value_of(token: &str) -> Option<String> {
    let table: toml::Table = toml::from_str(&format!("v = {token}")).ok()?;
    match table.get("v")? {
        toml::Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn render(s: &Scanned, entries: &[&Entry]) -> String {
    let tokens: Vec<&str> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Id { token, .. } => Some(token.as_str()),
            _ => None,
        })
        .collect();
    if !s.multiline {
        return format!("[{}]", tokens.join(", "));
    }
    let had_ids = s.entries.iter().any(|e| matches!(e, Entry::Id { .. }));
    let trailing_comma = !had_ids || s.trailing_comma;
    let indent = s
        .entries
        .iter()
        .find_map(|e| match e {
            Entry::Id { indent, .. } => indent.clone(),
            Entry::Comment { indent, .. } => Some(indent.clone()),
            Entry::Blank => None,
        })
        .unwrap_or_else(|| format!("{}  ", s.close.as_deref().unwrap_or("")));
    let id_count = tokens.len();
    let mut seen = 0;
    let lines: Vec<String> = entries
        .iter()
        .map(|e| match e {
            Entry::Blank => String::new(),
            Entry::Comment { indent, text } => format!("{indent}{text}"),
            Entry::Id {
                token,
                indent: own,
                note,
                ..
            } => {
                seen += 1;
                let comma = if seen < id_count || trailing_comma {
                    ","
                } else {
                    ""
                };
                format!("{}{token}{comma}{note}", own.as_deref().unwrap_or(&indent))
            }
        })
        .collect();
    let open = format!("[{}\n", s.head);
    // A `]` after a comment would be part of it.
    let close_inline = s.close.is_none()
        && matches!(entries.last(), Some(Entry::Id { note, .. }) if note.is_empty());
    if close_inline {
        return format!("{open}{}]", lines.join("\n"));
    }
    let close = format!("{}]", s.close.as_deref().unwrap_or(""));
    if lines.is_empty() {
        format!("{open}{close}")
    } else {
        format!("{open}{}\n{close}", lines.join("\n"))
    }
}

fn quote(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(written: &str, after: &[&str]) -> String {
        let after: Vec<String> = after.iter().map(|s| s.to_string()).collect();
        edit_string_array(written, &after).unwrap()
    }

    /// #897: the upgrade wrote every `inputs` it touched on one line.
    #[test]
    fn one_id_per_line_stays_one_id_per_line() {
        let written = "[\n    \"a/render_markdown\",\n    \"b/render_markdown\",\n]";
        assert_eq!(
            edit(written, &["a/keyword_index", "b/keyword_index"]),
            "[\n    \"a/keyword_index\",\n    \"b/keyword_index\",\n]"
        );
        assert_eq!(
            edit(written, &["a/render_markdown"]),
            "[\n    \"a/render_markdown\",\n]"
        );
    }

    #[test]
    fn no_trailing_comma_stays_without_one() {
        let written = "[\n  \"a\",\n  \"b\"\n]";
        assert_eq!(
            edit(written, &["a", "b", "c"]),
            "[\n  \"a\",\n  \"b\",\n  \"c\"\n]"
        );
        assert_eq!(edit(written, &["b"]), "[\n  \"b\"\n]");
    }

    #[test]
    fn comments_and_blank_lines_survive() {
        let written =
            "[ # every source\n  # the crew\n  \"a\", # logs [old]\n\n  \"b\",\n  # \"c\",\n]";
        assert_eq!(
            edit(written, &["b", "d"]),
            "[ # every source\n  # the crew\n\n  \"b\",\n  # \"c\",\n  \"d\",\n]"
        );
        assert_eq!(
            edit(written, &["a", "b", "d"]),
            "[ # every source\n  # the crew\n  \"a\", # logs [old]\n\n  \"b\",\n  # \"c\",\n  \"d\",\n]"
        );
    }

    #[test]
    fn one_line_stays_one_line() {
        assert_eq!(edit("[\"a\", 'b']", &["b", "c"]), "['b', \"c\"]");
        assert_eq!(edit("[]", &["a"]), "[\"a\"]");
    }

    #[test]
    fn an_unchanged_array_is_left_as_written() {
        let written = "[ \"a\" ,'b' ]";
        assert_eq!(edit(written, &["a", "b"]), written);
    }

    #[test]
    fn a_close_after_the_last_id_stays_there() {
        assert_eq!(edit("[\n  \"a\",\n  \"b\"]", &["a"]), "[\n  \"a\"]");
        // …unless a comment would swallow it.
        assert_eq!(edit("[\n  # x\n  \"a\"]", &[]), "[\n  # x\n]");
    }
}
