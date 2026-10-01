<script setup lang="ts">
// One node of the containers layout, drawn inside its parent: a card
// (a slot its DOM is teleported into, under a header when chrome shows)
// or a container laying out its children. Recursive; everything that
// changes the tree goes through the host (containersApi.ts).
import { computed, inject } from "vue";
import CardControls from "@/components/CardControls.vue";
import { growSourceBox, vAutoGrow } from "@/components/autoGrow";
import { devMode } from "@/devMode";
import {
  DEFAULT_COLUMN,
  LAYOUTS,
  LAYOUT_LABELS,
  type Layout,
  type TreeNode,
} from "@/views/containerTree";
import { CONTAINERS_API } from "@/views/containersApi";

// `height`: the fixed height a stack gives this node. It sizes the
// node's body, not its header, so showing chrome never squeezes a card.
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
    :class="[`ct-box--${node.layout}`, { 'is-solid': solid, 'is-dev': devMode }]"
    :data-box-id="node.id"
  >
    <div v-if="chrome" class="ct-box-head">
      <span class="ct-box-name">{{ api.titleOf(node) }}</span>
      <div class="ct-seg" role="group" aria-label="container layout">
        <button
          v-for="l in LAYOUTS"
          :key="l"
          :class="{ 'is-active': node.layout === l }"
          :aria-pressed="node.layout === l"
          @click="api.setLayout(node.id, l)"
        >
          {{ LAYOUT_LABELS[l] }}
        </button>
      </div>
      <label class="ct-flag" title="cards opened from inside land in the next container out">
        <input
          type="checkbox"
          :checked="node.solidified"
          @change="api.toggleFlag(node, 'solidified')"
        />
        Solidified
      </label>
      <label
        class="ct-flag"
        title="this container and everything inside it count as solidified, and look finished outside dev mode"
      >
        <input
          type="checkbox"
          :checked="node.solidifyAll"
          @change="api.toggleFlag(node, 'solidifyAll')"
        />
        Solidify all
      </label>
      <span class="ct-spacer" />
      <span v-if="solid" class="ct-badge">solidified</span>
      <button class="ct-icon-btn" title="add a card" @click="api.addCard(node.id)">＋</button>
      <button class="ct-icon-btn" title="more" @click="api.openMenu($event, api.boxMenu(node))">
        ⋯
      </button>
    </div>

    <div v-if="node.layout === 'tabs'" class="ct-tabs-body" data-body :style="bodyStyle">
      <div class="ct-strip" role="tablist">
        <button
          v-for="child in node.children"
          :key="child.id"
          class="ct-strip-tab"
          :class="{ 'is-selected': child.id === node.selected }"
          role="tab"
          :aria-selected="child.id === node.selected"
          @click="api.select(child.id)"
        >
          {{ api.titleOf(child) }}
          <span
            v-if="chrome"
            class="ct-strip-close"
            role="button"
            aria-label="close"
            @click.stop="api.close(child.id)"
            >✕</span
          >
        </button>
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
.ct-card-head,
.ct-box-head {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 3px 6px;
  border-bottom: 1px solid var(--datalib-border-soft);
  background: var(--datalib-bg);
  font-size: var(--datalib-font-size-small);
}
.ct-box-head {
  background: var(--datalib-sidebar);
}
.ct-card-title,
.ct-box-name {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 600;
}
.ct-box-name {
  flex: 0 1 auto;
}
.ct-spacer {
  flex: 1 1 auto;
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
.ct-slot {
  flex: 1 1 auto;
  min-height: 0;
  display: flex;
}
.ct-box {
  border: 1px solid transparent;
}
/* In dev mode every container shows its edge, and a solidified one says so. */
.ct-box.is-dev {
  border-color: var(--datalib-border);
}
.ct-box.is-dev.is-solid {
  border-style: dashed;
  border-color: var(--datalib-accent);
}
.ct-badge {
  color: var(--datalib-accent);
  font-weight: 600;
}
.ct-seg {
  display: flex;
  border: 1px solid var(--datalib-border);
  border-radius: var(--datalib-radius);
  overflow: hidden;
}
.ct-seg button {
  border: none;
  background: var(--datalib-surface);
  color: var(--datalib-fg);
  font: inherit;
  padding: 1px 7px;
  cursor: pointer;
}
.ct-seg button + button {
  border-left: 1px solid var(--datalib-border);
}
.ct-seg button.is-active {
  background: var(--datalib-fg);
  color: var(--datalib-surface);
}
.ct-flag {
  display: flex;
  align-items: center;
  gap: 3px;
  white-space: nowrap;
  cursor: pointer;
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
  gap: 6px;
  max-width: 14rem;
  padding: 3px 8px;
  border: 1px solid transparent;
  border-bottom: none;
  border-radius: var(--datalib-radius) var(--datalib-radius) 0 0;
  background: transparent;
  color: var(--datalib-muted);
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
  font-size: 10px;
  color: var(--datalib-muted);
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
