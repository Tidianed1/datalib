import { describe, expect, it } from "vitest";
import { historyRows, type HistoryRow } from "./commitHistory";
import {
  addComparison,
  changeDetail,
  changeSummary,
  commitLabel,
  commitTooltip,
  defaultPair,
  recordStore,
  selectedPair,
} from "./compareCommits";
import { formatShortStamp } from "./timeFormat";
import type { HistoryCommit, TreeHistory } from "@/api";

const commit = (hash: string, date: string): HistoryCommit => ({
  hash,
  parent: null,
  committer: "doltlite",
  date,
  message: `sync ${hash}`,
  run: null,
  tables: [],
});

const store = (path: string, hashes: string[]) => ({
  path,
  truncated: false,
  commits: hashes.map((h, i) => commit(h, `2026-09-0${9 - i}T12:00:00+00:00`)),
});

/// The history of a source whose download and render keep a store each —
/// newest first in each.
function enterprise(): HistoryRow[] {
  const h: TreeHistory = {
    tree: "enterprise",
    stores: [
      store("enterprise/ingest/entities.doltlite_db", ["e3", "e2", "e1"]),
      store("enterprise/render_markdown/render.doltlite_db", ["r2", "r1"]),
    ],
  };
  return historyRows([h]);
}

const commitRow = (rows: HistoryRow[], hash: string) =>
  rows.find((r) => r.level === "commit" && r.hash === hash)!;

describe("compareCommits", () => {
  it("compares commits of the download's entities store, not its other stores", () => {
    expect(recordStore(enterprise(), "enterprise")).toBe("enterprise/ingest/entities.doltlite_db");
  });

  it("starts on the newest two commits, older as from", () => {
    const pair = defaultPair(enterprise(), "enterprise");
    expect(pair).toMatchObject({ from: { hash: "e2" }, to: { hash: "e3" } });
  });

  it("says why there is nothing to compare before a second sync", () => {
    const once = historyRows([
      { tree: "q", stores: [store("q/ingest/entities.doltlite_db", ["only"])] },
    ]);
    expect(defaultPair(once, "q")).toMatch(/synced once/);
    expect(defaultPair([], "q")).toMatch(/no synced data/);
  });

  it("orders a selected pair by the log, whichever was clicked first", () => {
    const rows = enterprise();
    const pair = selectedPair(rows, "enterprise", [commitRow(rows, "e1"), commitRow(rows, "e3")]);
    expect(pair).toMatchObject({ from: { hash: "e1" }, to: { hash: "e3" } });
  });

  /// Only the record store's commits are raw commits the diff render
  /// reads; a render store's hash in the config fails its sync.
  it("refuses a selection that is not two commits of the record store", () => {
    const rows = enterprise();
    const e3 = commitRow(rows, "e3");
    expect(selectedPair(rows, "enterprise", [e3])).toBe("Select two commits to compare");
    expect(selectedPair(rows, "enterprise", [e3, commitRow(rows, "r2")])).toBe(
      "Compare two commits of enterprise/ingest/entities.doltlite_db",
    );
    const tableRow = { ...e3, level: "table" as const, key: `${e3.key}#t` };
    expect(selectedPair(rows, "enterprise", [e3, commitRow(rows, "e2"), tableRow])).toBe(
      "Select two commits to compare",
    );
  });

  it("adds the comparison as a group with its render step", () => {
    const base = '[[groups]]\nid = "enterprise"\ntype = "slack"\n';
    const { text, renderId } = addComparison(base, {
      id: "enterprise-changes",
      name: "Enterprise · changes",
      source: "enterprise",
      from: "e2",
      to: "e3",
      maxDocuments: 50,
    });
    expect(renderId).toBe("enterprise-changes/render_markdown");
    expect(text).toContain('source = "enterprise"');
    expect(text).toContain('from = "e2"');
    expect(text).toContain('to = "e3"');
    expect(text).toContain("max_documents = 50");
  });
});

describe("the compare bar", () => {
  const changes = (added: number, deleted: number, modified: number) => ({
    added,
    deleted,
    modified,
    tables: [],
  });

  /// The label was the provider's summary and a hash prefix, which says
  /// nothing about what the comparison will show.
  it("labels a side with the minute it was made, the message and hash only on hover", () => {
    const e3 = commitRow(enterprise(), "e3");
    expect(commitLabel(e3)).toBe(formatShortStamp("2026-09-09T12:00:00+00:00"));
    expect(commitLabel(e3)).not.toContain("sync");
    expect(commitLabel(e3)).not.toContain("e3");
    const tip = commitTooltip(e3).split("\n");
    expect(tip.slice(1)).toEqual(["sync e3", "e3"]);
  });

  it("counts what the pair changed, leaving out what is zero", () => {
    expect(changeSummary(changes(3, 4, 1))).toBe("3 added, 4 removed, 1 changed");
    expect(changeSummary(changes(0, 2, 0))).toBe("2 removed");
    expect(changeSummary(changes(1200, 0, 5))).toBe(`${(1200).toLocaleString()} added, 5 changed`);
    expect(changeSummary(changes(0, 0, 0))).toBe("No records changed");
  });

  it("breaks the counts down by table on hover", () => {
    expect(
      changeDetail({
        ...changes(3, 1, 0),
        tables: [
          { table: "contacts", added: 2, deleted: 1, modified: 0 },
          { table: "addressbooks", added: 1, deleted: 0, modified: 0 },
        ],
      }),
    ).toBe("contacts: 2 added, 1 removed\naddressbooks: 1 added");
  });
});
