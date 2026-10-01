<script setup lang="ts">
// Containers layout host: a tree of containers, each laying out its own
// children (containerTree.ts holds the rules). The outermost container
// is always tabs, listed down the sidebar as a tree; what is inside each
// tab is drawn by the recursive ContainerNode. The tree is kept in the
// library (`/api/ui/state/layout`), so it survives a restart.
//
// The cards themselves live in one flat pool here and are teleported
// into the slot their node draws, as in TilingView: rearranging the
// containers moves a card's DOM without remounting it.
import { computed, onBeforeUnmount, provide, reactive, ref, watch, watchEffect } from "vue";
import { useRoute, useRouter } from "vue-router";
import ShadowCard from "@/components/ShadowCard.vue";
import CardIcon from "@/components/CardIcon.vue";
import ContainerNode from "@/views/ContainerNode.vue";
import { fetchUiState, putUiState } from "@/api";
import { createBus } from "@/cards/bus";
import { chainHref } from "@/cards/chainHref";
import { setCardHelp } from "@/cards/help";
import { cardType, newCardId } from "@/cards/cardId";
import { displayTitle } from "@/cards/title";
import { devMode } from "@/devMode";
import { decodeColumns } from "@/router/columns";
import { pageTitle } from "@/views/millerStack";
import { isMainWindow } from "@/views/tabsWindow";
import {
  BUILTIN_COMPOSITES,
  composite,
  loadComposites,
  saveComposite,
  savedComposites,
} from "@/views/composites";
import {
  LAYOUTS,
  LAYOUT_LABELS,
  addChild,
  cards,
  find,
  instantiate,
  isSolidified,
  makeBox,
  makeCard,
  move,
  openFrom,
  parentOf,
  parseTree,
  remove,
  rename,
  resetTo,
  reveal,
  setBasis,
  setCard,
  setFlag,
  setLayout as setBoxLayout,
  setTemplate,
  tabRows,
  underSolidifyAll,
  unwrap,
  wrap,
  type BoxNode,
  type CardNode,
  type Layout,
  type TreeNode,
} from "@/views/containerTree";
import { CONTAINERS_API, type ContainersApi, type MenuItem } from "@/views/containersApi";
import type { CardCtx, HostCommands } from "@/cards/types";

defineProps<{ active: boolean }>();

const route = useRoute();
const router = useRouter();
const bus = createBus();

const STATE_NAME = "layout";

function dashboard(): TreeNode {
  return instantiate(BUILTIN_COMPOSITES.Dashboard, newCardId);
}

function startingRoot(): BoxNode {
  return makeBox(newCardId(), "tabs", [dashboard()]);
}

const root = ref<BoxNode>(startingRoot());
const ready = ref(false);
let mainWindow = false;

// The outermost container is always tabs, and never empty.
function settle(next: TreeNode): BoxNode {
  const box = next.kind === "box" ? next : makeBox(newCardId(), "tabs", [next]);
  const fixed: BoxNode = { ...box, layout: "tabs", solidified: false, solidifyAll: false };
  if (fixed.children.length > 0) {
    const selected = fixed.children.some((c) => c.id === fixed.selected)
      ? fixed.selected
      : fixed.children[0].id;
    return { ...fixed, selected };
  }
  const fresh = dashboard();
  return { ...fixed, children: [fresh], selected: fresh.id };
}

function update(next: TreeNode) {
  root.value = settle(next);
}

// ---- keeping the tree ----

let saveTimer: ReturnType<typeof setTimeout> | null = null;
function save() {
  saveTimer = null;
  if (!mainWindow) return;
  putUiState(STATE_NAME, root.value).catch((e: unknown) =>
    console.warn("could not keep the layout", e),
  );
}
watch(root, () => {
  if (!ready.value) return;
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(save, 400);
});
function flush() {
  if (saveTimer) {
    clearTimeout(saveTimer);
    save();
  }
}
window.addEventListener("pagehide", flush);
onBeforeUnmount(() => {
  window.removeEventListener("pagehide", flush);
  flush();
});

// A URL naming cards (a link, a popped-out card) opens them in a tab of
// their own, then the address goes back to "/": the tree, not the URL,
// is what this layout keeps.
function adoptRoute() {
  const specs = decodeColumns(route.path);
  if (specs.length === 0) return;
  const nodes = specs.map((s) => makeCard(newCardId(), s.code, s.state));
  const node: TreeNode =
    nodes.length === 1 ? nodes[0] : makeBox(newCardId(), "columns", nodes, { name: null });
  update(addChild(root.value, root.value.id, node));
  void router.replace("/");
}
watch(
  () => route.path,
  () => {
    if (ready.value) adoptRoute();
  },
);

void (async () => {
  mainWindow = await isMainWindow();
  void loadComposites();
  if (mainWindow) {
    try {
      const kept = parseTree(await fetchUiState(STATE_NAME));
      if (kept) root.value = settle(kept);
    } catch (e) {
      console.warn("could not read the kept layout", e);
    }
  } else {
    // A second window (a popped-out card) starts empty but for its URL.
    root.value = { ...root.value, children: [] };
  }
  ready.value = true;
  adoptRoute();
  if (root.value.children.length === 0) update(root.value);
})();

// ---- the cards ----

const allCards = computed(() => cards(root.value));
// A card mounts the first time its slot is drawn, and stays mounted
// while it is in the tree, so a tab switched away from keeps its state.
const mounted = reactive(new Set<string>());
const slots = reactive(new Map<string, Element>());
const pool = computed(() => allCards.value.filter((c) => mounted.has(c.id)));
watch(allCards, (list) => {
  const live = new Set(list.map((c) => c.id));
  for (const id of [...mounted]) if (!live.has(id)) mounted.delete(id);
  for (const id of [...ctxCache.keys()]) if (!live.has(id)) ctxCache.delete(id);
});

function setSlot(id: string, el: Element | null) {
  if (el) {
    slots.set(id, el);
    mounted.add(id);
  } else {
    slots.delete(id);
  }
}

function cardById(id: string): CardNode | undefined {
  const n = find(root.value, id);
  return n?.kind === "card" ? n : undefined;
}

const ctxCache = new Map<string, CardCtx>();
function ctxFor(card: CardNode): CardCtx {
  let ctx = ctxCache.get(card.id);
  if (!ctx) {
    const cardId = card.id;
    const host: HostCommands = {
      openCards: (...sources) => {
        const nodes = sources.map((s) => makeCard(newCardId(), s));
        update(openFrom(root.value, cardId, nodes));
        return nodes.map((n) => n.id);
      },
      hrefFor: (...sources) => chainHref(sources),
      setSource: (source) => update(setCard(root.value, cardId, { source, state: "" })),
      close: () => close(cardId),
      setState: (state) => {
        if (cardById(cardId)?.state !== state) update(setCard(root.value, cardId, { state }));
      },
    };
    ctx = {
      cardId,
      get cardType() {
        return cardType(cardById(cardId)?.source ?? "");
      },
      get initialState() {
        return cardById(cardId)?.state ?? "";
      },
      setTitle: (title) => {
        if (title !== null && cardById(cardId)?.title !== title) {
          update(setCard(root.value, cardId, { title }));
        }
      },
      setHelp: (html) => setCardHelp(cardId, html),
      bus,
      host,
    };
    ctxCache.set(cardId, ctx);
  }
  return ctx;
}

function titleOf(node: TreeNode): string {
  if (node.kind === "card") return displayTitle(node.source, node.title);
  if (node.name) return node.name;
  const first = node.children[0];
  return first ? `${titleOf(first)}${node.children.length > 1 ? " …" : ""}` : "Empty";
}

// ---- the commands ----

function select(id: string) {
  update(reveal(root.value, id));
}

function close(id: string) {
  update(remove(root.value, id));
}

function commitSource(card: CardNode, e: Event) {
  const source = (e.target as HTMLTextAreaElement).value;
  if (source !== card.source) update(setCard(root.value, card.id, { source, state: "" }));
}

function setLayout(id: string, layout: Layout) {
  update(setBoxLayout(root.value, id, layout));
}

function toggleFlag(box: BoxNode, flag: "solidified" | "solidifyAll") {
  update(setFlag(root.value, box.id, flag, !box[flag]));
}

function addCard(boxId: string) {
  update(addChild(root.value, boxId, makeCard(newCardId(), "galleryView()")));
}

function addBox(boxId: string, layout: Layout) {
  const box = makeBox(newCardId(), layout, [makeCard(newCardId(), "galleryView()")]);
  update(addChild(root.value, boxId, box));
}

function addComposite(boxId: string, name: string) {
  const template = composite(name);
  if (template) update(addChild(root.value, boxId, instantiate(template, newCardId)));
}

// The toolbar's "New card", and its Logs and Data sources.
function newTab() {
  addCard(root.value.id);
}
function showCard(source: string) {
  const have = allCards.value.find((c) => c.source === source);
  if (have) select(have.id);
  else update(addChild(root.value, root.value.id, makeCard(newCardId(), source)));
}
defineExpose({ addCard: newTab, showCard });

// ---- naming ----

const asking = ref<{ title: string; value: string; done: (v: string | null) => void } | null>(null);
function askName(title: string, value: string): Promise<string | null> {
  return new Promise((resolve) => {
    asking.value = {
      title,
      value,
      done: (v) => {
        asking.value = null;
        resolve(v === null || v.trim() === "" ? null : v.trim());
      },
    };
  });
}
const vFocusSelect = {
  mounted(el: HTMLInputElement) {
    el.focus();
    el.select();
  },
};

async function renameNode(node: TreeNode) {
  const name = await askName("Name", titleOf(node));
  if (name !== null) update(rename(root.value, node.id, name));
}

async function saveAsComposite(box: BoxNode) {
  const name = await askName("Save as composite", box.name ?? titleOf(box));
  if (name === null) return;
  if (name in BUILTIN_COMPOSITES) {
    window.alert(`"${name}" is a built-in composite; pick another name.`);
    return;
  }
  await saveComposite(name, box);
  update(setTemplate(root.value, box.id, name));
}

// ---- menus ----

const menu = ref<{ items: MenuItem[]; x: number; y: number } | null>(null);

function openMenu(ev: MouseEvent, items: MenuItem[]) {
  ev.preventDefault();
  ev.stopPropagation();
  // Kept on screen: a menu opened near the right or bottom edge opens
  // leftward or upward instead.
  const width = 260;
  const height = items.length * 24 + 12;
  menu.value = {
    items,
    x: Math.max(4, Math.min(ev.clientX, window.innerWidth - width - 4)),
    y: Math.max(4, Math.min(ev.clientY, window.innerHeight - height - 4)),
  };
  window.addEventListener("pointerdown", onPointerOutside, true);
  window.addEventListener("keydown", onMenuKey, true);
  window.addEventListener("blur", closeMenu);
}
function closeMenu() {
  menu.value = null;
  window.removeEventListener("pointerdown", onPointerOutside, true);
  window.removeEventListener("keydown", onMenuKey, true);
  window.removeEventListener("blur", closeMenu);
}
onBeforeUnmount(closeMenu);
function onPointerOutside(ev: PointerEvent) {
  if (!(ev.target as Element | null)?.closest?.(".ct-menu")) closeMenu();
}
function onMenuKey(ev: KeyboardEvent) {
  if (ev.key === "Escape") closeMenu();
}
function runItem(item: MenuItem) {
  closeMenu();
  if (item !== "separator") item.run();
}

function placeItems(node: TreeNode): MenuItem[] {
  const out: MenuItem[] = [];
  const parent = parentOf(root.value, node.id);
  if (parent && parent.children.length > 1) {
    out.push({ label: "Move earlier", run: () => update(move(root.value, node.id, -1)) });
    out.push({ label: "Move later", run: () => update(move(root.value, node.id, 1)) });
  }
  for (const layout of LAYOUTS) {
    out.push({
      label: `Put in a new ${LAYOUT_LABELS[layout]} container`,
      run: () => update(wrap(root.value, node.id, layout, newCardId())),
    });
  }
  return out;
}

function compositeItems(boxId: string): MenuItem[] {
  return Object.keys({ ...BUILTIN_COMPOSITES, ...savedComposites.value }).map((name) => ({
    label: `Add composite: ${name}`,
    run: () => addComposite(boxId, name),
  }));
}

function boxMenu(box: BoxNode): MenuItem[] {
  const isRoot = box.id === root.value.id;
  const items: MenuItem[] = [
    { label: "Add card", run: () => addCard(box.id) },
    ...LAYOUTS.map((layout) => ({
      label: `Add ${LAYOUT_LABELS[layout]} container`,
      run: () => addBox(box.id, layout),
    })),
    ...compositeItems(box.id),
  ];
  if (isRoot) return items;
  items.push("separator");
  items.push({
    label: "Solidified",
    checked: box.solidified,
    run: () => toggleFlag(box, "solidified"),
  });
  items.push({
    label: "Solidify all",
    checked: box.solidifyAll,
    run: () => toggleFlag(box, "solidifyAll"),
  });
  items.push("separator");
  items.push({ label: "Rename…", run: () => void renameNode(box) });
  items.push({ label: "Save as composite…", run: () => void saveAsComposite(box) });
  const template = box.template ? composite(box.template) : undefined;
  if (template) {
    items.push({
      label: `Reset to "${box.template}"`,
      run: () => update(resetTo(root.value, box.id, template, newCardId)),
    });
  }
  items.push("separator");
  items.push(...placeItems(box));
  items.push({
    label: "Take the cards out of this container",
    run: () => update(unwrap(root.value, box.id)),
  });
  items.push({ label: "Close", run: () => close(box.id) });
  return items;
}

function cardMenu(card: CardNode): MenuItem[] {
  return [
    ...placeItems(card),
    { label: "Rename…", run: () => void renameNode(card) },
    "separator",
    { label: "Close", run: () => close(card.id) },
  ];
}

// The sidebar's "New": a card, an empty container, or a composite, as a tab.
function newMenu(ev: MouseEvent) {
  openMenu(ev, [
    { label: "Card", run: newTab },
    ...LAYOUTS.filter((l) => l !== "tabs").map((layout) => ({
      label: `${LAYOUT_LABELS[layout]} container`,
      run: () => addBox(root.value.id, layout),
    })),
    "separator",
    ...Object.keys({ ...BUILTIN_COMPOSITES, ...savedComposites.value }).map((name) => ({
      label: name,
      run: () => addComposite(root.value.id, name),
    })),
  ]);
}

function rowMenu(node: TreeNode, ev: MouseEvent) {
  openMenu(ev, node.kind === "box" ? boxMenu(node) : cardMenu(node));
}

// ---- resizing ----

const MIN_PX = 60;

function startResize(id: string, axis: "x" | "y", ev: PointerEvent) {
  if (ev.button !== 0) return;
  ev.preventDefault();
  const handle = ev.currentTarget as HTMLElement;
  const child = handle.previousElementSibling as HTMLElement | null;
  // A stack sizes a child's body, below its header (ContainerNode `height`).
  const sized = axis === "y" ? child?.querySelector<HTMLElement>("[data-body]") : child;
  if (!sized) return;
  const startSize = axis === "x" ? sized.offsetWidth : sized.offsetHeight;
  const start = axis === "x" ? ev.clientX : ev.clientY;
  handle.setPointerCapture(ev.pointerId);
  const onMove = (e: PointerEvent) => {
    const delta = (axis === "x" ? e.clientX : e.clientY) - start;
    update(setBasis(root.value, id, Math.max(MIN_PX, Math.round(startSize + delta))));
  };
  const onUp = (e: PointerEvent) => {
    handle.releasePointerCapture(e.pointerId);
    handle.removeEventListener("pointermove", onMove);
    handle.removeEventListener("pointerup", onUp);
    handle.removeEventListener("pointercancel", onUp);
  };
  handle.addEventListener("pointermove", onMove);
  handle.addEventListener("pointerup", onUp);
  handle.addEventListener("pointercancel", onUp);
}

// ---- what the page shows ----

const rows = computed(() => tabRows(root.value));
const selectedTab = computed(() => root.value.children.find((c) => c.id === root.value.selected));

watchEffect(() => {
  const tab = selectedTab.value;
  document.title = pageTitle(tab ? [titleOf(tab)] : []);
});

const api: ContainersApi = {
  ctxFor,
  titleOf,
  setSlot,
  chromeShown: (id) => devMode.value || !underSolidifyAll(root.value, id),
  isSolidified: (id) => isSolidified(root.value, id),
  select,
  close,
  commitSource,
  setLayout,
  toggleFlag,
  addCard,
  openMenu,
  boxMenu,
  cardMenu,
  startResize,
};
provide(CONTAINERS_API, api);
</script>

<template>
  <div class="ct-root">
    <nav class="ct-sidebar" aria-label="open tabs">
      <ul class="ct-tabs" role="tree">
        <li
          v-for="row in rows"
          :key="row.node.id"
          class="ct-tab"
          :class="{ 'is-selected': row.node.id === root.selected }"
          role="treeitem"
          :aria-selected="row.node.id === root.selected"
          :data-node-id="row.node.id"
          :style="{ paddingLeft: 0.4 + row.depth * 0.9 + 'rem' }"
          :title="titleOf(row.node)"
          @click="select(row.node.id)"
          @contextmenu="rowMenu(row.node, $event)"
          @auxclick.prevent="(e: MouseEvent) => e.button === 1 && close(row.node.id)"
        >
          <CardIcon v-if="row.node.kind === 'card'" class="ct-tab-icon" :source="row.node.source" />
          <svg v-else class="ct-tab-icon" viewBox="0 0 24 24" aria-hidden="true">
            <path
              fill="currentColor"
              d="M4 4h7v7H4zm9 0h7v7h-7zM4 13h7v7H4zm9 0h7v7h-7z"
              :opacity="row.node.solidifyAll ? 1 : 0.55"
            />
          </svg>
          <span class="ct-tab-label">{{ titleOf(row.node) }}</span>
          <button class="ct-tab-action" title="more" @click.stop="rowMenu(row.node, $event)">
            ⋯
          </button>
          <button class="ct-tab-action" title="close" @click.stop="close(row.node.id)">✕</button>
        </li>
        <li class="ct-new-row" role="none">
          <button class="ct-new" title="a new tab" @click="newMenu($event)">＋ New…</button>
        </li>
      </ul>
    </nav>
    <section class="ct-main">
      <ContainerNode
        v-if="selectedTab"
        :key="selectedTab.id"
        :node="selectedTab"
        parent-layout="tabs"
      />
    </section>

    <div class="ct-pool" aria-hidden="true">
      <Teleport
        v-for="card in pool"
        :key="card.id"
        :to="slots.get(card.id)"
        :disabled="!slots.get(card.id)"
      >
        <ShadowCard class="ct-mounted-card" :source="card.source" :ctx="ctxFor(card)" />
      </Teleport>
    </div>

    <ul
      v-if="menu"
      class="ct-menu"
      role="menu"
      :style="{ left: menu.x + 'px', top: menu.y + 'px' }"
    >
      <template v-for="(item, i) in menu.items" :key="i">
        <li v-if="item === 'separator'" class="ct-menu-sep" role="separator" />
        <li v-else role="menuitem" @click="runItem(item)">
          <span class="ct-menu-check">{{ item.checked ? "✓" : "" }}</span
          >{{ item.label }}
        </li>
      </template>
    </ul>

    <div v-if="asking" class="ct-ask-backdrop" @click.self="asking.done(null)">
      <form class="ct-ask" @submit.prevent="asking.done(asking.value)">
        <label class="ct-ask-title" for="ct-ask-input">{{ asking.title }}</label>
        <input
          id="ct-ask-input"
          v-model="asking.value"
          v-focus-select
          @keydown.esc.prevent="asking.done(null)"
        />
        <div class="ct-ask-buttons">
          <button type="button" @click="asking.done(null)">Cancel</button>
          <button type="submit" class="ct-ask-ok">OK</button>
        </div>
      </form>
    </div>
  </div>
</template>

<style scoped>
.ct-root {
  display: flex;
  flex: 1 1 0;
  min-height: 0;
}
.ct-sidebar {
  flex: 0 0 240px;
  display: flex;
  flex-direction: column;
  min-height: 0;
  background: var(--datalib-sidebar);
  border-right: 1px solid var(--datalib-border);
}
.ct-tabs {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  margin: 0;
  padding: var(--datalib-pad) 6px;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.ct-tab {
  display: flex;
  align-items: center;
  gap: 6px;
  height: var(--datalib-row-h);
  padding-right: 4px;
  cursor: pointer;
  border-radius: var(--datalib-radius);
  color: var(--datalib-fg);
}
.ct-tab:hover {
  background: var(--datalib-hover);
}
.ct-tab.is-selected {
  background: var(--datalib-selected);
  font-weight: 600;
}
.ct-tab-icon {
  flex: 0 0 auto;
  width: var(--datalib-icon-size);
  height: var(--datalib-icon-size);
  color: var(--datalib-muted);
}
.ct-tab.is-selected .ct-tab-icon {
  color: var(--datalib-accent);
}
.ct-tab-label {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ct-tab-action {
  flex: 0 0 auto;
  visibility: hidden;
  padding: 0 0.2rem;
  border: none;
  border-radius: 3px;
  background: transparent;
  color: var(--datalib-muted);
  cursor: pointer;
  font-size: 11px;
}
.ct-tab:hover .ct-tab-action,
.ct-tab.is-selected .ct-tab-action {
  visibility: visible;
}
.ct-tab-action:hover {
  background: var(--datalib-hover);
  color: var(--datalib-fg);
}
.ct-new {
  width: 100%;
  height: var(--datalib-row-h);
  box-sizing: border-box;
  padding: 0 6px;
  cursor: pointer;
  font: inherit;
  text-align: left;
  color: var(--datalib-muted);
  background: transparent;
  border: 1px dashed var(--datalib-border);
  border-radius: var(--datalib-radius);
}
.ct-new:hover {
  color: var(--datalib-fg);
  background: var(--datalib-hover);
}
.ct-main {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--datalib-surface);
  overflow: hidden;
}
.ct-pool {
  display: none;
}
.ct-mounted-card {
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
}
.ct-menu {
  position: fixed;
  z-index: 20;
  margin: 0;
  padding: 4px;
  list-style: none;
  min-width: 13rem;
  background: var(--datalib-surface);
  color: var(--datalib-fg);
  border: 1px solid var(--datalib-border);
  border-radius: calc(var(--datalib-radius) + 2px);
  box-shadow: 0 10px 28px rgba(0, 0, 0, 0.18);
}
.ct-menu li[role="menuitem"] {
  padding: 4px 8px 4px 2px;
  border-radius: var(--datalib-radius);
  cursor: pointer;
  white-space: nowrap;
}
.ct-menu li[role="menuitem"]:hover {
  background: var(--datalib-hover);
}
.ct-menu-check {
  display: inline-block;
  width: 1.2em;
  text-align: center;
}
.ct-menu-sep {
  height: 1px;
  margin: 4px 6px;
  background: var(--datalib-border-soft);
}
.ct-ask-backdrop {
  position: fixed;
  inset: 0;
  z-index: 30;
  background: rgba(0, 0, 0, 0.3);
  display: flex;
  align-items: center;
  justify-content: center;
}
.ct-ask {
  width: min(360px, 90vw);
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 16px;
  background: var(--datalib-bg);
  color: var(--datalib-fg);
  border: 1px solid var(--datalib-border);
  border-radius: calc(var(--datalib-radius) + 4px);
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.22);
}
.ct-ask-title {
  font-weight: 600;
}
.ct-ask input {
  font: inherit;
  padding: 4px 6px;
  border: 1px solid var(--datalib-border);
  border-radius: var(--datalib-radius);
  background: var(--datalib-input-bg);
  color: var(--datalib-fg);
}
.ct-ask-buttons {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
.ct-ask-buttons button {
  font: inherit;
  padding: 3px 12px;
  border: 1px solid var(--datalib-border);
  border-radius: var(--datalib-radius);
  background: var(--datalib-surface);
  color: var(--datalib-fg);
  cursor: pointer;
}
.ct-ask-buttons .ct-ask-ok {
  background: var(--datalib-accent);
  border-color: var(--datalib-accent);
  color: var(--datalib-on-accent);
}
</style>
