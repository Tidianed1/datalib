// What every card kind says about itself: a title and description for
// the gallery, the icon the layouts draw beside its name, and whether
// it is a developer tool. A builtin declares it here; a custom
// component declares the same fields in its `<name>.json` (api.ts
// `Meta`, where the last is spelled `dev_tool`). `cardMeta` answers for either,
// so nothing that draws a card knows which kind it is.
import type { ViewLibs } from "./types";
import { cardType } from "./cardId";
import { followRenames, frontendManifest } from "./frontendRegistry";

export type CardMeta = {
  title: string;
  description: string;
  // A token cards/icons.ts resolves; null draws the default glyph.
  icon: string | null;
  // A tool for working on the library or on datalib itself — its logs,
  // its config, its pipeline, its components — rather than a view of
  // the data. The gallery lists these apart, under "Developer tools".
  devTool?: boolean;
};

type BuiltinMeta = CardMeta & {
  // The source the gallery's entry expands to; absent when the card
  // needs arguments only another card can supply.
  gallery?: string;
  // Listed only once the gallery shows every view: a building block of
  // a composite, such as a Dashboard section, that is rarely wanted alone.
  galleryHidden?: boolean;
};

/// Every builtin, in gallery order.
export const BUILTIN_META: Record<keyof ViewLibs, BuiltinMeta> = {
  sourcesView: {
    title: "Manage data sources",
    description: "Configure, view, and execute data ingestion steps and data stores.",
    icon: "sources",
    gallery: "sourcesView()",
  },
  searchView: {
    title: "Search",
    description:
      "Find anything in your library by its words or its meaning. Read results in place, or see them as a table with every column.",
    icon: "search",
    gallery: "searchView()",
  },
  // Not in the gallery: it needs the url of the table to show.
  gridView: {
    title: "Grid",
    description: "A table over any endpoint that pages, sorts and groups the way the search does.",
    icon: "table",
  },
  umapView: {
    title: "Embedding map",
    description:
      "Every document placed by what it says, so like sits near like. Filter, colour by type or source, hover to preview.",
    icon: "map",
    gallery: "umapView()",
  },
  logView: {
    title: "Logs",
    description:
      "Every line the runner, the steps and the server wrote; pick a run or a process, narrow with the query bar.",
    icon: "log",
    devTool: true,
    gallery: "logView()",
  },
  configView: {
    title: "config.toml",
    description: "The config file itself, edited directly.",
    icon: "code",
    devTool: true,
    gallery: "configView()",
  },
  documentPickerView: {
    title: "Markdown Document",
    description: "View rendered markdown for any document in your library.",
    icon: "document",
    gallery: "documentPickerView()",
  },
  dactalView: {
    title: "DACTAL explorer",
    description: "Query and pivot your data with the DACTAL table UI.",
    icon: "table",
    devTool: true,
    gallery: "dactalView()",
  },
  perseusView: {
    title: "Perseus corpus",
    description: "Browse the Perseus editions by book, chapter, and section.",
    icon: "book",
    gallery: "perseusView()",
  },
  sourceDagView: {
    title: "Pipeline DAG",
    description: "See your sources' step graph and watch syncs flow through it live.",
    icon: "dag",
    devTool: true,
    gallery: "sourceDagView()",
  },
  tableView: {
    title: "Table",
    description: "Any endpoint that declares its columns, drawn as a typed table.",
    icon: "table",
    devTool: true,
    gallery: 'tableView({ url: "/api/manage/rows" })',
  },
  aliasView: {
    title: "Component library",
    description: "List the custom components stored on this instance.",
    icon: "component",
    devTool: true,
    gallery: "aliasView()",
  },
  documentView: {
    title: "Document",
    description: "One rendered document.",
    icon: "document",
  },
  galleryView: {
    title: "New card",
    description: "Pick what a card should show.",
    icon: "add",
  },
  agentSeedView: {
    title: "New component",
    description: "A component waiting for a coding agent to build it.",
    icon: "component",
  },
  logLineView: {
    title: "Log line",
    description: "One log line with its fields.",
    icon: "log",
  },
  historyView: {
    title: "Commit history",
    description: "A store's commits, and the difference between two of them.",
    icon: "history",
  },
  personView: {
    title: "Person",
    description: "Your contact and what each source says about one person.",
    icon: "person",
  },
  syncDashboardView: {
    title: "Sync dashboard",
    description: "One source's sync: its steps, charts over the run, and its log.",
    icon: "dashboard",
  },
  syncStatusView: {
    title: "Sync",
    description: "When the library last synced, and the button that syncs everything.",
    icon: "history",
    gallery: "syncStatusView()",
    galleryHidden: true,
  },
  needsYouView: {
    title: "Needs you",
    description: "Sources whose last sync failed or that hold errors, with the fix beside each.",
    icon: "problem",
    gallery: "needsYouView()",
    galleryHidden: true,
  },
  libraryView: {
    title: "Your library",
    description: "How many items the library holds, and what takes its space on disk.",
    icon: "book",
    gallery: "libraryView()",
    galleryHidden: true,
  },
  sourcesOverviewView: {
    title: "Sources overview",
    description: "Each source's state, when it last synced, its items and its size.",
    icon: "sources",
    gallery: "sourcesOverviewView()",
    galleryHidden: true,
  },
  latestActivityView: {
    title: "Latest activity",
    description: "The newest documents in the library.",
    icon: "document",
    gallery: "latestActivityView()",
    galleryHidden: true,
  },
};

/// The builtins the gallery offers, in order, each with its source;
/// with `all`, the ones hidden by default too.
export function galleryBuiltins(all = false): (CardMeta & { source: string })[] {
  return Object.values(BUILTIN_META).flatMap((m) =>
    m.gallery && (all || !m.galleryHidden)
      ? [
          {
            title: m.title,
            description: m.description,
            icon: m.icon,
            devTool: m.devTool,
            source: m.gallery,
          },
        ]
      : [],
  );
}

/// The metadata of the card `source` would show: a builtin's, or the
/// custom component's as its namespace last reported it (following a
/// rename). Null for source that calls neither, which draws as a
/// generic card.
export function cardMeta(source: string): CardMeta | null {
  const type = cardType(source);
  if (type in BUILTIN_META) return BUILTIN_META[type as keyof ViewLibs];
  const m = type.match(/^comp\.([A-Za-z_$][\w$]*)\.([A-Za-z_$][\w$]*)$/);
  if (!m) return null;
  const [, ns, name] = m;
  const meta = frontendManifest.value.get(ns)?.get(followRenames(ns, name) ?? name);
  if (!meta || "renamed_to" in meta) return null;
  return {
    title: meta.title,
    description: meta.description,
    icon: meta.icon ?? null,
    devTool: meta.dev_tool === true,
  };
}

/// `entries` as the gallery lists them: the views of the data first,
/// then the developer tools, each in the order given.
export function byAudience<E extends { devTool?: boolean }>(
  entries: E[],
): { views: E[]; devTools: E[] } {
  return {
    views: entries.filter((e) => !e.devTool),
    devTools: entries.filter((e) => e.devTool),
  };
}
