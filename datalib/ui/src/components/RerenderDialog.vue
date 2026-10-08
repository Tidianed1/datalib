<script setup lang="ts">
// Asked once a launch has migrated the raw stores: the rendered
// documents of the sources listed are in a shape this build does not
// write, and re-rendering them reads what is already downloaded.
// Also says which raw stores could not be migrated, since that is news
// from the same launch.
import { computed, onMounted, ref } from "vue";
import type { Upgrade } from "@/api";
import { failedStores, rerenderSources } from "@/upgrade";

const props = defineProps<{ upgrade: Upgrade }>();

const emit = defineEmits<{
  (e: "answer", rerender: boolean): void;
}>();

const sources = computed(() => rerenderSources(props.upgrade));
const failed = computed(() => failedStores(props.upgrade));
const rerenderButton = ref<HTMLButtonElement | null>(null);

onMounted(() => rerenderButton.value?.focus());

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") emit("answer", false);
}
</script>

<template>
  <div class="dialog-backdrop" @keydown="onKeydown">
    <div
      class="rerender dialog"
      role="dialog"
      aria-modal="true"
      aria-label="Your rendered documents are out of date"
    >
      <header class="dialog-head">
        <h2>Your rendered documents are out of date</h2>
      </header>
      <div class="rerender-body dialog-body">
        <template v-if="sources.length">
          <p>
            This version of datalib lays out rendered documents differently. Until they are rendered
            again, these sources show what the older version wrote, and a sync re-renders only the
            source it syncs:
          </p>
          <ul class="sources">
            <li v-for="s in sources" :key="s">{{ s }}</li>
          </ul>
          <p>Re-rendering reads what is already downloaded. Nothing is downloaded.</p>
        </template>
        <template v-if="failed.length">
          <p class="failed-lead">
            These sources' downloads could not be updated for this version, so their syncs will fail
            until they are reset:
          </p>
          <ul class="sources failed">
            <li v-for="f in failed" :key="f.source">
              <strong>{{ f.source }}</strong>
              <span class="error">{{ f.error }}</span>
            </li>
          </ul>
        </template>
      </div>
      <footer class="dialog-foot">
        <button class="btn ghost" @click="emit('answer', false)">
          {{ sources.length ? "Not now" : "Close" }}
        </button>
        <button
          v-if="sources.length"
          ref="rerenderButton"
          class="btn primary"
          @click="emit('answer', true)"
        >
          Re-render now
        </button>
      </footer>
    </div>
  </div>
</template>

<style scoped src="./dialog.css"></style>
<style scoped>
.rerender {
  width: min(560px, 100%);
}
.rerender-body {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.rerender-body p {
  margin: 0;
}
.sources {
  margin: 0;
  padding-left: 1.25rem;
}
.failed .error {
  display: block;
  color: var(--datalib-error-fg);
  font-size: var(--datalib-font-size-small);
  overflow-wrap: anywhere;
}
</style>
