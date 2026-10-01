<script setup lang="ts">
// Routed view for the card surface. Owns the chrome the layouts
// share — the status bar along the bottom: the data root and its
// size, the log, then the density, layout and dev toggles flush
// right — and
// keeps each layout host alive across toggles (v-show, not v-if) so
// switching back doesn't lose its cards. A grid card carries its own
// row count. Which layout is showing is remembered in this browser.
import { onBeforeUnmount, onMounted, ref, useTemplateRef } from "vue";
import MillerView from "@/views/MillerView.vue";
import TreeView from "@/views/TreeView.vue";
import TilingView from "@/views/TilingView.vue";
import TabsView from "@/views/TabsView.vue";
import RootStorageBar from "@/components/RootStorageBar.vue";
import { devMode } from "@/devMode";
import { density, DENSITIES } from "@/density";
import { LOG_CARD, surface, type SurfaceCommands } from "@/surface";

const LAYOUTS = ["columns", "tabs", "tree", "tiling"] as const;
type Layout = (typeof LAYOUTS)[number];
const LAYOUT_KEY = "datalib-layout";
// What a browser that has never picked a layout gets.
const DEFAULT_LAYOUT: Layout = "tabs";

function storedLayout(): Layout {
  try {
    const s = localStorage.getItem(LAYOUT_KEY);
    return LAYOUTS.find((l) => l === s) ?? DEFAULT_LAYOUT;
  } catch {
    return DEFAULT_LAYOUT;
  }
}

const layout = ref<Layout>(DEFAULT_LAYOUT);
const millerMounted = ref(false);
const tabsMounted = ref(false);
const treeMounted = ref(false);
const tilingMounted = ref(false);

function setLayout(next: Layout) {
  layout.value = next;
  if (next === "columns") millerMounted.value = true;
  if (next === "tabs") tabsMounted.value = true;
  if (next === "tree") treeMounted.value = true;
  if (next === "tiling") tilingMounted.value = true;
  try {
    localStorage.setItem(LAYOUT_KEY, next);
  } catch {
    // Blocked storage: the choice lasts as long as the page.
  }
}

// A layout mounts the first time it is shown, so a hidden one runs no
// cards. The tabs layout opens the URL it loads on; mounted later, it
// keeps the URL another layout wrote out of its tabs.
const initialLayout = storedLayout();
setLayout(initialLayout);

// The toolbar's commands go to whichever layout is showing.
const miller = useTemplateRef<SurfaceCommands>("miller");
const tabs = useTemplateRef<SurfaceCommands>("tabs");
const tree = useTemplateRef<SurfaceCommands>("tree");
const tiling = useTemplateRef<SurfaceCommands>("tiling");
function active(): SurfaceCommands | null {
  if (layout.value === "tabs") return tabs.value;
  if (layout.value === "tree") return tree.value;
  if (layout.value === "tiling") return tiling.value;
  return miller.value;
}
onMounted(() => {
  surface.value = {
    addCard: () => active()?.addCard(),
    showCard: (source) => active()?.showCard(source),
  };
});
onBeforeUnmount(() => {
  surface.value = null;
});
</script>

<template>
  <div class="cards-root">
    <MillerView
      v-if="millerMounted"
      ref="miller"
      v-show="layout === 'columns'"
      :active="layout === 'columns'"
    />
    <TabsView
      v-if="tabsMounted"
      ref="tabs"
      v-show="layout === 'tabs'"
      :active="layout === 'tabs'"
      :open-url-on-mount="initialLayout === 'tabs'"
    />
    <TreeView v-if="treeMounted" ref="tree" v-show="layout === 'tree'" />
    <TilingView v-if="tilingMounted" ref="tiling" v-show="layout === 'tiling'" />
    <div class="cards-statusbar">
      <RootStorageBar />
      <!-- What the system is doing belongs down here with the data
           root's size: the log over every run, revealed if a card
           already shows it. -->
      <button
        class="cards-logs"
        title="the run log: every line the runner, the steps and the server wrote"
        @click="active()?.showCard(LOG_CARD)"
      >
        Logs
      </button>
      <div class="cards-toggle cards-density-toggle" role="group" aria-label="density">
        <button
          v-for="d in DENSITIES"
          :key="d"
          :class="{ 'is-active': density === d }"
          :aria-pressed="density === d"
          :title="d === 'compact' ? 'fit more on screen' : 'larger text and more room'"
          @click="density = d"
        >
          {{ d === "compact" ? "Compact" : "Comfortable" }}
        </button>
      </div>
      <div class="cards-toggle" role="group" aria-label="card layout">
        <button
          :class="{ 'is-active': layout === 'columns' }"
          title="miller columns (synced to the URL)"
          @click="setLayout('columns')"
        >
          Columns
        </button>
        <button
          :class="{ 'is-active': layout === 'tabs' }"
          title="one card at a time, with a tree of every open card beside it, each under the card that opened it (kept in this browser)"
          @click="setLayout('tabs')"
        >
          Tabs
        </button>
        <button
          :class="{ 'is-active': layout === 'tree' }"
          title="2D tree (in-memory only, not in the URL)"
          @click="setLayout('tree')"
        >
          Tree
        </button>
        <button
          :class="{ 'is-active': layout === 'tiling' }"
          title="tiling window manager (in-memory only, not in the URL)"
          @click="setLayout('tiling')"
        >
          Tiling
        </button>
      </div>
      <button
        class="cards-dev-toggle"
        :class="{ 'is-active': devMode }"
        :aria-pressed="devMode"
        title="dev mode: show and edit each card's source"
        @click="devMode = !devMode"
      >
        Dev
      </button>
    </div>
  </div>
</template>

<style scoped>
.cards-root {
  display: flex;
  flex-direction: column;
  /* Fill whatever the shell's flex layout gives us (everything below
     the header); basis 0 + min-height 0 so intrinsic content height
     can't stretch the page. */
  flex: 1 1 0;
  min-height: 0;
}
.cards-statusbar {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 10px;
  height: var(--datalib-statusbar-h);
  box-sizing: border-box;
  padding: 0 10px;
  border-top: 1px solid var(--datalib-border);
  background: var(--datalib-sidebar);
  color: var(--datalib-muted);
  font-size: var(--datalib-font-size-small);
}
.cards-logs,
.cards-dev-toggle {
  flex: 0 0 auto;
  height: calc(var(--datalib-control-h) - 6px);
  border: 1px solid var(--datalib-border);
  border-radius: var(--datalib-radius);
  background: var(--datalib-surface);
  color: var(--datalib-fg);
  cursor: pointer;
  font: inherit;
  padding: 0 8px;
}
.cards-logs:hover,
.cards-dev-toggle:hover {
  background: var(--datalib-hover);
}
.cards-dev-toggle.is-active {
  background: var(--datalib-accent);
  border-color: var(--datalib-accent);
  color: var(--datalib-on-accent);
}
/* The toggles sit flush right as a cluster — the first one carries the
   auto margin. */
.cards-density-toggle {
  margin-left: auto;
}
.cards-toggle {
  /* Always claim full intrinsic width (never shrink). */
  flex: 0 0 auto;
  display: flex;
  border: 1px solid var(--datalib-border);
  border-radius: var(--datalib-radius);
  overflow: hidden;
}
.cards-toggle button {
  height: calc(var(--datalib-control-h) - 8px);
  border: none;
  background: var(--datalib-surface);
  color: var(--datalib-fg);
  cursor: pointer;
  font: inherit;
  padding: 0 8px;
}
.cards-toggle button + button {
  border-left: 1px solid var(--datalib-border);
}
.cards-toggle button:hover {
  background: var(--datalib-hover);
}
.cards-toggle button.is-active {
  background: var(--datalib-fg);
  color: var(--datalib-surface);
}
</style>
