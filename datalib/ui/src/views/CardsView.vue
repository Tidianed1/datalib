<script setup lang="ts">
// Routed view for the card surface: the containers layout, and the
// status bar along the bottom — the data root and its size, the log,
// then the density and dev toggles flush right.
import { onBeforeUnmount, onMounted, useTemplateRef } from "vue";
import ContainersView from "@/views/ContainersView.vue";
import RootStorageBar from "@/components/RootStorageBar.vue";
import { editMode } from "@/editMode";
import { density, DENSITIES } from "@/density";
import { LOG_CARD, surface, type SurfaceCommands } from "@/surface";

// The toolbar's commands go to the layout.
const containers = useTemplateRef<SurfaceCommands>("containers");
onMounted(() => {
  surface.value = {
    addCard: () => containers.value?.addCard(),
    showCard: (source) => containers.value?.showCard(source),
  };
});
onBeforeUnmount(() => {
  surface.value = null;
});
</script>

<template>
  <div class="cards-root">
    <ContainersView ref="containers" />
    <div class="cards-statusbar">
      <RootStorageBar />
      <!-- What the system is doing belongs down here with the data
           root's size: the log over every run, revealed if a card
           already shows it. -->
      <button
        class="cards-logs"
        title="the run log: every line the runner, the steps and the server wrote"
        @click="containers?.showCard(LOG_CARD)"
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
      <button
        class="cards-edit-toggle"
        :class="{ 'is-active': editMode }"
        :aria-pressed="editMode"
        title="edit mode: show and edit each card's source, and every container, solidified ones included"
        @click="editMode = !editMode"
      >
        Edit
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
.cards-edit-toggle {
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
.cards-edit-toggle:hover {
  background: var(--datalib-hover);
}
.cards-edit-toggle.is-active {
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
