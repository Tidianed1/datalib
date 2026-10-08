// Builtin view: the new-card gallery — the way every new card starts,
// in and out of edit mode. It lists, each with a short description and
// its icon: the composites (views/composites.ts — the Dashboard, the
// app's front door, first), then every parameter-less builtin
// cards/catalog.ts offers, then every titled component in the frontend
// store, then the "build a component with an agent" entry (handoff.ts),
// which mints a fresh component and walks the user through handing it
// to a coding agent. An entry whose metadata says `devTool` — the logs,
// the config, the pipeline graph, the agent entry — is listed after
// the rest under "Developer tools", a section that starts closed
// outside edit mode. A builtin that is a building block (a Dashboard
// section) is listed only once "Show every view" is on. Picking an
// entry REPLACES this card — with the chosen component via
// ctx.host.setSource, or with a copy of the composite via
// ctx.host.becomeComposite — so the gallery is a transient "what should
// this card be?" step, not a lingering column.
import { ref, watch } from "vue";
import type { CardRender } from "../types";
import { ensureFrontend, frontendManifest, gallerySource } from "../frontendRegistry";
import { createComponentWithAgent } from "@/handoff";
import { editMode } from "@/editMode";
import { byAudience, galleryBuiltins, type CardMeta } from "../catalog";
import { resolveIcon } from "../icons";
import { galleryComposites, loadComposites, savedComposites } from "@/views/composites";

// Whether the gallery lists every view, the building blocks too; kept
// in this browser.
const SHOW_ALL_KEY = "datalib-gallery-all";
function storedShowAll(): boolean {
  try {
    return localStorage.getItem(SHOW_ALL_KEY) === "1";
  } catch {
    return false;
  }
}
const showAll = ref(storedShowAll());
watch(showAll, (on) => {
  try {
    localStorage.setItem(SHOW_ALL_KEY, on ? "1" : "0");
  } catch {
    // Blocked storage: the choice lasts as long as the page.
  }
});

// Whether "Developer tools" is open, once the person has opened or
// closed it; kept in this browser. Before that it follows edit mode.
const DEV_OPEN_KEY = "datalib-gallery-dev-tools";
function storedDevOpen(): boolean | null {
  try {
    const kept = localStorage.getItem(DEV_OPEN_KEY);
    return kept === null ? null : kept === "1";
  } catch {
    return null;
  }
}
const devOpen = ref(storedDevOpen());
watch(devOpen, (open) => {
  if (open === null) return;
  try {
    localStorage.setItem(DEV_OPEN_KEY, open ? "1" : "0");
  } catch {
    // Blocked storage: the choice lasts as long as the page.
  }
});

type GalleryEntry = CardMeta & {
  // Card source the entry expands to, e.g. `gridView()`.
  source: string;
};

function iconElement(token: string | null): Element {
  const icon = resolveIcon(token);
  if (icon.kind === "image") {
    const img = document.createElement("img");
    img.className = "gv-icon";
    img.src = icon.url;
    img.alt = "";
    return img;
  }
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", "gv-icon");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("fill", "currentColor");
  path.setAttribute("d", icon.path);
  svg.appendChild(path);
  return svg;
}

export function galleryView(): CardRender {
  return (root, ctx) => {
    ctx.setTitle("New card");
    const style = document.createElement("style");
    style.textContent = `
      :host { display: block; height: 100%; position: relative; }
      /* The host clips; the list scrolls in a box pinned to it, the way
         vueCard pins a Vue card's root. */
      .gv { position: absolute; inset: 0; overflow-y: auto; font: var(--datalib-font-size, 13px)/1.5 var(--datalib-font, system-ui, sans-serif); color: var(--datalib-fg, inherit); }
      .gv-head { display: flex; align-items: center; gap: 12px; padding: 8px 12px; border-bottom: 1px solid var(--datalib-border, #8884); }
      .gv-head-text { flex: 1 1 auto; opacity: .6; }
      .gv-all { display: flex; align-items: center; gap: 5px; font-size: 12px; cursor: pointer; white-space: nowrap; }
      .gv-row { display: flex; gap: 10px; align-items: flex-start; padding: 8px 12px; cursor: pointer; border-bottom: 1px solid var(--datalib-border, #8882); }
      .gv-icon { flex: 0 0 auto; width: 18px; height: 18px; margin-top: 1px; color: var(--datalib-accent); }
      .gv-text { flex: 1 1 auto; min-width: 0; }
      .gv-row:hover { background: var(--datalib-hover, rgba(127,127,127,.12)); }
      /* Title line: the dev-mode source shares the title's line while
         it fits (baseline-aligned flex) and wraps under it when the
         column is narrow — minimal layout shift vs non-dev. */
      .gv-head-line { display: flex; flex-wrap: wrap; align-items: baseline; column-gap: 10px; }
      .gv-title { font-weight: 600; }
      .gv-desc { opacity: .65; }
      .gv-src { font: 11px/1.4 ui-monospace, Menlo, monospace; opacity: .5; }
      .gv-foot { padding: 8px 12px; opacity: .55; font-size: 12px; }
      .gv-dev > summary { display: flex; align-items: baseline; gap: 8px; padding: 8px 12px; cursor: pointer; font-weight: 600; list-style: none; border-bottom: 1px solid var(--datalib-border, #8882); background: var(--datalib-hover, rgba(127,127,127,.08)); }
      .gv-dev > summary::-webkit-details-marker { display: none; }
      .gv-dev > summary::before { content: "▸"; font-size: 10px; opacity: .7; }
      .gv-dev[open] > summary::before { content: "▾"; }
      .gv-dev-note { font-weight: 400; opacity: .65; }
    `;
    root.appendChild(style);

    const wrap = document.createElement("div");
    wrap.className = "gv";
    root.appendChild(wrap);

    function paint([manifest, dev, all, , keptOpen]: [
      Map<string, Map<string, import("@/api").Meta>>,
      boolean,
      boolean,
      unknown,
      boolean | null,
    ]) {
      wrap.replaceChildren();
      const head = document.createElement("div");
      head.className = "gv-head";
      const headText = document.createElement("span");
      headText.className = "gv-head-text";
      headText.textContent = "pick what this card should show";
      const toggle = document.createElement("label");
      toggle.className = "gv-all";
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = all;
      box.addEventListener("change", () => (showAll.value = box.checked));
      toggle.append(box, "Show every view");
      head.append(headText, toggle);
      wrap.appendChild(head);

      // One row per component in every namespace, with its own stored
      // arguments baked into the source the row expands to. Nothing
      // here knows or cares which namespace an applet wrote — `user`
      // and `slack_work` are read the same way.
      const custom: GalleryEntry[] = [];
      for (const [ns, entries] of [...manifest.entries()].sort((a, b) =>
        a[0].localeCompare(b[0]),
      )) {
        for (const [name, meta] of [...entries.entries()].sort((a, b) =>
          a[0].localeCompare(b[0]),
        )) {
          // A tombstone is a redirect, not something to offer.
          if ("renamed_to" in meta) continue;
          // An untitled component is one nobody meant to advertise.
          if (!meta.title.trim()) continue;
          custom.push({
            source: gallerySource(ns, name, meta.component_args),
            title: meta.title,
            description: meta.description,
            icon: meta.icon ?? null,
            devTool: meta.dev_tool === true,
          });
        }
      }

      function addRow(
        into: Element,
        title: string,
        description: string,
        icon: string | null,
        src: string | null,
        onPick: () => void,
      ) {
        const row = document.createElement("div");
        row.className = "gv-row";
        row.addEventListener("click", onPick);

        row.appendChild(iconElement(icon));
        const text = document.createElement("div");
        text.className = "gv-text";
        const headLine = document.createElement("div");
        headLine.className = "gv-head-line";
        const titleEl = document.createElement("span");
        titleEl.className = "gv-title";
        titleEl.textContent = title;
        headLine.appendChild(titleEl);
        // Edit mode: show what the pick expands to, teaching the
        // source-expression model row by row. Same line as the title
        // while it fits (see .gv-head-line).
        if (dev && src !== null) {
          const code = document.createElement("span");
          code.className = "gv-src";
          code.textContent = src;
          headLine.appendChild(code);
        }
        const desc = document.createElement("div");
        desc.className = "gv-desc";
        desc.textContent = description;
        text.append(headLine, desc);
        row.appendChild(text);
        into.appendChild(row);
      }

      const addEntry = (into: Element, entry: GalleryEntry) =>
        addRow(into, entry.title, entry.description, entry.icon, entry.source, () =>
          ctx.host.setSource(entry.source),
        );

      for (const c of galleryComposites()) {
        addRow(wrap, c.name, c.description, c.icon, null, () => ctx.host.becomeComposite(c.name));
      }
      const { views, devTools } = byAudience([...galleryBuiltins(all), ...custom]);
      for (const entry of views) addEntry(wrap, entry);

      // The tools for working on the library itself, apart from the
      // views of its data, and closed until asked for outside edit mode.
      const section = document.createElement("details");
      section.className = "gv-dev";
      section.open = keptOpen ?? dev;
      section.addEventListener("toggle", () => {
        if (section.open !== (devOpen.value ?? dev)) devOpen.value = section.open;
      });
      const summary = document.createElement("summary");
      const note = document.createElement("span");
      note.className = "gv-dev-note";
      note.textContent = "logs, the config, the pipeline, components";
      summary.append("Developer tools", note);
      section.appendChild(summary);
      for (const entry of devTools) addEntry(section, entry);
      // Last, after even the user's own components: the escape hatch
      // for when nothing above fits. No source line in edit mode — the
      // component name is minted on pick.
      addRow(
        section,
        "New component, built by an agent",
        "Create a fresh component and hand it to a coding agent to build.",
        "component",
        null,
        () => void createComponentWithAgent(ctx.host),
      );
      wrap.appendChild(section);

      if (dev) {
        const foot = document.createElement("div");
        foot.className = "gv-foot";
        foot.textContent =
          "edit mode: every card is a JS expression — you can also type " +
          "source directly into the box above and press Enter.";
        wrap.appendChild(foot);
      }
    }

    void ensureFrontend();
    void loadComposites();
    const stop = watch([frontendManifest, editMode, showAll, savedComposites, devOpen], paint, {
      immediate: true,
    });
    return () => stop();
  };
}
