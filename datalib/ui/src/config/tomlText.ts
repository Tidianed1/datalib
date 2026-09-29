// TOML written by hand, into text a person also edits: a quoted string,
// and an array of strings edited where it stands, keeping how it was
// written — on one line, or one id per line with its indentation, its
// trailing comma and the comments and blank lines between the ids. The
// config upgrade's `datalib/backend/migrate_config/src/array_layout.rs`
// edits an array the same way.

import { parseTOML, getStaticTOMLValue } from "toml-eslint-parser";

type Id = {
  kind: "id";
  /// As written, quotes and all.
  token: string;
  /// The string it spells; null for anything that is not a string.
  value: string | null;
  /// The whitespace before it when it starts a line; null when it follows
  /// something else on its line.
  indent: string | null;
  /// A comment after it on its line, with the space before the `#`.
  note: string;
};

type Entry = Id | { kind: "comment"; indent: string; text: string } | { kind: "blank" };

type Scanned = {
  entries: Entry[];
  /// A comment on the `[` line, with the space before it.
  head: string;
  multiline: boolean;
  trailingComma: boolean;
  /// The indentation of a `]` on its own line; null when it closes the
  /// last line of entries.
  close: string | null;
  /// Where the `]` is.
  end: number;
};

/// Rewrite the array whose `[` is at `open` to hold `edit`'s values: those
/// that stay keep their place, and new ones go at the end. The text comes
/// back unchanged when the values do not change, or when there is no
/// closed array at `open`.
export function editStringArray(
  text: string,
  open: number,
  edit: (values: string[]) => string[],
): string {
  const scanned = scan(text, open + 1);
  if (!scanned) return text;
  const ids = scanned.entries.filter(isId);
  const before = ids.flatMap((e) => (e.value === null ? [] : [e.value]));
  const after = edit(before);
  if (after.length === before.length && after.every((v, i) => v === before[i])) return text;
  const kept = scanned.entries.filter(
    (e) => !isId(e) || e.value === null || after.includes(e.value),
  );
  const added: Id[] = after
    .filter((v) => !before.includes(v))
    .map((v) => ({ kind: "id", token: quote(v), value: v, indent: null, note: "" }));
  return text.slice(0, open) + render(scanned, [...kept, ...added]) + text.slice(scanned.end + 1);
}

function isId(e: Entry): e is Id {
  return e.kind === "id";
}

function scan(text: string, from: number): Scanned | null {
  const entries: Entry[] = [];
  let head = "";
  let multiline = false;
  let trailingComma = false;
  // Only whitespace so far on a line after the first.
  let lineStart = false;
  let space = "";
  let onThisLine: Id | null = null;
  for (let i = from; i < text.length;) {
    const c = text[i];
    if (c === "]") {
      return { entries, head, multiline, trailingComma, close: lineStart ? space : null, end: i };
    }
    if (c === " " || c === "\t" || c === "\r") {
      if (c !== "\r") space += c;
      i++;
    } else if (c === "\n") {
      if (lineStart) entries.push({ kind: "blank" });
      multiline = true;
      lineStart = true;
      space = "";
      onThisLine = null;
      i++;
    } else if (c === "#") {
      const stop = text.indexOf("\n", i) === -1 ? text.length : text.indexOf("\n", i);
      const comment = text.slice(i, stop).replace(/\r$/, "");
      if (onThisLine) onThisLine.note = space + comment;
      else if (!multiline) head = space + comment;
      else entries.push({ kind: "comment", indent: space, text: comment });
      lineStart = false;
      space = "";
      i = stop;
    } else if (c === ",") {
      trailingComma = true;
      lineStart = false;
      space = "";
      i++;
    } else {
      const stop = tokenEnd(text, i);
      const token = text.slice(i, stop);
      const id: Id = {
        kind: "id",
        token,
        value: valueOf(token),
        indent: lineStart ? space : null,
        note: "",
      };
      entries.push(id);
      onThisLine = id;
      trailingComma = false;
      lineStart = false;
      space = "";
      i = stop;
    }
  }
  return null;
}

/// Where the value starting at `i` ends: past its closing quote for a
/// string, else at the next separator.
function tokenEnd(text: string, i: number): number {
  for (const q of ['"""', "'''", '"', "'"]) {
    if (!text.startsWith(q, i)) continue;
    let j = i + q.length;
    while (j < text.length && !text.startsWith(q, j)) j += q[0] === '"' && text[j] === "\\" ? 2 : 1;
    return Math.min(j + q.length, text.length);
  }
  const stop = text.slice(i).search(/[ \t\r\n,#\]]/);
  return stop === -1 ? text.length : i + stop;
}

function valueOf(token: string): string | null {
  try {
    const { v } = getStaticTOMLValue(parseTOML(`v = ${token}`)) as { v: unknown };
    return typeof v === "string" ? v : null;
  } catch {
    return null;
  }
}

function render(s: Scanned, entries: Entry[]): string {
  const ids = entries.filter(isId);
  if (!s.multiline) return `[${ids.map((e) => e.token).join(", ")}]`;
  const hadIds = s.entries.some(isId);
  const trailingComma = hadIds ? s.trailingComma : true;
  const indents = s.entries
    .map((e) => (e.kind === "blank" ? null : e.indent))
    .filter((x): x is string => x !== null);
  const indent = indents[0] ?? `${s.close ?? ""}  `;
  const last = ids[ids.length - 1];
  const lines = entries.map((e) => {
    if (e.kind === "blank") return "";
    if (e.kind === "comment") return e.indent + e.text;
    const comma = e !== last || trailingComma ? "," : "";
    return `${e.indent ?? indent}${e.token}${comma}${e.note}`;
  });
  const open = `[${s.head}\n`;
  const tail = entries[entries.length - 1];
  // A `]` after a comment would be part of it.
  const closeInline = s.close === null && tail !== undefined && isId(tail) && tail.note === "";
  if (closeInline) return `${open}${lines.join("\n")}]`;
  const close = `${s.close ?? ""}]`;
  return lines.length ? `${open}${lines.join("\n")}\n${close}` : `${open}${close}`;
}

/// TOML basic string. Dates are quoted too: a bare `2026-01-01` parses
/// as a TOML date, and the providers validate a *string*.
export function quote(s: string): string {
  const escaped = s
    .replace(/\\/g, "\\\\")
    .replace(/"/g, '\\"')
    .replace(/\n/g, "\\n")
    .replace(/\r/g, "\\r")
    .replace(/\t/g, "\\t")
    // Everything else TOML calls a control char, as \uXXXX.
    .replace(
      /[\u0000-\u001f\u007f]/g,
      (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, "0")}`,
    );
  return `"${escaped}"`;
}
