import { describe, expect, it } from "vitest";
import type { SearchKeySpec } from "@/api";
import { chipWords, completingAt, termValue, words } from "./queryText";

const KEYS: SearchKeySpec[] = [
  { key: "source_id", aliases: [], values: { kind: "source" } },
  { key: "channel", aliases: [], values: { kind: "text" } },
  { key: "step", aliases: [], values: { kind: "step" } },
  { key: "author_handle", aliases: ["handle"], values: { kind: "text" } },
];

/** `|` marks the cursor. */
function at(typed: string) {
  const pos = typed.indexOf("|");
  return completingAt(typed.replace("|", ""), pos);
}

describe("words", () => {
  it("splits as datalib_query does, with each word's place", () => {
    const q = 'level:warn hello -target:sqlx "two words" k:"a b"';
    expect(words(q).map((w) => [q.slice(w.from, w.to), w.key, w.value, w.negate])).toEqual([
      ["level:warn", "level", "warn", false],
      ["hello", null, "hello", false],
      ["-target:sqlx", "target", "sqlx", true],
      ['"two words"', null, '"two words"', false],
      ['k:"a b"', "k", "a b", false],
    ]);
    const neg = words(q)[2];
    expect(q.slice(neg.valueFrom, neg.to)).toBe("sqlx");
  });

  /** A lone `-`, an empty key or an empty value is not a term. */
  it("leaves malformed terms as free text", () => {
    expect(words("- :x key: -foo").map((w) => w.key)).toEqual([null, null, null, null]);
  });

  it("splits a term at its first colon, so a value may hold more", () => {
    const [w] = words("from:email:ann@example.com");
    expect([w.key, w.value]).toEqual(["from", "email:ann@example.com"]);
  });
});

describe("completingAt", () => {
  it("is a key while the word has no colon before the cursor", () => {
    expect(at("budget chan|")).toEqual({ kind: "key", from: 7, to: 11, typed: "chan" });
    expect(at("-chan|")).toEqual({ kind: "key", from: 1, to: 5, typed: "chan" });
    expect(at("cha|nnel:bridge")).toEqual({ kind: "key", from: 0, to: 7, typed: "cha" });
  });

  it("is a value past the colon, with the rest of the query to narrow it", () => {
    expect(at("is:document channel:br| budget")).toEqual({
      kind: "value",
      key: "channel",
      from: 20,
      to: 22,
      typed: "br",
      rest: "is:document budget",
    });
    expect(at('channel:"two w|')).toMatchObject({ kind: "value", typed: "two w" });
    expect(at("channel:|")).toMatchObject({ kind: "value", key: "channel", typed: "" });
  });

  it("offers nothing between words or for what cannot start a key", () => {
    expect(at("a | b")).toBeNull();
    expect(at("|")).toBeNull();
    expect(at('"phr|')).toBeNull();
    expect(at("3d|")).toBeNull();
  });
});

describe("termValue", () => {
  it("mirrors datalib_query::term", () => {
    expect(termValue("plain")).toBe("plain");
    expect(termValue("a:b")).toBe("a:b");
    expect(termValue("two words")).toBe('"two words"');
    expect(termValue("-leading")).toBe('"-leading"');
    expect(termValue('say "hi" \\ done')).toBe('"say \\"hi\\" \\\\ done"');
  });
});

describe("chipWords", () => {
  it("draws a source, a group or a step as its chip, by key or alias", () => {
    const q = "source_id:slack channel:bridge -step:slack/ingest";
    expect(
      chipWords(q, KEYS).map(({ word, uri }) => [q.slice(word.valueFrom, word.to), uri]),
    ).toEqual([
      ["slack", "datalib:group/slack"],
      ["slack/ingest", "datalib:step/slack/ingest"],
    ]);
    expect(chipWords("nope:slack", KEYS)).toEqual([]);
  });
});
