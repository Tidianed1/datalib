// The size scale as numbers: a step from MIN_STEP (the most on screen)
// to MAX_STEP in STEP increments. Step 0 is the old Compact and step 1
// the old Comfortable; theme.css draws every size from the step. Pure,
// so density.ts and its tests share one rule.
export const MIN_STEP = 0;
export const MAX_STEP = 2;
export const STEP = 0.25;

// A stored or typed value, on the scale: rounded to a step, kept in
// range; MIN_STEP for anything that is not a number.
export function onScale(value: unknown): number {
  const n = typeof value === "string" ? Number.parseFloat(value) : Number(value);
  if (!Number.isFinite(n)) return MIN_STEP;
  return Math.min(MAX_STEP, Math.max(MIN_STEP, Math.round(n / STEP) * STEP));
}

// How many steps the scale has, and which one `step` is (0-based):
// what the status bar's ticks show.
export const STEPS = Math.round((MAX_STEP - MIN_STEP) / STEP) + 1;

export function stepIndex(step: number): number {
  return Math.round((onScale(step) - MIN_STEP) / STEP);
}
