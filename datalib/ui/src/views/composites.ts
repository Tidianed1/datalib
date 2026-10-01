// Composites: container subtrees kept under a name, to be opened again
// as a whole. The built-in ones ship with the app; the person's own are
// kept in the library (`/api/ui/state/composites`).
import { ref } from "vue";
import { fetchUiState, putUiState } from "@/api";
import { DASHBOARD_PARTS, type DashboardPart } from "@/cards/dashboard";
import { makeBox, makeCard, parseComposites, type BoxNode } from "@/views/containerTree";

const STATE_NAME = "composites";

// Fixed heights for the two short parts; the other two share the rest.
const PART_BASIS: Record<DashboardPart, number | null> = {
  sync: 44,
  library: 112,
  sources: null,
  activity: null,
};

// The Dashboard as four stacked cards, solidified: a card opened from
// it gets a tab of its own.
const DASHBOARD: BoxNode = makeBox(
  "dashboard",
  "stack",
  DASHBOARD_PARTS.map((part) => ({
    ...makeCard(`dashboard-${part}`, `dashboardView(${JSON.stringify({ part })})`),
    basis: PART_BASIS[part],
  })),
  { solidified: true, name: "Dashboard", template: "Dashboard" },
);

export const BUILTIN_COMPOSITES: Record<string, BoxNode> = { Dashboard: DASHBOARD };

export const savedComposites = ref<Record<string, BoxNode>>({});

export async function loadComposites() {
  try {
    savedComposites.value = parseComposites(await fetchUiState(STATE_NAME));
  } catch (e) {
    console.warn("could not load the saved composites", e);
  }
}

export function composite(name: string): BoxNode | undefined {
  return isBuiltinComposite(name) ? BUILTIN_COMPOSITES[name] : savedComposites.value[name];
}

export function isBuiltinComposite(name: string): boolean {
  return Object.hasOwn(BUILTIN_COMPOSITES, name);
}

// Keep `box` as the composite `name`, replacing a saved one of that
// name. A built-in name is the caller's to refuse (isBuiltinComposite).
export async function saveComposite(name: string, box: BoxNode): Promise<void> {
  const next = {
    ...savedComposites.value,
    [name]: { ...box, name, template: name, basis: null, openedBy: null },
  };
  await putUiState(STATE_NAME, next);
  savedComposites.value = next;
}
