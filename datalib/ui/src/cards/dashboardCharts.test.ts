import { describe, expect, it } from "vitest";
import type { DashboardStep } from "@/api";
import {
  MAX_LINES,
  labelText,
  stepCharts,
  stepPath,
  sumSeries,
  valueAt,
  yRange,
} from "./dashboardCharts";

const at = (s: number) => new Date(Date.UTC(2026, 8, 30, 12, 0, s)).toISOString();

function step(over: Partial<DashboardStep> = {}): DashboardStep {
  return {
    id: "slack/ingest",
    in_run: true,
    state: "running",
    attempt: 1,
    started_at_utc: at(0),
    finished_at_utc: null,
    error: null,
    msg: null,
    series: [],
    warnings: [],
    errors: [],
    disk: [],
    ...over,
  };
}

describe("reading a step function", () => {
  const pts = [
    { t: 10, v: 1 },
    { t: 20, v: 5 },
  ];
  it("carries the last value forward and has none before the first", () => {
    expect(valueAt(pts, 5)).toBeNull();
    expect(valueAt(pts, 10)).toBe(1);
    expect(valueAt(pts, 19)).toBe(1);
    expect(valueAt(pts, 99)).toBe(5);
  });

  it("sums series that move at different times, counting one not yet begun as nothing", () => {
    expect(sumSeries([pts, [{ t: 15, v: 100 }]])).toEqual([
      { t: 10, v: 1 },
      { t: 15, v: 101 },
      { t: 20, v: 105 },
    ]);
  });

  it("draws flat between points and carries the last to the end", () => {
    const path = stepPath(
      pts,
      30,
      (t) => t,
      (v) => v,
    );
    expect(path).toBe("M10.0,1.0L20.0,1.0L20.0,5.0L30.0,5.0");
  });
});

describe("a step's charts", () => {
  it("leads with the queue summed over producers, then one chart per series name", () => {
    const charts = stepCharts(
      step({
        series: [
          { name: "api_requests", labels: "", points: [{ at: at(1), value: 3 }] },
          { name: "queued", labels: "", points: [{ at: at(1), value: 4 }] },
          { name: "queued", labels: "from=a/ingest", points: [{ at: at(2), value: 6 }] },
          { name: "problems", labels: "severity=error", points: [{ at: at(2), value: 1 }] },
        ],
        warnings: [{ at: at(3), value: 1 }],
        disk: [{ at: at(0), value: 1000 }],
      }),
    );
    expect(charts.map((c) => c.key)).toEqual(["queued", "api_requests", "log_problems", "disk"]);
    expect(charts[0].lines[0].points.at(-1)?.v).toBe(10);
  });

  it("folds labels past the cap into one summed line, keeping each colour on its series", () => {
    const series = Array.from({ length: MAX_LINES + 2 }, (_, i) => ({
      name: "rows_upserted",
      labels: `table=t${i}`,
      points: [{ at: at(i), value: 1 }],
    }));
    const [chart] = stepCharts(step({ series }));
    expect(chart.title).toBe("rows upserted");
    expect(chart.lines).toHaveLength(MAX_LINES);
    expect(chart.lines[0]).toMatchObject({ label: "t0", color: { slot: 0 } });
    expect(chart.lines.at(-1)).toMatchObject({ label: "3 more" });
    expect(chart.lines.at(-1)?.points.at(-1)?.v).toBe(3);
  });

  it("names a line by its label values", () => {
    expect(labelText("")).toBe("total");
    expect(labelText("table=messages,kind=dm")).toBe("messages, dm");
  });
});

describe("the y-axis", () => {
  const chart = (fromZero: boolean, vs: number[]) => ({
    key: "k",
    title: "k",
    unit: "count" as const,
    fromZero,
    lines: [{ label: "l", color: { accent: true as const }, points: vs.map((v, t) => ({ t, v })) }],
  });
  it("starts a count at zero and a size at its own range", () => {
    expect(yRange(chart(true, [5, 10]), [0, 10])).toEqual([0, 10]);
    const [lo, hi] = yRange(chart(false, [1000, 1010]), [0, 10]);
    expect(lo).toBeGreaterThan(900);
    expect(hi).toBeLessThan(1100);
  });
});
