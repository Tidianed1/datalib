<script setup lang="ts">
// The containers layout's popup menu, opened at the pointer. It measures
// itself once drawn and moves to stay on screen, and closes on a click
// outside it, on Escape, or when the window loses focus.
import { nextTick, onBeforeUnmount, onMounted, ref, useTemplateRef } from "vue";
import type { MenuItem } from "@/views/containersApi";

const props = defineProps<{ items: MenuItem[]; x: number; y: number }>();
const emit = defineEmits<{ close: [] }>();

const el = useTemplateRef<HTMLUListElement>("el");
const pos = ref({ left: props.x, top: props.y });
const MARGIN = 4;

onMounted(async () => {
  await nextTick();
  const box = el.value?.getBoundingClientRect();
  if (box) {
    pos.value = {
      left: Math.max(MARGIN, Math.min(props.x, window.innerWidth - box.width - MARGIN)),
      top: Math.max(MARGIN, Math.min(props.y, window.innerHeight - box.height - MARGIN)),
    };
  }
  window.addEventListener("pointerdown", onPointerOutside, true);
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("blur", close);
});
onBeforeUnmount(() => {
  window.removeEventListener("pointerdown", onPointerOutside, true);
  window.removeEventListener("keydown", onKey, true);
  window.removeEventListener("blur", close);
});

function close() {
  emit("close");
}
function onPointerOutside(ev: PointerEvent) {
  if (!el.value?.contains(ev.target as Node)) close();
}
function onKey(ev: KeyboardEvent) {
  if (ev.key === "Escape") close();
}
function run(item: MenuItem) {
  close();
  if (item !== "separator") item.run();
}
</script>

<template>
  <ul ref="el" class="ct-menu" role="menu" :style="{ left: pos.left + 'px', top: pos.top + 'px' }">
    <template v-for="(item, i) in items" :key="i">
      <li v-if="item === 'separator'" class="ct-menu-sep" role="separator" />
      <li
        v-else
        :role="item.checked === undefined ? 'menuitem' : 'menuitemcheckbox'"
        :aria-checked="item.checked"
        @click="run(item)"
      >
        <span class="ct-menu-check">{{ item.checked ? "✓" : "" }}</span
        >{{ item.label }}
      </li>
    </template>
  </ul>
</template>

<style scoped>
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
.ct-menu li:not(.ct-menu-sep) {
  padding: 4px 8px 4px 2px;
  border-radius: var(--datalib-radius);
  cursor: pointer;
  white-space: nowrap;
}
.ct-menu li:not(.ct-menu-sep):hover {
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
</style>
