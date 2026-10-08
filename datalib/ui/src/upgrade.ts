// What the launch's upgrade screens say about a step
// (docs/dev/plans/upgrade_on_launch.md).

import type { Upgrade } from "@/api";

/// The source a step belongs to, as a person would name it: its group,
/// or the search index for the index's own steps.
export function stepSource(step: string): string {
  const group = step.split("/")[0];
  return group === "unified_index" ? "Search index" : group;
}

/// The sources a re-render covers, each once, in the order given.
export function rerenderSources(upgrade: Upgrade): string[] {
  return [...new Set(upgrade.rerender.map(stepSource))];
}

/// The raw stores the launch could not migrate, with why.
export function failedStores(upgrade: Upgrade): { source: string; error: string }[] {
  return upgrade.stores
    .filter((s) => s.state === "failed")
    .map((s) => ({ source: stepSource(s.step), error: s.error ?? "" }));
}
