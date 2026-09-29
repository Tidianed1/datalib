// Rendered markdown as the words a person reads in a one-line cell: the
// grid's Contents column, and a qmd hit's snippet. Markup goes — tags,
// images, link targets, heading and quote marks, emphasis, code fences,
// entities — and every run of whitespace, non-breaking included, is one
// space.

/// The words of `markdown` on one line. Reading stops once more than
/// `limit` characters are in hand, so a long body costs only its start.
///
/// A `<details>` block reads as its summary, the way the page shows it
/// folded — a reply's Thinking stays out of the reply — unless the block
/// is all there is, as on a tool call's own row, where its contents are
/// the point.
pub fn plain_text(markdown: &str, limit: usize) -> String {
    let mut out = Words::default();
    let mut folded: Option<Folded> = None;
    let mut blocks = 0;
    let mut sole_body: Option<Words> = None;
    let mut outside = false;
    let mut fold = |block: Folded, out: &mut Words, blocks: usize, outside: bool| {
        out.push(&block.summary);
        if blocks == 1 && !outside {
            sole_body = Some(block.body);
        }
    };
    for line in markdown.lines() {
        if out.chars > limit {
            break;
        }
        let mut rest = line.trim_start();
        if folded.is_none() {
            let Some(opened) = rest.strip_prefix("<details>") else {
                if let Some(text) = line_text(line) {
                    outside |= out.push(&text);
                }
                continue;
            };
            blocks += 1;
            let (summary, after) = summary_of(opened);
            folded = Some(Folded {
                summary: line_text(summary).unwrap_or_default(),
                body: Words::default(),
            });
            rest = after;
        }
        let block = folded.as_mut().expect("inside a block");
        let (inside, closed) = match rest.find("</details>") {
            Some(at) => (&rest[..at], true),
            None => (rest, false),
        };
        if block.body.chars <= limit {
            if let Some(text) = line_text(inside) {
                block.body.push(&text);
            }
        }
        if closed {
            fold(folded.take().unwrap(), &mut out, blocks, outside);
        }
    }
    // A block the text ends inside — cut off — reads as closed.
    if let Some(block) = folded.take() {
        fold(block, &mut out, blocks, outside);
    }
    if let (1, false, Some(body)) = (blocks, outside, sole_body) {
        out.push(&body.text);
    }
    out.text
}

/// `<summary>…</summary>` at the start of a `<details>` line, and what
/// follows it.
fn summary_of(opened: &str) -> (&str, &str) {
    const OPEN: &str = "<summary>";
    const CLOSE: &str = "</summary>";
    match (opened.find(OPEN), opened.find(CLOSE)) {
        (Some(a), Some(b)) if a < b => (&opened[a + OPEN.len()..b], &opened[b + CLOSE.len()..]),
        _ => ("", opened),
    }
}

struct Folded {
    summary: String,
    body: Words,
}

#[derive(Default)]
struct Words {
    text: String,
    chars: usize,
}

impl Words {
    /// Appends `text`'s words, one space apart; whether there were any.
    fn push(&mut self, text: &str) -> bool {
        let mut any = false;
        for word in text.split_whitespace() {
            if !self.text.is_empty() {
                self.text.push(' ');
                self.chars += 1;
            }
            self.text.push_str(word);
            self.chars += word.chars().count();
            any = true;
        }
        any
    }
}

fn line_text(line: &str) -> Option<String> {
    let line = line.trim();
    if line.starts_with("```") || line.starts_with("~~~") {
        return None;
    }
    if matches!(line, "---" | "***" | "___") {
        return None;
    }
    let line = without_block_marks(line);
    let line = unescaped(line);
    let line = without_images(&line);
    let line = without_link_targets(&line);
    let line = without_tags(&line);
    Some(without_emphasis(&line))
}

/// A quote's `>`s, then a heading's `#`s — only where one is followed by a
/// space, so `#general` and `#4` stay words.
fn without_block_marks(line: &str) -> &str {
    let mut rest = line;
    while let Some(r) = rest.strip_prefix('>') {
        rest = r.trim_start();
    }
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    if (1..=6).contains(&hashes) && rest[hashes..].starts_with(' ') {
        rest = rest[hashes..].trim_start();
    }
    rest
}

/// The rendered bodies carry escaped HTML (`&lt;br&gt;`) as well as real
/// tags; unescaping first lets one pass strip both. `&amp;` goes last so
/// `&amp;lt;` stays the text `&lt;`.
fn unescaped(line: &str) -> String {
    line.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// `![alt](target)` says nothing in a cell — mostly a logo — and goes whole.
fn without_images(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find("![") {
        let Some(end) = link_end(&rest[at + 1..]) else {
            break;
        };
        out.push_str(&rest[..at]);
        rest = &rest[at + 1 + end..];
    }
    out.push_str(rest);
    out
}

/// `[text](target)` keeps its text; a bare ` (https://…)` goes too.
fn without_link_targets(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find('[') {
        match link_end(&rest[at..]) {
            Some(end) => {
                let link = &rest[at..at + end];
                let text = &link[1..link.find("](").unwrap_or(1)];
                out.push_str(&rest[..at]);
                out.push_str(text);
                rest = &rest[at + end..];
            }
            None => {
                out.push_str(&rest[..=at]);
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    let mut bare = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(at) = rest.find("(http") {
        let Some(close) = rest[at..].find(')') else {
            break;
        };
        bare.push_str(rest[..at].trim_end());
        rest = &rest[at + close + 1..];
    }
    bare.push_str(rest);
    bare
}

/// The byte length of the `[text](target)` that `s` opens with, if it
/// opens with one.
fn link_end(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    if !s[i + 1..].starts_with('(') {
                        return None;
                    }
                    let close = s[i + 1..].find(')')?;
                    return Some(i + 1 + close + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Tags go; one that breaks a line (`<br>`, `</p>`, `<div …>`) leaves a
/// space so the words either side do not run together. Only what reads as
/// a tag — `<name`, `</name`, `<!` — so `Worf <worf@enterprise.test>`
/// keeps its address.
fn without_tags(line: &str) -> String {
    const BREAKS: &[&str] = &[
        "br", "p", "div", "li", "tr", "td", "th", "h1", "h2", "h3", "h4", "h5", "h6", "summary",
        "details",
    ];
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let Some(name) = tag_name(after) else {
            out.push('<');
            rest = after;
            continue;
        };
        if BREAKS.contains(&name.to_ascii_lowercase().as_str()) {
            out.push(' ');
        }
        // A tag the line cut off — qmd's snippets end mid-word — goes too.
        rest = after.find('>').map_or("", |close| &after[close + 1..]);
    }
    out.push_str(rest);
    out
}

fn tag_name(after_lt: &str) -> Option<&str> {
    if after_lt.starts_with('!') {
        return Some("");
    }
    let s = after_lt.strip_prefix('/').unwrap_or(after_lt);
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or(s.len());
    let name = &s[..end];
    let next = s[end..].chars().next();
    let named = name.starts_with(|c: char| c.is_ascii_alphabetic());
    (named && matches!(next, None | Some('>' | '/' | ' ' | '\t'))).then_some(name)
}

/// `**strong**`, `__strong__`, `` `code` `` and `*emphasis*`, and a
/// backslash escape. A lone `*` with space both sides is arithmetic and
/// stays; `_emphasis_` is left alone, since snake_case is far commoner.
fn without_emphasis(line: &str) -> String {
    let line = line.replace("**", "").replace("__", "").replace('`', "");
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let before = i.checked_sub(1).map(|j| chars[j]);
        let after = chars.get(i + 1).copied();
        match c {
            '\\' if after.is_some_and(|a| a.is_ascii_punctuation()) => {
                out.push(after.unwrap());
                i += 2;
                continue;
            }
            '*' => {
                let opens = before.is_none_or(char::is_whitespace)
                    && after.is_some_and(|a| !a.is_whitespace());
                let closes = before.is_some_and(|b| !b.is_whitespace())
                    && after.is_none_or(|a| a.is_whitespace() || a.is_ascii_punctuation());
                if !(opens || closes) {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::plain_text;

    fn text(md: &str) -> String {
        plain_text(md, usize::MAX)
    }

    /// An email opens on its sender's logo, as an image inside a link.
    #[test]
    fn an_image_goes_whole_and_a_link_keeps_its_words() {
        assert_eq!(
            text(
                "[![Starfleet](https://sf.test/logo.png)](https://sf.test)\n\n\
                 # Your shore leave is approved\n\n\
                 Report to [Transporter Room 3](https://sf.test/tr3?a=1&amp;b=2) at **0900**."
            ),
            "Your shore leave is approved Report to Transporter Room 3 at 0900."
        );
    }

    /// A reply's reasoning is folded on the page, and in the cell.
    #[test]
    fn a_folded_block_beside_other_text_reads_as_its_summary() {
        assert_eq!(
            text(
                "Plot a course.\n<details><summary>Thinking</summary>\n\n\
                 > The Neutral Zone is closer.\n\n</details>\n\nCourse laid in."
            ),
            "Plot a course. Thinking Course laid in."
        );
    }

    /// A tool call's own row is nothing but the folded block, so the
    /// block's contents are what it says.
    #[test]
    fn a_folded_tool_call_reads_as_its_summary_and_arguments() {
        assert_eq!(
            text(
                "<details><summary>Tool use: Bash</summary>\n\n```json\n\
                 {\n  \"command\": \"ls /holodeck\"\n}\n```\n\n</details>"
            ),
            "Tool use: Bash { \"command\": \"ls /holodeck\" }"
        );
    }

    #[test]
    fn quote_and_heading_marks_go_but_a_hash_word_stays() {
        assert_eq!(
            text("> > quoted\n## When\n#bridge #4 - Deck 12"),
            "quoted When #bridge #4 - Deck 12"
        );
    }

    #[test]
    fn an_address_in_angle_brackets_is_not_a_tag() {
        assert_eq!(
            text("Worf <worf@enterprise.test><br>a <b>bold</b> 3 < 4"),
            "Worf <worf@enterprise.test> a bold 3 < 4"
        );
    }

    #[test]
    fn emphasis_marks_go_and_arithmetic_stays() {
        assert_eq!(
            text("*Riker commented on his own photo.* 2 * 3 is `six`, \\[file\\] snake_case"),
            "Riker commented on his own photo. 2 * 3 is six, [file] snake_case"
        );
    }

    /// Non-breaking spaces from HTML mail are spaces.
    #[test]
    fn every_run_of_whitespace_is_one_space() {
        assert_eq!(
            text("Data\u{a0}\u{a0} has\n\n\taccepted"),
            "Data has accepted"
        );
    }

    /// A body of many thousand lines is read only as far as the cell needs.
    #[test]
    fn reading_stops_past_the_limit() {
        let body = "Engage.\n".repeat(10_000);
        let out = plain_text(&body, 20);
        assert!(out.chars().count() <= 20 + "Engage.".len() + 1, "{out}");
        assert!(out.starts_with("Engage. Engage."));
    }
}
