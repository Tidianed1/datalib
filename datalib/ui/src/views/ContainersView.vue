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
import ContainerMenu from "@/views/ContainerMenu.vue";
import NameDialog from "@/views/NameDialog.vue";
import { fetchUiState, putUiState } from "@/api";
import { createBus } from "@/cards/bus";
import { chainHref } from "@/cards/chainHref";
import { setCardHelp } from "@/cards/help";
import { cardType, newCardId } from "@/cards/cardId";
import { displayTitle } from "@/cards/title";
import { devMode } from "@/devMode";
import { decodeColumns } from "@/router/columns";
import { pushToast } from "@/toasts";
import { pageTitle } from "@/views/millerStack";
import { isMainWindow } from "@/views/tabsWindow";
import {
  BUILTIN_COMPOSITES,
  composite,
  isBuiltinComposite,
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

const props = defineProps<{
  // Whether this layout is on screen, and so owns the URL and the page title.
  active: boolean;
  // Page load into this layout: a URL naming cards is what the person
  // asked for, so open it.
  openUrlOnMount: boolean;
}>();

const route = useRoute();
const router = useRouter();
const bus = createBus();

const STATE_NAME = "layout";
// How long the tree sits unchanged before it is written: a resize drag
// or a burst of card state is one write, not dozens.
const SAVE_DELAY_MS = 400;

function dashboard(): TreeNode {
  return instantiate(BUILTIN_COMPOSITES.Dashboard, newCardId);
}

// The outermost container is always tabs, never solidified, and never
// empty: a tree left with no tabs gets the Dashboard back.
function settle(box: BoxNode): BoxNode {
  const fixed: BoxNode = { ...box, layout: "tabs", solidified: false, solidifyAll: false };
  if (fixed.children.length === 0) {
    const fresh = dashboard();
    return { ...fixed, children: [fresh], selected: fresh.id };
  }
  const selected = fixed.children.some((c) => c.id === fixed.selected)
    ? fixed.selected
    : fixed.children[0].id;
  return { ...fixed, selected };
}

const root = ref<BoxNode>(settle(makeBox(newCardId(), "tabs", [])));
const ready = ref(false);
let mainWindow = false;

function update(next: TreeNode) {
  if (next.kind === "box") root.value = settle(next);
}

// ---- keeping the tree ----

let saveTimer: ReturnType<typeof setTimeout> | null = null;
let saveFailed = false;

// Only the main window keeps its tree; a popped-out card's window is
// a scratch space that goes when it closes.
async function save(keepalive = false) {
  saveTimer = null;
  if (!mainWindow || !ready.value) return;
  try {
    await putUiState(STATE_NAME, root.value, { keepalive });
    saveFailed = false;
  } catch (e) {
    console.warn("could not keep the layout", e);
    // Once per run of failures, not once per change.
    if (!saveFailed) pushToast("Could not save the layout to the library.");
    saveFailed = true;
  }
}
watch(root, () => {
  if (!ready.value) return;
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => void save(), SAVE_DELAY_MS);
});
function flush() {
  if (!saveTimer) return;
  clearTimeout(saveTimer);
  void save(true);
}
window.addEventListener("pagehide", flush);
onBeforeUnmount(() => {
  window.removeEventListener("pagehide", flush);
  flush();
});

// The cards a URL names (a link, a popped-out card), as one node: a
// card, or a columns container for a chain. The tree, not the URL, is
// what this layout keeps, so once opened the address goes back to "/".
function routeNode(): TreeNode | null {
  const specs = decodeColumns(route.path);
  if (specs.length === 0) return null;
  const nodes = specs.map((s) => makeCard(newCardId(), s.code, s.state));
  return nodes.length === 1 ? nodes[0] : makeBox(newCardId(), "columns", nodes);
}

function openRoute() {
  const node = routeNode();
  if (!node) return;
  update(addChild(root.value, root.value.id, node));
  void router.replace("/");
}
watch(
  () => route.path,
  () => {
    if (ready.value && props.active) openRoute();
  },
);

async function start() {
  mainWindow = await isMainWindow();
  void loadComposites();
  let kept: BoxNode | null = null;
  if (mainWindow) {
    try {
      kept = parseTree(await fetchUiState(STATE_NAME));
    } catch (e) {
      console.warn("could not read the kept layout", e);
      pushToast("Could not read the saved layout from the library; starting afresh.");
    }
  }
  // A second window starts with nothing but the cards its URL names.
  const fromUrl = props.openUrlOnMount || !mainWindow ? routeNode() : null;
  let tree = kept ?? makeBox(newCardId(), "tabs", mainWindow ? [dashboard()] : []);
  if (fromUrl) tree = addChild(tree, tree.id, fromUrl) as BoxNode;
  root.value = settle(tree);
  ready.value = true;
  if (fromUrl) void router.replace("/");
}
void start();

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
      // A card resets its title to null each time it runs, then names
      // itself; keeping the last real name means an unmounted tab has one.
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
  if (node.name) return node.name;
  if (node.kind === "card") return displayTitle(node.source, node.title);
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

const asking = ref<{
  title: string;
  initial: string;
  check?: (name: string) => string | null;
  resolve: (name: string | null) => void;
} | null>(null);

function askName(
  title: string,
  initial: string,
  check?: (name: string) => string | null,
): Promise<string | null> {
  return new Promise((resolve) => {
    asking.value = { title, initial, check, resolve };
  });
}
function answer(name: string | null) {
  asking.value?.resolve(name);
  asking.value = null;
}

async function renameNode(node: TreeNode) {
  const name = await askName("Name", titleOf(node));
  if (name !== null) update(rename(root.value, node.id, name));
}

async function saveAsComposite(box: BoxNode) {
  const name = await askName("Save as composite", box.name ?? titleOf(box), (n) =>
    isBuiltinComposite(n) ? `"${n}" is a built-in composite; pick another name.` : null,
  );
  if (name === null) return;
  try {
    await saveComposite(name, box);
  } catch (e) {
    console.warn("could not save the composite", e);
    pushToast(`Could not save the composite "${name}" to the library.`);
    return;
  }
  update(setTemplate(root.value, box.id, name));
}

// ---- menus ----

const menu = ref<{ items: MenuItem[]; x: number; y: number } | null>(null);

function openMenu(ev: MouseEvent, items: MenuItem[]) {
  ev.preventDefault();
  ev.stopPropagation();
  menu.value = { items, x: ev.clientX, y: ev.clientY };
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

const compositeNames = computed(() =>
  Object.keys({ ...BUILTIN_COMPOSITES, ...savedComposites.value }),
);

function boxMenu(box: BoxNode): MenuItem[] {
  const items: MenuItem[] = [
    { label: "Add card", run: () => addCard(box.id) },
    ...LAYOUTS.map((layout) => ({
      label: `Add ${LAYOUT_LABELS[layout]} container`,
      run: () => addBox(box.id, layout),
    })),
    ...compositeNames.value.map((name) => ({
      label: `Add composite: ${name}`,
      run: () => addComposite(box.id, name),
    })),
  ];
  if (box.id === root.value.id) return items;
  items.push(
    "separator",
    { label: "Solidified", checked: box.solidified, run: () => toggleFlag(box, "solidified") },
    {
      label: "Solidify all",
      checked: box.solidifyAll,
      run: () => toggleFlag(box, "solidifyAll"),
    },
    "separator",
    { label: "Rename…", run: () => void renameNode(box) },
    { label: "Save as composite…", run: () => void saveAsComposite(box) },
  );
  const template = box.template ? composite(box.template) : undefined;
  if (template) {
    items.push({
      label: `Reset to "${box.template}"`,
      run: () => update(resetTo(root.value, box.id, template, newCardId)),
    });
  }
  items.push(
    "separator",
    ...placeItems(box),
    {
      label: "Take the cards out of this container",
      run: () => update(unwrap(root.value, box.id)),
    },
    { label: "Close", run: () => close(box.id) },
  );
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
    ...compositeNames.value.map((name) => ({
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
  if (!props.active) return;
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

    <ContainerMenu v-if="menu" :items="menu.items" :x="menu.x" :y="menu.y" @close="menu = null" />
    <NameDialog
      v-if="asking"
      :title="asking.title"
      :initial="asking.initial"
      :check="asking.check"
      @done="answer"
    />
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
</style>
