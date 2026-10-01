<script setup lang="ts">
// The Dashboard card: where a person starts. What needs them, how big the
// library is, each source's state, and the newest documents — all read
// from endpoints other cards already use (the manage rows and the
// search), so it adds no backend of its own.
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { UNIFIED_INDEX, type ManageResponse, type ManageRow, type SearchRow } from "@/api";
import { useApi } from "@/cards/cardApi";
import type { CardCtx } from "./types";
import { changed, subscribeLive } from "@/live";
import { iconUrl } from "@/config/icons";
import { formatBytes } from "@/config/bytes";
import { formatRelative } from "@/config/timeFormat";
import { syncAllButton } from "@/config/syncAll";
import { isDesktopApp, revealActionLabel, revealInFileManager } from "@/desktop";
import { copyToClipboard } from "@/clipboard";
import { pushToast } from "@/toasts";
import { logSource } from "./libs/logView";
import { statusTone, needsYou, type Tone } from "./dashboard";

const props = defineProps<{ ctx: CardCtx }>();
const api = useApi();

props.ctx.setTitle("Dashboard");

const manage = ref<ManageResponse | null>(null);
const recent = ref<SearchRow[]>([]);
const loadError = ref<string | null>(null);
const now = ref(Date.now());

async function loadRows() {
  try {
    manage.value = await api.fetchManageRows();
    loadError.value = null;
  } catch (e) {
    loadError.value = (e as Error).message;
  }
}

async function loadRecent() {
  try {
    recent.value = (await api.fetchSearch("is:document", 12)).rows;
  } catch {
    // No index yet, or the applet is starting: the panel says so below.
    recent.value = [];
  }
}

const groups = computed(() => (manage.value?.rows ?? []).filter((r) => r.kind === "group"));
const sources = computed(() => groups.value.filter((r) => r.type !== null && !r.dropped));
const attention = computed(() => needsYou(groups.value));

const totalItems = computed(() => sources.value.reduce((n, r) => n + (r.items.value ?? 0), 0));
const rootBytes = computed(() => manage.value?.storage.root.bytes ?? 0);

/// The storage bar: one segment per source, largest first, then what
/// the index and the app's own stores take.
const segments = computed(() => {
  const total = rootBytes.value;
  if (total <= 0) return [];
  const bySource = sources.value
    .map((r) => ({ id: r.id, label: r.name.label, bytes: r.disk.value ?? 0 }))
    .filter((s) => s.bytes > 0)
    .sort((a, b) => b.bytes - a.bytes);
  const shown = bySource.slice(0, 5);
  const rest = total - shown.reduce((n, s) => n + s.bytes, 0);
  const out = shown.map((s, i) => ({ ...s, tone: `seg-${i}` }));
  if (rest > 0)
    out.push({ id: "rest", label: "Search index and app data", bytes: rest, tone: "seg-rest" });
  return out.map((s) => ({ ...s, pct: (100 * s.bytes) / total }));
});

const lastRun = computed(() => {
  const run = manage.value?.run;
  if (!run) return { tone: "muted" as Tone, text: "Not synced yet" };
  if (run.live) return { tone: "run" as Tone, text: "Syncing now" };
  return { tone: "ok" as Tone, text: `Synced ${formatRelative(run.finished_at, now.value)}` };
});

const syncAll = computed(() => syncAllButton(sources.value));

async function syncEverything() {
  const b = syncAll.value;
  try {
    if (b.stops.length > 0) for (const id of b.stops) await api.stopRequest(id);
    else await api.openRequest([]);
  } catch (e) {
    pushToast((e as Error).message);
  }
}

async function syncRow(row: ManageRow) {
  try {
    await api.openRequest(row.seeds);
  } catch (e) {
    pushToast((e as Error).message);
  }
}

function open(...sources: string[]) {
  props.ctx.host.openCards(...sources);
}

function openLog(row: ManageRow) {
  open(
    logSource({ step: row.status_from ?? row.id, run: row.last_run_id || null, jumpToEnd: true }),
  );
}

function openProblems(row: ManageRow) {
  const everything = row.id === "unified_index";
  const opts = {
    url: `${UNIFIED_INDEX}/problems`,
    q: everything ? "" : `source_id:${row.id}`,
    name: everything ? "Problems" : `Problems: ${row.name.label}`,
  };
  open(`gridView(${JSON.stringify(opts)})`);
}

function openDocument(row: SearchRow) {
  if (row.markdown_uuid) open(`documentView(${JSON.stringify(row.markdown_uuid)})`);
}

const canReveal = isDesktopApp();
const revealLabel = revealActionLabel();
async function showRoot() {
  const abs = manage.value?.storage.root.abs;
  if (!abs) return;
  if (canReveal) {
    await revealInFileManager(abs);
    return;
  }
  const ok = await copyToClipboard(abs);
  pushToast(ok ? "Data root path copied" : "Could not copy the path", ok ? "info" : "error");
}

const dateFormat = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
function when(iso: string | null): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return "";
  return now.value - t < 86_400_000 ? formatRelative(iso, now.value) : dateFormat.format(t);
}

function statusClass(row: ManageRow): string {
  return `tone-${statusTone(row.status.key)}`;
}

let stop: (() => void) | null = null;
let tick: ReturnType<typeof setInterval> | null = null;
onMounted(() => {
  void loadRows();
  void loadRecent();
  stop = subscribeLive({
    root: (e) => {
      if (changed(e, "manage.rows") || changed(e, "storage")) void loadRows();
      if (e.kind === "index_changed") void loadRecent();
    },
    resync: () => {
      void loadRows();
      void loadRecent();
    },
  });
  // "Synced 5 minutes ago" keeps counting while the card sits open.
  tick = setInterval(() => (now.value = Date.now()), 30_000);
});
onBeforeUnmount(() => {
  stop?.();
  if (tick) clearInterval(tick);
});
</script>

<template>
  <div class="dashboard">
    <header class="dashboard-head">
      <span class="dashboard-state" :class="`tone-${lastRun.tone}`">
        <span class="dot" />{{ lastRun.text }}
      </span>
      <button
        class="dashboard-btn"
        :disabled="!!syncAll.blocked"
        :title="syncAll.blocked ?? syncAll.label"
        @click="syncEverything"
      >
        {{ syncAll.stops.length > 0 ? "Stop syncing" : "Sync now" }}
      </button>
    </header>

    <div class="dashboard-body">
      <p v-if="loadError" class="dashboard-error">Could not load the sources: {{ loadError }}</p>

      <section v-if="attention.length" class="panel panel-warn" aria-label="Needs you">
        <h2 class="panel-head panel-head-warn">
          <span class="section-title">Needs you · {{ attention.length }}</span>
        </h2>
        <div v-for="a in attention" :key="a.row.id" class="row">
          <img
            v-if="iconUrl(a.row.name.icon)"
            class="tile"
            :src="iconUrl(a.row.name.icon)!"
            alt=""
          />
          <span class="row-text">
            <strong>{{ a.row.name.label }}</strong> {{ a.text }}
          </span>
          <button v-if="a.problems" class="link" @click="openProblems(a.row)">Review</button>
          <template v-if="a.failed">
            <button class="link" @click="openLog(a.row)">View log</button>
            <button class="dashboard-btn dashboard-btn-strong" @click="syncRow(a.row)">
              Sync again
            </button>
          </template>
        </div>
      </section>

      <section class="panel" aria-label="Your library">
        <h2 class="panel-head">
          <span class="section-title">Your library</span>
          <button class="link" @click="showRoot">
            {{ canReveal ? revealLabel : "Copy folder path" }}
          </button>
        </h2>
        <div class="library">
          <div class="stats">
            <span
              ><span class="stat">{{ totalItems.toLocaleString() }}</span> items</span
            >
            <span
              ><span class="stat">{{ formatBytes(rootBytes) }}</span> on disk</span
            >
          </div>
          <div
            v-if="segments.length"
            class="bar"
            role="img"
            :aria-label="`${formatBytes(rootBytes)} on disk`"
          >
            <span
              v-for="s in segments"
              :key="s.id"
              :class="s.tone"
              :style="{ width: s.pct + '%' }"
              :title="`${s.label}: ${formatBytes(s.bytes)}`"
            />
          </div>
          <div class="legend">
            <span v-for="s in segments" :key="s.id"
              ><span class="swatch" :class="s.tone" />{{ s.label }} {{ formatBytes(s.bytes) }}</span
            >
          </div>
        </div>
      </section>

      <section class="panel" aria-label="Sources">
        <h2 class="panel-head">
          <span class="section-title">Sources</span>
          <button class="link" @click="open('sourcesView()')">Open Sources</button>
        </h2>
        <div class="grid grid-head">
          <span /><span>Name</span><span>Status</span><span>Updated</span
          ><span class="num">Items</span><span class="num">Size</span>
        </div>
        <p v-if="manage && sources.length === 0" class="empty">
          No sources yet. Open Sources to add one.
        </p>
        <div v-for="r in sources" :key="r.id" class="grid grid-row">
          <img v-if="iconUrl(r.name.icon)" class="tile" :src="iconUrl(r.name.icon)!" alt="" />
          <span v-else />
          <span class="name" :title="r.name.detail ?? r.id">{{ r.name.label }}</span>
          <span class="status" :class="statusClass(r)" :title="r.status.detail ?? ''"
            ><span class="dot" />{{ r.status.label }}</span
          >
          <span class="muted">{{ when(r.status.at) }}</span>
          <span class="num">{{ r.items.value?.toLocaleString() ?? "" }}</span>
          <span class="num muted">{{ r.disk.value != null ? formatBytes(r.disk.value) : "" }}</span>
        </div>
      </section>

      <section class="panel" aria-label="Latest activity">
        <h2 class="panel-head">
          <span class="section-title">Latest activity</span>
          <button class="link" @click="open('searchView()')">Search everything</button>
        </h2>
        <p v-if="recent.length === 0" class="empty">Nothing indexed yet.</p>
        <button v-for="d in recent" :key="d.uuid" class="row recent" @click="openDocument(d)">
          <img
            v-if="iconUrl(d.source_ref?.icon)"
            class="tile tile-small"
            :src="iconUrl(d.source_ref?.icon)!"
            alt=""
          />
          <strong class="recent-title">{{ d.conversation_name || d.kind }}</strong>
          <span class="recent-snippet">{{ d.snippet }}</span>
          <span class="muted">{{ when(d.touched_at) }}</span>
        </button>
      </section>
    </div>
  </div>
</template>

<style scoped>
.dashboard {
  height: 100%;
  display: flex;
  flex-direction: column;
  font-size: var(--datalib-font-size);
  color: var(--datalib-fg);
  background: var(--datalib-surface-2);
}
.dashboard-head {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  padding: 6px var(--datalib-pad);
  background: var(--datalib-bg);
  border-bottom: 1px solid var(--datalib-border-soft);
}
.dashboard-body {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: var(--datalib-gap);
  padding: var(--datalib-pad);
}
.dashboard-state {
  display: flex;
  align-items: center;
  gap: 6px;
}
.dashboard-btn {
  height: var(--datalib-control-h);
  padding: 0 10px;
  font: inherit;
  font-weight: 600;
  border: 1px solid var(--datalib-border);
  border-radius: var(--datalib-radius);
  background: var(--datalib-bg);
  color: var(--datalib-fg);
  cursor: pointer;
}
.dashboard-btn:hover:not(:disabled) {
  background: var(--datalib-hover);
}
.dashboard-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
.dashboard-btn-strong {
  border-color: var(--datalib-accent);
  background: var(--datalib-accent);
  color: var(--datalib-on-accent);
}
.dashboard-btn-strong:hover {
  background: color-mix(in srgb, var(--datalib-accent) 85%, black);
}
.dashboard-error {
  margin: 0;
  color: var(--datalib-error-fg);
}
/* Small capitals over each panel. Declared here, not taken from the
   app's stylesheet: document styles stop at the card's shadow root. */
.section-title {
  font-size: var(--datalib-font-size-small);
  font-weight: 600;
  letter-spacing: 0.4px;
  text-transform: uppercase;
  color: var(--datalib-muted);
}
/* Clipped, so a tinted header keeps the panel's rounded corners. */
.panel {
  flex: 0 0 auto;
  background: var(--datalib-bg);
  border: 1px solid var(--datalib-border-soft);
  border-radius: var(--datalib-radius);
  overflow: hidden;
}
.panel-warn {
  border-color: var(--datalib-warn-border);
}
.panel-head {
  display: flex;
  align-items: center;
  gap: 8px;
  height: calc(var(--datalib-row-h) + 2px);
  margin: 0;
  padding: 0 10px;
  border-bottom: 1px solid var(--datalib-border-soft);
  font: inherit;
}
.panel-head .section-title {
  flex: 1 1 auto;
}
.panel-head-warn {
  background: var(--datalib-warn-bg);
  border-bottom-color: var(--datalib-warn-border);
}
.panel-head-warn .section-title {
  color: var(--datalib-warn-fg);
}
.row {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: calc(var(--datalib-row-h) + 4px);
  padding: 0 10px;
  border-top: 1px solid var(--datalib-border-soft);
}
.panel-head + .row {
  border-top: none;
}
.row-text {
  flex: 1 1 auto;
  min-width: 0;
}
.tile {
  flex: 0 0 auto;
  width: calc(var(--datalib-icon-size) + 4px);
  height: calc(var(--datalib-icon-size) + 4px);
}
.tile-small {
  width: var(--datalib-icon-size);
  height: var(--datalib-icon-size);
}
.link {
  padding: 0;
  border: 0;
  background: none;
  font: inherit;
  font-weight: 600;
  color: var(--datalib-accent);
  cursor: pointer;
}
.link:hover {
  text-decoration: underline;
}
.library {
  display: flex;
  flex-direction: column;
  gap: var(--datalib-gap);
  padding: calc(var(--datalib-pad) + 2px) 10px;
  color: var(--datalib-muted);
}
.stats {
  display: flex;
  gap: 40px;
}
.stat {
  font-size: calc(var(--datalib-title-size) + 4px);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  color: var(--datalib-fg);
}
.bar {
  display: flex;
  gap: 2px;
  height: 6px;
  border-radius: 3px;
  overflow: hidden;
}
.legend {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 16px;
}
.legend > span {
  display: flex;
  align-items: center;
  gap: 6px;
}
.swatch {
  width: 8px;
  height: 8px;
  border-radius: 2px;
}
/* The storage segments, largest first: one hue stepped in lightness,
   so the order reads without a legend lookup. */
.seg-0 {
  background: var(--datalib-accent);
}
.seg-1 {
  background: color-mix(in srgb, var(--datalib-accent) 72%, var(--datalib-bg));
}
.seg-2 {
  background: color-mix(in srgb, var(--datalib-accent) 52%, var(--datalib-bg));
}
.seg-3 {
  background: color-mix(in srgb, var(--datalib-accent) 36%, var(--datalib-bg));
}
.seg-4 {
  background: color-mix(in srgb, var(--datalib-accent) 24%, var(--datalib-bg));
}
.seg-rest {
  background: var(--datalib-border);
}
.grid {
  display: grid;
  grid-template-columns:
    calc(var(--datalib-icon-size) + 4px) minmax(0, 1.4fr) minmax(0, 1.6fr)
    110px 80px 90px;
  column-gap: 12px;
  align-items: center;
  padding: 0 10px;
}
.grid-head {
  height: calc(var(--datalib-row-h) - 2px);
  font-size: var(--datalib-font-size-small);
  color: var(--datalib-faint);
  border-bottom: 1px solid var(--datalib-border-soft);
}
.grid-row {
  min-height: calc(var(--datalib-row-h) + 4px);
  border-top: 1px solid var(--datalib-border-soft);
}
.grid-head + .grid-row {
  border-top: none;
}
.name {
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.status {
  display: flex;
  align-items: center;
  gap: 6px;
  white-space: nowrap;
  overflow: hidden;
}
.num {
  text-align: right;
  font-variant-numeric: tabular-nums;
}
.muted {
  color: var(--datalib-muted);
  white-space: nowrap;
}
.empty {
  margin: 0;
  padding: 10px;
  color: var(--datalib-muted);
}
.recent {
  width: 100%;
  font: inherit;
  color: inherit;
  text-align: left;
  background: none;
  border-left: 0;
  border-right: 0;
  border-bottom: 0;
  cursor: pointer;
}
.recent:hover {
  background: var(--datalib-hover);
}
.recent-title {
  flex: 0 1 auto;
  max-width: 40%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.recent-snippet {
  flex: 1 1 auto;
  min-width: 0;
  color: var(--datalib-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.dot {
  flex: 0 0 auto;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: currentColor;
}
.tone-ok {
  color: var(--datalib-ok);
}
.tone-ok .dot {
  background: var(--datalib-ok-dot);
}
.tone-run {
  color: var(--datalib-accent);
}
.tone-warn {
  color: var(--datalib-warn-fg);
}
.tone-warn .dot {
  background: var(--datalib-warn-dot);
}
.tone-error {
  color: var(--datalib-error-fg);
}
.tone-muted {
  color: var(--datalib-muted);
}
.tone-muted .dot {
  background: var(--datalib-border);
}
</style>
