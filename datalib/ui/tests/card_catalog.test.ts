import { afterEach, describe, expect, it } from "vitest";
import { BUILTIN_META, byAudience, cardMeta, galleryBuiltins } from "../src/cards/catalog";
import { CARD_GLYPHS, DEFAULT_CARD_ICON, resolveIcon } from "../src/cards/icons";
import { frontendManifest } from "../src/cards/frontendRegistry";
import type { Meta } from "../src/api";

afterEach(() => {
  frontendManifest.value = new Map();
});

function manifest(ns: string, entries: Record<string, Meta>) {
  frontendManifest.value = new Map([[ns, new Map(Object.entries(entries))]]);
}

describe("the card catalog", () => {
  // That every builtin has an entry is the type checker's job:
  // BUILTIN_META is keyed by ViewLibs.
  it("names a glyph for every builtin", () => {
    for (const meta of Object.values(BUILTIN_META)) {
      expect(meta.icon && meta.icon in CARD_GLYPHS).toBe(true);
    }
  });

  it("offers Sources first among the builtins", () => {
    expect(galleryBuiltins()[0].source).toBe("sourcesView()");
  });

  /** The gallery's two groups come from each entry's own `devTool`, not from a list of names. */
  it("lists the developer tools apart from the views, each in catalog order", () => {
    const { views, devTools } = byAudience(galleryBuiltins());
    expect(views[0].source).toBe("sourcesView()");
    expect(views.map((e) => e.source)).not.toContain("logView()");
    expect(devTools.map((e) => e.source)).toEqual([
      "logView()",
      "configView()",
      "dactalView()",
      "sourceDagView()",
      'tableView({ url: "/api/manage/rows" })',
      "aliasView()",
      // The Dashboard's sections: building blocks, rarely wanted alone.
      "syncStatusView()",
      "needsYouView()",
      "libraryView()",
      "sourcesOverviewView()",
      "latestActivityView()",
    ]);
    expect(views.length + devTools.length).toBe(galleryBuiltins().length);
  });

  it("reads a custom component's dev_tool as its devTool", () => {
    manifest("user", {
      probe: {
        title: "Probe",
        description: "",
        component_hash: "a",
        component_args: [],
        dev_tool: true,
      },
      tetris: { title: "Tetris", description: "", component_hash: "b", component_args: [] },
    });
    expect(cardMeta("comp.user.probe()")?.devTool).toBe(true);
    expect(cardMeta("comp.user.tetris()")?.devTool).toBe(false);
  });

  it("answers for a builtin by the factory the source calls", () => {
    expect(cardMeta('gridView({ q: "x" })')?.icon).toBe("table");
  });

  /** A custom component's icon comes from its own metadata, the same field a builtin fills. */
  it("answers for a custom component from its namespace, following a rename", () => {
    manifest("slack_work", {
      channels: {
        title: "Slack — work",
        description: "",
        component_hash: "abc",
        component_args: ["slack_work"],
        icon: "slack",
      },
      old: { renamed_to: "channels" },
    });
    expect(cardMeta('comp.slack_work.channels("slack_work")')?.icon).toBe("slack");
    expect(cardMeta("comp.slack_work.old()")?.title).toBe("Slack — work");
    expect(cardMeta("comp.nowhere.thing()")).toBeNull();
  });
});

describe("resolveIcon", () => {
  it("draws a glyph by name", () => {
    expect(resolveIcon("home")).toEqual({ kind: "glyph", path: CARD_GLYPHS.home });
  });
  it("draws an inline image, and only an image", () => {
    const png = "data:image/png;base64,iVBORw0KGgo=";
    expect(resolveIcon(png)).toEqual({ kind: "image", url: png });
    expect(resolveIcon("data:text/html,<script>alert(1)</script>").kind).toBe("glyph");
  });
  it("draws an unknown or missing token as the default glyph", () => {
    const fallback = { kind: "glyph", path: CARD_GLYPHS[DEFAULT_CARD_ICON] };
    expect(resolveIcon("no-such-icon")).toEqual(fallback);
    expect(resolveIcon(null)).toEqual(fallback);
  });
});
