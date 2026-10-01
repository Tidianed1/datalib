// Builtin view: the new-card gallery — the way every new card starts,
// in both dev and non-dev mode. It lists every parameter-less
// component with a short description and its icon: the builtins
// cards/catalog.ts offers first (Home leading, since it's the app's
// front door, then Sources), then every
// titled component in the frontend store, then the
// "build a component with an agent" entry (handoff.ts),
// which mints a fresh component and walks the user through handing it
// to a coding agent. Picking an entry REPLACES this card with the
// chosen component via ctx.host.setSource, so the gallery is a
// transient "what should this card be?" step, not a lingering column.
import { watch } from "vue";
import type { CardRender } from "../types";
import { ensureFrontend, frontendManifest, gallerySource } from "../frontendRegistry";
import { createComponentWithAgent } from "@/handoff";
import { devMode } from "@/devMode";
import { galleryBuiltins, type CardMeta } from "../catalog";
import { resolveIcon } from "../icons";

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
      .gv-head { padding: 8px 12px; opacity: .6; border-bottom: 1px solid var(--datalib-border, #8884); }
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
    `;
    root.appendChild(style);

    const wrap = document.createElement("div");
    wrap.className = "gv";
    root.appendChild(wrap);

    function paint([manifest, dev]: [Map<string, Map<string, import("@/api").Meta>>, boolean]) {
      wrap.replaceChildren();
      const head = document.createElement("div");
      head.className = "gv-head";
      head.textContent = "pick what this card should show";
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
          });
        }
      }

      function addRow(
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
        // Dev mode: show what the pick expands to, teaching the
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
        wrap.appendChild(row);
      }

      for (const entry of [...galleryBuiltins(), ...custom]) {
        addRow(entry.title, entry.description, entry.icon, entry.source, () =>
          ctx.host.setSource(entry.source),
        );
      }
      // Last, after even the user's own components: the escape hatch
      // for when nothing above fits. No source line in dev mode — the
      // component name is minted on pick.
      addRow(
        "New component, built by an agent",
        "Create a fresh component and hand it to a coding agent to build.",
        "component",
        null,
        () => void createComponentWithAgent(ctx.host),
      );

      if (dev) {
        const foot = document.createElement("div");
        foot.className = "gv-foot";
        foot.textContent =
          "dev mode: every card is a JS expression — you can also type " +
          "source directly into the box above and press Enter.";
        wrap.appendChild(foot);
      }
    }

    void ensureFrontend();
    const stop = watch([frontendManifest, devMode], paint, { immediate: true });
    return () => stop();
  };
}
