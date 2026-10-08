<script setup lang="ts">
// The blocking screen while a launch migrates the raw stores an older
// datalib wrote (docs/dev/plans/upgrade_on_launch.md). Nothing syncs
// until it is done; the page drops it on the `upgrade_changed` that says
// so.
import type { ConfigResponse, MigrateState } from "@/api";
import { stepSource } from "@/upgrade";

defineProps<{ config: ConfigResponse }>();

const said: Record<MigrateState, string> = {
  waiting: "Waiting",
  running: "Updating…",
  done: "Done",
  failed: "Could not update",
};
</script>

<template>
  <section class="upgrading notice">
    <div class="card" role="status" aria-live="polite">
      <h2>Updating your data for this version of datalib</h2>
      <p>
        This version stores some sources' downloads differently, so it is updating them before
        anything syncs. Nothing is downloaded while this runs.
      </p>
      <ul class="stores">
        <li v-for="s in config.upgrade.stores" :key="s.step" :data-state="s.state">
          <span class="source">{{ stepSource(s.step) }}</span>
          <span class="state">{{ said[s.state] }}</span>
          <span v-if="s.error" class="error">{{ s.error }}</span>
        </li>
      </ul>
    </div>
  </section>
</template>

<style scoped src="./notice.css"></style>
<style scoped>
.card {
  width: 100%;
  max-width: 48rem;
}
.stores {
  list-style: none;
  margin: 0.75rem 0 0;
  padding: 0;
}
.stores li {
  display: flex;
  flex-wrap: wrap;
  gap: 0.25rem 0.75rem;
  padding: 0.25rem 0;
}
.source {
  min-width: 12rem;
  font-weight: 500;
}
.state {
  color: var(--datalib-muted);
}
li[data-state="failed"] .state,
.error {
  color: var(--datalib-error-fg);
}
.error {
  flex-basis: 100%;
  font-size: var(--datalib-font-size-small);
  overflow-wrap: anywhere;
}
</style>
