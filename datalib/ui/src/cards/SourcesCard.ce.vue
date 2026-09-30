<script setup lang="ts">
// The Sources card as it has always looked: the typed table over the
// manage rows. Its logic lives in sourcesCardModel.ts, shared with the
// redesigned `sourcesNextView()`.
import type { CardCtx } from "./types";
import TableGrid from "./TableGrid.ce.vue";
import { ACTION_ICONS } from "./typedColumns";
import SourceWizard from "@/components/SourceWizard.vue";
import ConfirmDialog from "@/components/ConfirmDialog.vue";
import { SOURCES_HELP, useSourcesCard } from "./sourcesCardModel";

const props = defineProps<{ ctx: CardCtx }>();

props.ctx.setTitle("Sources");
props.ctx.setHelp(SOURCES_HELP);

const {
  cardEl,
  banner,
  busy,
  parseError,
  configError,
  loadError,
  configPath,
  droppedRows,
  emptyDiagnosis,
  manage,
  rows,
  syncAll,
  actions,
  openConfig,
  openAdd,
  contextMenuItems,
  isGroupOpenByDefault,
  onGridReady,
  onCellDoubleClicked,
  onCellEdit,
  onRowGroupOpened,
  wizardOpen,
  wizardKey,
  takenIds,
  editing,
  closeWizard,
  onWizardSubmit,
  asking,
} = useSourcesCard(props.ctx);
</script>

<template>
  <section ref="cardEl" class="m2 m2-card">
    <header class="m2-head">
      <!-- Here rather than above the table: it comes and goes with each
           sync, and a line appearing above the table moves every row
           under the pointer. -->
      <p
        v-if="banner"
        class="m2-msg m2-head-msg"
        :class="banner.ok ? 'good' : 'bad'"
        :title="banner.text"
        role="status"
      >
        {{ banner.text }}
      </p>
      <div class="m2-head-actions">
        <button
          class="m2-btn m2-runall"
          :class="{ danger: syncAll.glyph === 'stop' }"
          :disabled="busy || !!parseError || !!configError || !!syncAll.blocked"
          :title="syncAll.blocked ?? syncAll.label"
          :aria-label="syncAll.label"
          @click="
            syncAll.stops.length > 0 ? actions.stopSyncs(syncAll.stops) : actions.runEverything()
          "
        >
          <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
            <path fill="currentColor" :d="ACTION_ICONS[syncAll.glyph]" />
          </svg>
        </button>
        <button class="m2-add" :disabled="busy || !!parseError || !!configError" @click="openAdd">
          + Data Source
        </button>
      </div>
    </header>

    <p v-if="loadError" class="m2-msg bad">Could not load the config: {{ loadError }}</p>
    <p v-if="parseError" class="m2-msg bad">
      The config doesn’t parse, so the table below can’t be trusted: {{ parseError }}
    </p>
    <div v-else-if="configError" class="m2-msg bad m2-invalid">
      <b>datalib won’t run this config.</b>
      <span>{{ configError }}</span>
      <span class="m2-invalid-why">
        It parses as TOML, so the table below still reflects it — but nothing will sync, and applets
        won’t start, until this is fixed. Open the config to edit it.
      </span>
      <button class="m2-btn" @click="openConfig">Show the config</button>
    </div>
    <!-- Entries the loader dropped. Not a whole-config error: the rest
         of the pipeline is running, which is why this is a note above a
         working table rather than a screen in front of it. The per-row
         Last update column carries each reason; this says how many and where
         to look, because a dropped row is easy to scroll past. -->
    <div v-else-if="droppedRows.length" class="m2-msg bad m2-invalid">
      <b>
        {{ droppedRows.length }}
        {{ droppedRows.length === 1 ? "entry isn’t" : "entries aren’t" }} in the pipeline.
      </b>
      <span class="m2-invalid-why">
        The rest of this config loaded and still syncs. These are in the file and were not loaded —
        each one’s Last update cell says why. Open the config to fix them, or run
        <code>datalib-dag --check {{ configPath }}</code
        >.
      </span>
      <ul class="m2-dropped">
        <li v-for="r in droppedRows" :key="r.id">
          <code>{{ r.id }}</code> — {{ r.dropped?.message }}
        </li>
      </ul>
      <button class="m2-btn" @click="openConfig">Show the config</button>
    </div>

    <div class="m2-grid">
      <!-- The typed viewer over the rows the server assembled: a tree,
           one group row with its steps and applets under it. Every row
           is rendered: a config is tens of rows, and a source just
           added lands at the bottom, where a virtualized grid would
           have no row for it until scrolled to. -->
      <TableGrid
        :columns="manage?.columns ?? []"
        :rows="rows"
        :tree="true"
        :virtualizeRows="false"
        :actions="actions.buttons"
        :menu="contextMenuItems"
        :selectable="true"
        :openByDefault="isGroupOpenByDefault"
        :pinnedColumns="1"
        @ready="onGridReady"
        @cellDoubleClick="onCellDoubleClicked"
        @edit="onCellEdit"
        @rowGroupOpened="onRowGroupOpened"
      />
    </div>

    <div class="m2-foot">
      <div v-if="emptyDiagnosis && !parseError" class="m2-msg bad m2-invalid">
        <b>This table is empty, and it shouldn’t be.</b>
        <span>{{ emptyDiagnosis }}</span>
        <button class="m2-btn" @click="openConfig">Show the config</button>
      </div>
      <p v-else-if="rows.length === 0 && !parseError" class="m2-empty">
        Nothing configured yet. The <b>+ Data Source</b> button walks you through one.
      </p>
    </div>

    <!-- The panels, out of the shadow root: their components' scoped
         styles live in the head, and a modal belongs over the whole
         page anyway. -->
    <Teleport to="body">
      <SourceWizard
        v-if="wizardOpen"
        :key="wizardKey"
        :taken-ids="takenIds"
        :editing="editing"
        @close="closeWizard"
        @submit="onWizardSubmit"
      />
      <ConfirmDialog
        v-if="asking"
        title="Remove"
        :message="asking.message"
        :check-label="asking.checkLabel"
        confirm-label="Remove"
        @answer="asking.resolve"
      />
    </Teleport>
  </section>
</template>
