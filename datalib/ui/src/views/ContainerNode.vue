<script setup lang="ts">
// One node of the containers layout, drawn inside its parent: a card
// (a slot its DOM is teleported into, under a header when chrome shows)
// or a container laying out its children. Recursive; everything that
// changes the tree goes through the host (containersApi.ts).
import { computed, inject } from "vue";
import CardControls from "@/components/CardControls.vue";
import { growSourceBox, vAutoGrow } from "@/components/autoGrow";
import { devMode } from "@/devMode";
import { DEFAULT_COLUMN, LAYOUT_LABELS, type Layout, type TreeNode } from "@/views/containerTree";
import { CONTAINERS_API } from "@/views/containersApi";

// `height`: the fixed height a stack gives this node. It sizes the
// node's body, not its header, so showing chrome never squeezes a card.
// Each layout's glyph, drawn as strokes on a 24px grid.
const LAYOUT_ICONS: Record<Layout, string> = {
  tabs: "M3 9h18v11H3zM3 9V5h8v4",
  stack: "M4 3h16v7H4zM4 14h16v7H4z",
  row: "M3 4h7v16H3zM14 4h7v16h-7z",
  columns: "M3 4h18v16H3zM9 4v16M15 4v16",
};

const props = defineProps<{ node: TreeNode; parentLayout: Layout; height?: number | null }>();
const api = inject(CONTAINERS_API)!;

const chrome = computed(() => api.chromeShown(props.node.id));
// A tab's name is on its tab, so a card directly in a tabs container
// shows a header only in dev mode, for its source.
const cardHead = computed(
  () =>
    props.node.kind === "card" &&
    (devMode.value || (chrome.value && props.parentLayout !== "tabs")),
);
// A container's edge is dashed while cards open into it, and a thick
// solid line once it is solidified (itself or from further out).
const solid = computed(() => props.node.kind === "box" && api.isSolidified(props.node.id));
const axis = computed(() =>
  props.node.kind === "box" && props.node.layout === "stack" ? "y" : "x",
);

const bodyStyle = computed(() => (props.height != null ? { flex: `0 0 ${props.height}px` } : {}));

function childStyle(child: TreeNode) {
  if (props.node.kind !== "box") return {};
  if (props.node.layout === "columns") {
    return { flex: `0 0 ${child.basis ?? DEFAULT_COLUMN}px` };
  }
  if (child.basis === null) return { flex: "1 1 0" };
  // A stack sizes the child's body (see `height`); a row its whole width.
  return props.node.layout === "stack" ? { flex: "0 0 auto" } : { flex: `0 0 ${child.basis}px` };
}

function hasHandle(i: number): boolean {
  if (props.node.kind !== "box") return false;
  if (props.node.layout === "columns") return true;
  return i < props.node.children.length - 1;
}

const slotRef = (el: unknown) => api.setSlot(props.node.id, (el as Element | null) ?? null);
</script>

<template>
  <div v-if="node.kind === 'card'" class="ct-card" :data-card-id="node.id">
    <div v-if="cardHead" class="ct-card-head">
      <textarea
        v-if="devMode"
        v-auto-grow
        class="ct-source"
        rows="1"
        :value="node.source"
        spellcheck="false"
        aria-label="card source"
        @input="growSourceBox($event.target as HTMLTextAreaElement)"
        @keydown.enter.exact.prevent="api.commitSource(node, $event)"
      />
      <span v-else class="ct-card-title">{{ api.titleOf(node) }}</span>
      <button
        class="ct-icon-btn"
        title="arrange this card"
        @click="api.openMenu($event, api.cardMenu(node))"
      >
        ⋯
      </button>
      <CardControls :source="node.source" :ctx="api.ctxFor(node)" />
    </div>
    <div :ref="slotRef" class="ct-slot" data-body :style="bodyStyle" />
  </div>

  <div
    v-else
    class="ct-box"
    :class="chrome ? ['ct-frame', `ct-frame--${node.layout}`, { 'is-solid': solid }] : []"
    :data-box-id="node.id"
  >
    <button
      v-if="chrome"
      class="ct-foldertab"
      :title="`${LAYOUT_LABELS[node.layout]} container: layout, solidifying and more`"
      @click="api.openMenu($event, api.boxMenu(node))"
    >
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path :d="LAYOUT_ICONS[node.layout]" />
      </svg>
      {{ LAYOUT_LABELS[node.layout] }}<template v-if="node.name"> · {{ node.name }}</template>
      <span v-if="solid" class="ct-foldertab-state">solidified</span>
      ▾
    </button>

    <div v-if="node.layout === 'tabs'" class="ct-tabs-body" data-body :style="bodyStyle">
      <div class="ct-strip" role="tablist">
        <div
          v-for="child in node.children"
          :key="child.id"
          class="ct-strip-tab"
          :class="{ 'is-selected': child.id === node.selected }"
        >
          <button
            class="ct-strip-name"
            role="tab"
            :aria-selected="child.id === node.selected"
            @click="api.select(child.id)"
          >
            {{ api.titleOf(child) }}
          </button>
          <button
            v-if="chrome"
            class="ct-strip-close"
            :aria-label="`close ${api.titleOf(child)}`"
            @click="api.close(child.id)"
          >
            ✕
          </button>
        </div>
      </div>
      <template v-for="child in node.children" :key="child.id">
        <ContainerNode
          v-if="child.id === node.selected"
          class="ct-fill"
          :node="child"
          parent-layout="tabs"
        />
      </template>
    </div>
    <div
      v-else
      class="ct-children"
      :class="`ct-children--${node.layout}`"
      data-body
      :style="bodyStyle"
    >
      <template v-for="(child, i) in node.children" :key="child.id">
        <div class="ct-child" :style="childStyle(child)">
          <ContainerNode
            :node="child"
            :parent-layout="node.layout"
            :height="node.layout === 'stack' ? child.basis : null"
          />
        </div>
        <div
          v-if="hasHandle(i)"
          class="ct-handle"
          :class="`ct-handle--${axis}`"
          role="separator"
          :aria-orientation="axis === 'x' ? 'vertical' : 'horizontal'"
          title="drag to resize"
          @pointerdown="api.startResize(child.id, axis, $event)"
        />
      </template>
    </div>
  </div>
</template>

<style scoped>
.ct-card,
.ct-box,
.ct-fill,
.ct-tabs-body {
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.ct-card-head {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 3px 6px;
  border-bottom: 1px solid var(--datalib-border-soft);
  background: var(--datalib-bg);
  font-size: var(--datalib-font-size-small);
}
.ct-card-title {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 600;
}
.ct-source {
  flex: 1 1 auto;
  min-width: 0;
  font: 12px/1.5 var(--datalib-mono);
  padding: 0.1rem 0.3rem;
  border: none;
  background: transparent;
  color: inherit;
  resize: none;
  overflow: hidden;
  white-space: pre-wrap;
  overflow-wrap: break-word;
}
.ct-source:focus {
  outline: 1px solid var(--datalib-accent);
}
/* A container with chrome is a frame coloured by its layout, with a
   folder tab on its top edge that opens its menu. */
.ct-frame {
  --ct-color: #4f5d7a;
  --ct-edge: 2px;
  position: relative;
  margin: 22px 3px 3px;
  padding: 5px;
  border: var(--ct-edge) dashed var(--ct-color);
  border-radius: 0 7px 7px 7px;
}
.ct-frame.is-solid {
  --ct-edge: 3px;
  border-style: solid;
}
.ct-frame--tabs {
  --ct-color: #a8601c;
}
.ct-frame--stack {
  --ct-color: #1d7a72;
}
.ct-frame--row {
  --ct-color: #7a4fb0;
}
.ct-frame--columns {
  --ct-color: #4f5d7a;
}
.ct-foldertab {
  position: absolute;
  left: calc(-1 * var(--ct-edge));
  top: -22px;
  max-width: calc(100% + 2 * var(--ct-edge));
  height: 21px;
  box-sizing: border-box;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 0 9px;
  border: none;
  border-radius: 6px 6px 0 0;
  background: var(--ct-color);
  color: #fff;
  font: inherit;
  font-size: var(--datalib-font-size-small);
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  cursor: pointer;
}
.ct-foldertab svg {
  flex: 0 0 auto;
  width: 12px;
  height: 12px;
  fill: none;
  stroke: currentColor;
  stroke-width: 2.2;
  stroke-linejoin: round;
}
.ct-foldertab-state {
  padding: 0 4px;
  border-radius: 3px;
  background: rgba(255, 255, 255, 0.22);
}
.ct-slot {
  flex: 1 1 auto;
  min-height: 0;
  display: flex;
}
.ct-icon-btn {
  flex: 0 0 auto;
  border: none;
  background: transparent;
  color: var(--datalib-muted);
  cursor: pointer;
  font: inherit;
  padding: 0 4px;
  border-radius: 3px;
}
.ct-icon-btn:hover {
  background: var(--datalib-hover);
  color: var(--datalib-fg);
}
.ct-strip {
  flex: 0 0 auto;
  display: flex;
  gap: 2px;
  padding: 3px 4px 0;
  border-bottom: 1px solid var(--datalib-border);
  background: var(--datalib-bg);
  overflow-x: auto;
}
.ct-strip-tab {
  display: flex;
  align-items: center;
  gap: 2px;
  max-width: 14rem;
  padding: 0 4px;
  border: 1px solid transparent;
  border-bottom: none;
  border-radius: var(--datalib-radius) var(--datalib-radius) 0 0;
  color: var(--datalib-muted);
}
.ct-strip-name {
  min-width: 0;
  padding: 3px 4px;
  border: none;
  background: transparent;
  color: inherit;
  font: inherit;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  cursor: pointer;
}
.ct-strip-tab.is-selected {
  background: var(--datalib-surface);
  border-color: var(--datalib-border);
  color: var(--datalib-fg);
  font-weight: 600;
}
.ct-strip-close {
  padding: 0 3px;
  border: none;
  background: transparent;
  font-size: 10px;
  color: var(--datalib-muted);
  cursor: pointer;
}
.ct-children {
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
  display: flex;
}
.ct-children--stack {
  flex-direction: column;
}
.ct-children--columns {
  overflow-x: auto;
}
.ct-child {
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.ct-handle {
  flex: 0 0 5px;
  background: var(--datalib-border-soft);
}
.ct-handle--x {
  cursor: col-resize;
}
.ct-handle--y {
  cursor: row-resize;
}
.ct-handle:hover {
  background: var(--datalib-accent);
}
</style>
