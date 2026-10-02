//! Escaping for upstream text a renderer splices into its markdown.
//!
//! The UI renders every document with markdown-it `html: true`, so a
//! name or a subject written verbatim is parsed as HTML and markdown. A
//! string that is plain text upstream goes through one of these at the
//! point it becomes markup; a string the source itself authored as
//! markup (a Notion page, an email's HTML part) does not. Which helper
//! is a question of where the text lands:
//!
//! - between raw HTML tags, in an HTML block: [`escape_text`];
//! - in a double-quoted attribute: [`escape_attr`];
//! - on one markdown line — a list item, a table cell, link text, a
//!   heading, a paragraph: [`escape_md_inline`];
//! - a multi-line plain-text body: [`escape_md_block`];
//! - markdown another tool built from plain text without escaping it:
//!   [`escape_html_outside_code`];
//! - a link destination: [`md_link_dest`];
//! - inside a code span: [`md_code_span`].
//!
//! Here rather than in `datalib_etl`: that crate sits upstream of ~130
//! test targets, and only the render side writes markup.

/// Escape text that lands between tags. `&` first, or the escapes
/// this function just wrote get escaped again.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        push_html_escaped(&mut out, c);
    }
    out
}

/// Escape a value going inside a double-quoted attribute.
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("&quot;"),
            _ => push_html_escaped(&mut out, c),
        }
    }
    out
}

/// Plain text bound for one markdown line, escaped so it reads as typed:
/// HTML-escaped, every character markdown reads as inline syntax
/// backslash-escaped, and a leading character that would open a block
/// (`# heading`, `- item`, `1. item`) escaped too. A line break becomes a
/// space, since it would end the construct the text sits in.
///
/// Not for text inside a raw HTML block, where markdown is not parsed
/// and the backslashes would show: use [`escape_text`] there.
pub fn escape_md_inline(s: &str) -> String {
    let one_line = s.replace(['\r', '\n'], " ");
    escape_md_line(&one_line, true)
}

/// Multi-line plain text — a note, a description, a text message — as
/// markdown that reads as typed. Each line is HTML-escaped, and a line
/// that would open a heading, a fence, a list or a quote, or underline
/// the line above into a heading, has that marker escaped. Line breaks
/// are kept as they are. Inline emphasis (`*really*`) is left to render
/// as emphasis, which is what a person typing it meant; links and
/// images are escaped, so a sender cannot dress text up as a link.
pub fn escape_md_block(s: &str) -> String {
    s.split('\n')
        .map(|line| escape_md_line(line, false))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A URL as a markdown link destination. One that markdown would read
/// to its end is left as it is; anything else goes in `<…>`, which takes
/// spaces and parentheses, with the angle brackets and line breaks that
/// would end it percent-encoded.
pub fn md_link_dest(url: &str) -> String {
    let bare = !url
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>' | '\\'));
    if bare && !url.is_empty() {
        return url.to_string();
    }
    let mut out = String::with_capacity(url.len() + 2);
    out.push('<');
    for c in url.chars() {
        match c {
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            _ => out.push(c),
        }
    }
    out.push('>');
    out
}

/// Text inside a code span. Markdown takes a code span's content
/// literally — an entity there shows as `&lt;`, so nothing is
/// HTML-escaped — and the only way out is a backtick run, which the
/// fence is made longer than.
pub fn md_code_span(s: &str) -> String {
    let one_line = s.replace(['\r', '\n'], " ");
    let longest_run = one_line
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest_run + 1);
    // A space each side keeps a leading or trailing backtick from
    // joining the fence; markdown strips one space from each end.
    if longest_run > 0 {
        format!("{fence} {one_line} {fence}")
    } else {
        format!("{fence}{one_line}{fence}")
    }
}

/// A fenced code block around `body`, shown exactly as it is. The fence
/// is longer than any run of backticks inside, so the body cannot close
/// it early; `lang` keeps only what an info string can carry.
pub fn md_code_block(lang: &str, body: &str) -> String {
    let longest_run = body.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest_run.max(2) + 1);
    let lang: String = lang
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '`')
        .collect();
    format!("{fence}{lang}\n{body}\n{fence}")
}

/// Markdown that a converter built from plain text without escaping the
/// text — a PDF's words through pdf-inspector: `<`, `>` and `&` escaped
/// wherever markdown would read them as HTML, and left alone inside code,
/// which markdown shows literally. A line's leading `>` stays a quote.
pub fn escape_html_outside_code(md: &str) -> String {
    let mut out = String::with_capacity(md.len() + 16);
    let mut open_fence: Option<String> = None;
    for (i, line) in md.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if let Some(fence) = &open_fence {
            if closes_fence(trimmed, fence) {
                open_fence = None;
            }
            out.push_str(line);
            continue;
        }
        if let Some(fence) = fence_opener(trimmed) {
            open_fence = Some(fence);
            out.push_str(line);
            continue;
        }
        let rest = line.trim_start_matches(['>', ' ', '\t']);
        out.push_str(&line[..line.len() - rest.len()]);
        escape_html_outside_code_spans(rest, &mut out);
    }
    out
}

/// The backticks or tildes opening a fenced block, if `line` opens one.
fn fence_opener(line: &str) -> Option<String> {
    let mark = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run: String = line.chars().take_while(|&c| c == mark).collect();
    (run.len() >= 3).then_some(run)
}

fn closes_fence(line: &str, fence: &str) -> bool {
    let mark = fence.chars().next().expect("a fence has a mark");
    let run = line.chars().take_while(|&c| c == mark).count();
    run >= fence.len() && line[run..].trim().is_empty()
}

fn escape_html_outside_code_spans(line: &str, out: &mut String) {
    for (part, is_code) in code_span_parts(line) {
        if is_code {
            out.push_str(part);
        } else {
            out.push_str(&escape_text(part));
        }
    }
}

/// `text` cut into its code spans (`true`, backticks included) and the
/// text between them (`false`), in order. A backtick run with no run of
/// the same length after it opens nothing, as in markdown.
pub fn code_span_parts(text: &str) -> Vec<(&str, bool)> {
    let mut parts = Vec::new();
    let mut plain_from = 0;
    let mut at = 0;
    while let Some(i) = text[at..].find('`') {
        let open = at + i;
        let run = backtick_run(&text[open..]);
        let after = open + run;
        match closing_backtick_run(&text[after..], run) {
            Some(close) => {
                let end = after + close + run;
                if plain_from < open {
                    parts.push((&text[plain_from..open], false));
                }
                parts.push((&text[open..end], true));
                plain_from = end;
                at = end;
            }
            None => at = after,
        }
    }
    if plain_from < text.len() {
        parts.push((&text[plain_from..], false));
    }
    parts
}

fn backtick_run(s: &str) -> usize {
    s.len() - s.trim_start_matches('`').len()
}

/// Where the next run of exactly `run` backticks starts in `s`: the end
/// of a code span opened by that many.
fn closing_backtick_run(s: &str, run: usize) -> Option<usize> {
    let mut at = 0;
    while let Some(i) = s[at..].find('`') {
        let start = at + i;
        let len = backtick_run(&s[start..]);
        if len == run {
            return Some(start);
        }
        at = start + len;
    }
    None
}

fn push_html_escaped(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        _ => out.push(c),
    }
}

/// One line of plain text. `all_inline` escapes every inline marker
/// (for a name or a title, which has no emphasis to keep); otherwise
/// only the ones that make links, images, code and strikethrough.
fn escape_md_line(line: &str, all_inline: bool) -> String {
    let body = line.trim_start();
    let indent = &line[..line.len() - body.len()];
    let block_marker_at = block_marker_at(body);
    let chars: Vec<(usize, char)> = body.char_indices().collect();
    let mut out = String::with_capacity(line.len() + 8);
    out.push_str(indent);
    for (n, &(i, c)) in chars.iter().enumerate() {
        let prev = n.checked_sub(1).map(|p| chars[p].1);
        let next = chars.get(n + 1).map(|&(_, c)| c);
        let inline_syntax = match c {
            '[' | ']' | '`' => true,
            // A backslash escapes only punctuation, or breaks the line
            // at its end.
            '\\' => next.is_none_or(|n| n.is_ascii_punctuation()),
            // Strikethrough takes two.
            '~' => prev == Some('~') || next == Some('~'),
            '*' | '|' => all_inline,
            // Between two letters or digits an underscore cannot open or
            // close emphasis: `snake_case`, `:robot_face:`.
            '_' => all_inline && !(is_word(prev) && is_word(next)),
            _ => false,
        };
        if inline_syntax || Some(i) == block_marker_at {
            out.push('\\');
            out.push(c);
        } else {
            push_html_escaped(&mut out, c);
        }
    }
    out
}

fn is_word(c: Option<char>) -> bool {
    c.is_some_and(char::is_alphanumeric)
}

/// Where the one character is that would make this line open a block —
/// a heading's `#`, a list item's bullet or the `.` of its `1.`, a rule,
/// or the underline that turns the line above into a heading. `None`
/// for a line that opens nothing, like `#hashtag`, `-5°C` or `1.1937`.
fn block_marker_at(body: &str) -> Option<usize> {
    let first = body.chars().next()?;
    let ends_marker = |at: usize| {
        body[at..]
            .chars()
            .next()
            .is_none_or(|c| c == ' ' || c == '\t')
    };
    let only = |mark: char| body.trim_end().chars().all(|c| c == mark || c == ' ');
    let hashes = body.len() - body.trim_start_matches('#').len();
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    match first {
        '#' if hashes <= 6 && ends_marker(hashes) => Some(0),
        '-' | '+' | '*' if ends_marker(1) || only(first) => Some(0),
        '=' if only('=') => Some(0),
        '0'..='9'
            if digits <= 9
                && matches!(body.as_bytes().get(digits), Some(b'.' | b')'))
                && ends_marker(digits + 1) =>
        {
            Some(digits)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_escapes_markup_without_double_escaping() {
        assert_eq!(escape_text("Riker & Troi"), "Riker &amp; Troi");
        assert_eq!(escape_text("<b>hi</b>"), "&lt;b&gt;hi&lt;/b&gt;");
        // The ampersand of an escape we just emitted is not re-escaped,
        // which is what escaping `&` first buys.
        assert_eq!(escape_text("a < b & c"), "a &lt; b &amp; c");
    }

    #[test]
    fn attr_escapes_the_quote_that_would_break_out() {
        assert_eq!(
            escape_attr("https://e.com/?a=b&c=\"d\""),
            "https://e.com/?a=b&amp;c=&quot;d&quot;"
        );
    }

    #[test]
    fn an_inline_value_reads_as_typed() {
        assert_eq!(
            escape_md_inline("<script>x</script> & co"),
            "&lt;script&gt;x&lt;/script&gt; &amp; co"
        );
        assert_eq!(escape_md_inline("# Ops"), "\\# Ops");
        assert_eq!(escape_md_inline("- Bob"), "\\- Bob");
        assert_eq!(escape_md_inline("1. Picard"), "1\\. Picard");
        // Only a marker followed by a space opens anything.
        assert_eq!(escape_md_inline("#ops -5°C 1.1937"), "#ops -5°C 1.1937");
        assert_eq!(escape_md_inline("---"), "\\---");
        assert_eq!(escape_md_inline("*Bob* [x](y)"), "\\*Bob\\* \\[x\\](y)");
        assert_eq!(escape_md_inline("a | b\nc"), "a \\| b c");
        // A leading `*` is escaped once, not twice.
        assert_eq!(escape_md_inline("***"), "\\*\\*\\*");
        assert_eq!(escape_md_inline("> quote"), "&gt; quote");
        assert_eq!(escape_md_inline("Riker, Will"), "Riker, Will");
        assert_eq!(
            escape_md_inline(":robot_face: _hi_"),
            ":robot_face: \\_hi\\_"
        );
        assert_eq!(
            escape_md_inline("C:\\Users ~/x ~~y~~"),
            "C:\\Users ~/x \\~\\~y\\~\\~"
        );
    }

    #[test]
    fn a_block_keeps_its_lines_and_emphasis_but_opens_no_structure() {
        let typed =
            "Hi <b>Data</b>,\n# not a heading\n*really*\n---\n![x](https://e.invalid/p.png)";
        assert_eq!(
            escape_md_block(typed),
            "Hi &lt;b&gt;Data&lt;/b&gt;,\n\\# not a heading\n*really*\n\\---\n!\\[x\\](https://e.invalid/p.png)"
        );
    }

    #[test]
    fn converted_text_is_escaped_everywhere_but_in_code() {
        let md = "# A <b> & B\n\
                  > quoted <i>\n\
                  run `a < b && c` then <x>\n\
                  ```\n\
                  <div> & </div>\n\
                  ```\n\
                  after <y>";
        assert_eq!(
            escape_html_outside_code(md),
            "# A &lt;b&gt; &amp; B\n\
             > quoted &lt;i&gt;\n\
             run `a < b && c` then &lt;x&gt;\n\
             ```\n\
             <div> & </div>\n\
             ```\n\
             after &lt;y&gt;"
        );
    }

    #[test]
    fn a_link_destination_cannot_be_closed_early() {
        assert_eq!(
            md_link_dest("https://e.invalid/a?b=1&c=2"),
            "https://e.invalid/a?b=1&c=2"
        );
        assert_eq!(
            md_link_dest("https://e.invalid/a_(b)"),
            "<https://e.invalid/a_(b)>"
        );
        assert_eq!(
            md_link_dest("https://e.invalid/a b"),
            "<https://e.invalid/a b>"
        );
        assert_eq!(
            md_link_dest("https://e.invalid/<x>"),
            "<https://e.invalid/%3Cx%3E>"
        );
    }

    #[test]
    fn a_code_block_outlasts_the_fences_inside_it() {
        assert_eq!(md_code_block("json", "{}"), "```json\n{}\n```");
        assert_eq!(
            md_code_block("rust `x`\n", "```\n</details>\n```"),
            "````rust\n```\n</details>\n```\n````"
        );
    }

    #[test]
    fn a_code_span_outlasts_the_backticks_inside_it() {
        assert_eq!(md_code_span("<id>"), "`<id>`");
        assert_eq!(md_code_span("a`b"), "`` a`b ``");
    }
}
