// What every card kind says about itself: a title and description for
// the gallery, and the icon the layouts draw beside its name. A builtin
// declares it here; a custom component declares the same three fields
// in its `<name>.json` (api.ts `Meta`). `cardMeta` answers for either,
// so nothing that draws a card knows which kind it is.
import type { ViewLibs } from "./types";
import { cardType } from "./cardId";
import { followRenames, frontendManifest } from "./frontendRegistry";

export type CardMeta = {
  title: string;
  description: string;
  // A token cards/icons.ts resolves; null draws the default glyph.
  icon: string | null;
};

type BuiltinMeta = CardMeta & {
  // The source the gallery's entry expands to; absent when the card
  // needs arguments only another card can supply.
  gallery?: string;
};

/// Every builtin, in gallery order.
export const BUILTIN_META: Record<keyof ViewLibs, BuiltinMeta> = {
  homeView: {
    title: "Home",
    description:
      "What needs you, how big your library is, each source's state, and the newest documents.",
    icon: "home",
    gallery: "homeView()",
  },
  sourcesView: {
    title: "Manage data sources",
    description: "Configure, view, and execute data ingestion steps and data stores.",
    icon: "sources",
    gallery: "sourcesView()",
  },
  gridView: {
    title: "Unified Search",
    description: "Search and browse everything in your library.",
    icon: "table",
    gallery: "gridView()",
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
    gallery: "logView()",
  },
  configView: {
    title: "config.toml",
    description: "The config file itself, edited directly.",
    icon: "code",
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
    gallery: "sourceDagView()",
  },
  tableView: {
    title: "Table",
    description: "Any endpoint that declares its columns, drawn as a typed table.",
    icon: "table",
    gallery: 'tableView({ url: "/api/manage/rows" })',
  },
  aliasView: {
    title: "Component library",
    description: "List the custom components stored on this instance.",
    icon: "component",
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
  syncDashboardView: {
    title: "Sync dashboard",
    description: "One source's sync: its steps, charts over the run, and its log.",
    icon: "dashboard",
  },
};

/// The builtins the gallery offers, in order, each with its source.
export function galleryBuiltins(): (CardMeta & { source: string })[] {
  return Object.values(BUILTIN_META).flatMap((m) =>
    m.gallery
      ? [{ title: m.title, description: m.description, icon: m.icon, source: m.gallery }]
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
  return { title: meta.title, description: meta.description, icon: meta.icon ?? null };
}
