<script setup lang="ts">
// One small time chart on the sync dashboard: a step function per line
// over the run, every chart on the card sharing the run's time axis and
// one crosshair. The header reads the value under the crosshair, or the
// latest when there is none; a legend names each line, with its value,
// when there is more than one. The decisions are `dashboardCharts.ts`.
import { computed, onMounted, onUnmounted, ref } from "vue";
import { formatBytes } from "@/config/bytes";
import { formatClock } from "@/config/timeFormat";
import { stepPath, valueAt, yRange, type Chart, type Line } from "./dashboardCharts";

const props = defineProps<{
  chart: Chart;
  /// The run, in ms: every chart on the card spans the same.
  domain: [number, number];
  /// Where the lines stop being carried forward: the step's finish, or now.
  end: number;
  /// The shared crosshair, in ms, or null.
  hover: number | null;
}>();
const emit = defineEmits<{ hover: [t: number | null] }>();

const HEIGHT = 92;
/// The top pad holds the axis maximum, above the line rather than under it.
const PAD = { top: 14, right: 8, bottom: 16, left: 8 };

const box = ref<HTMLElement | null>(null);
const width = ref(280);
let observer: ResizeObserver | null = null;
onMounted(() => {
  if (!box.value) return;
  observer = new ResizeObserver(([entry]) => {
    width.value = Math.max(120, Math.floor(entry.contentRect.width));
  });
  observer.observe(box.value);
});
onUnmounted(() => observer?.disconnect());

const range = computed(() => yRange(props.chart, props.domain));
const x = (t: number) => {
  const [a, b] = props.domain;
  const w = width.value - PAD.left - PAD.right;
  return PAD.left + (b > a ? ((t - a) / (b - a)) * w : 0);
};
const y = (v: number) => {
  const [lo, hi] = range.value;
  const h = HEIGHT - PAD.top - PAD.bottom;
  return PAD.top + h - ((v - lo) / (hi - lo || 1)) * h;
};

const paths = computed(() =>
  props.chart.lines.map((l) => ({ line: l, d: stepPath(l.points, props.end, x, y) })),
);

const ends = computed(() =>
  props.chart.lines.flatMap((l) => {
    const last = l.points.at(-1);
    return last
      ? [{ line: l, x: x(Math.max(last.t, Math.min(props.end, props.domain[1]))), y: y(last.v) }]
      : [];
  }),
);

const format = (v: number | null) =>
  v === null ? "—" : props.chart.unit === "bytes" ? formatBytes(v) : v.toLocaleString();

/// What the header and legend read: under the crosshair, or the latest.
const readAt = computed(() =>
  props.hover === null ? props.end : Math.min(props.hover, props.end),
);
const valueOf = (l: Line) => valueAt(l.points, readAt.value);

const headline = computed(() => {
  const [first] = props.chart.lines;
  if (props.chart.lines.length !== 1 || !first) return null;
  return format(valueOf(first));
});

function colorVar(l: Line): string {
  const c = l.color;
  if ("slot" in c) return `var(--viz-series-${c.slot + 1})`;
  if ("status" in c)
    return c.status === "warn" ? "var(--datalib-log-warn)" : "var(--datalib-log-error)";
  return "var(--datalib-accent)";
}

const crosshairX = computed(() =>
  props.hover !== null && props.hover >= props.domain[0] && props.hover <= props.domain[1]
    ? x(props.hover)
    : null,
);

function onMove(e: PointerEvent) {
  const svg = e.currentTarget as SVGSVGElement;
  const px = e.clientX - svg.getBoundingClientRect().left;
  const [a, b] = props.domain;
  const w = width.value - PAD.left - PAD.right;
  const t = a + ((px - PAD.left) / w) * (b - a);
  emit("hover", Math.min(b, Math.max(a, t)));
}

const clipId = `tc-${Math.random().toString(36).slice(2)}`;
const time = formatClock;
</script>

<template>
  <figure class="tc" :data-chart="chart.key">
    <figcaption class="tc-head">
      <span class="tc-title">{{ chart.title }}</span>
      <span v-if="headline !== null" class="tc-value">{{ headline }}</span>
    </figcaption>
    <ul v-if="chart.lines.length > 1" class="tc-legend">
      <li v-for="l in chart.lines" :key="l.label">
        <span class="tc-swatch" :style="{ background: colorVar(l) }" />
        <span class="tc-label">{{ l.label }}</span>
        <span class="tc-legend-value">{{ format(valueOf(l)) }}</span>
      </li>
    </ul>
    <div ref="box" class="tc-plot">
      <svg
        :width="width"
        :height="HEIGHT"
        :viewBox="`0 0 ${width} ${HEIGHT}`"
        role="img"
        :aria-label="`${chart.title} over the run`"
        @pointermove="onMove"
        @pointerleave="emit('hover', null)"
      >
        <defs>
          <clipPath :id="clipId">
            <rect :x="PAD.left" y="0" :width="width - PAD.left - PAD.right" :height="HEIGHT" />
          </clipPath>
        </defs>
        <line
          class="tc-grid"
          :x1="PAD.left"
          :x2="width - PAD.right"
          :y1="HEIGHT - PAD.bottom"
          :y2="HEIGHT - PAD.bottom"
        />
        <text class="tc-axis" :x="PAD.left" :y="PAD.top - 4">{{ format(range[1]) }}</text>
        <g :clip-path="`url(#${clipId})`">
          <path
            v-for="p in paths"
            :key="p.line.label"
            class="tc-line"
            :d="p.d"
            :style="{ stroke: colorVar(p.line) }"
          />
          <!-- Where each line stands now: a series with one sample is
               otherwise a line of no length. -->
          <circle
            v-for="p in ends"
            :key="`end-${p.line.label}`"
            class="tc-end"
            r="3"
            :cx="p.x"
            :cy="p.y"
            :style="{ fill: colorVar(p.line) }"
          />
        </g>
        <line
          v-if="crosshairX !== null"
          class="tc-cross"
          :x1="crosshairX"
          :x2="crosshairX"
          :y1="PAD.top"
          :y2="HEIGHT - PAD.bottom"
        />
        <text class="tc-axis" :x="PAD.left" :y="HEIGHT - 3">{{ time(domain[0]) }}</text>
        <text class="tc-axis" text-anchor="end" :x="width - PAD.right" :y="HEIGHT - 3">
          {{ hover !== null ? time(readAt) : time(domain[1]) }}
        </text>
      </svg>
    </div>
  </figure>
</template>

<style>
.tc {
  margin: 0;
  padding: 8px 10px 4px;
  border: 1px solid var(--datalib-border);
  border-radius: var(--datalib-radius);
  background: var(--datalib-bg);
  min-width: 0;
}
.tc-head {
  display: flex;
  justify-content: space-between;
  gap: 8px;
  font-size: var(--datalib-font-size-small);
}
.tc-title {
  color: var(--datalib-muted);
}
.tc-value {
  color: var(--datalib-fg);
  font-variant-numeric: tabular-nums;
  font-weight: 600;
}
.tc-legend {
  display: flex;
  flex-wrap: wrap;
  gap: 2px 10px;
  margin: 4px 0 0;
  padding: 0;
  list-style: none;
  font-size: var(--datalib-font-size-small);
  color: var(--datalib-fg);
}
.tc-legend li {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.tc-swatch {
  width: 10px;
  height: 3px;
  border-radius: 2px;
}
.tc-label {
  color: var(--datalib-muted);
}
.tc-legend-value {
  font-variant-numeric: tabular-nums;
}
.tc-plot {
  width: 100%;
  margin-top: 2px;
}
.tc-plot svg {
  display: block;
  touch-action: none;
}
.tc-line {
  fill: none;
  stroke-width: 2;
  stroke-linejoin: round;
  stroke-linecap: round;
}
.tc-end {
  stroke: var(--datalib-bg);
  stroke-width: 1.5;
}
.tc-grid {
  stroke: var(--datalib-border);
  stroke-width: 1;
}
.tc-cross {
  stroke: var(--datalib-muted);
  stroke-width: 1;
  stroke-dasharray: 2 2;
}
.tc-axis {
  fill: var(--datalib-muted);
  font-size: 10px;
  font-variant-numeric: tabular-nums;
}
</style>
