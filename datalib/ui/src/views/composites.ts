// Composites: container subtrees kept under a name, to be opened again
// as a whole. The built-in ones ship with the app; the person's own are
// kept in the library (`/api/ui/state/composites`).
import { ref } from "vue";
import { fetchUiState, putUiState } from "@/api";
import { DASHBOARD_PARTS } from "@/cards/dashboard";
import { makeBox, makeCard, parseComposites, type BoxNode } from "@/views/containerTree";

const STATE_NAME = "composites";

// Fixed heights for the two short parts; the other two share the rest.
const PART_BASIS: Record<string, number | null> = {
  sync: 44,
  library: 112,
  sources: null,
  activity: null,
};

// The Dashboard as four stacked cards, solidified all the way down: a
// card opened from it gets a tab of its own.
const DASHBOARD: BoxNode = makeBox(
  "dashboard",
  "stack",
  DASHBOARD_PARTS.map((part) => ({
    ...makeCard(`dashboard-${part}`, `dashboardView(${JSON.stringify({ part })})`),
    basis: PART_BASIS[part],
  })),
  { solidifyAll: true, name: "Dashboard", template: "Dashboard" },
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
  return BUILTIN_COMPOSITES[name] ?? savedComposites.value[name];
}

// Keep `box` as the composite `name`, replacing one of that name.
// Built-in names are taken.
export async function saveComposite(name: string, box: BoxNode): Promise<boolean> {
  if (name in BUILTIN_COMPOSITES) return false;
  const next = {
    ...savedComposites.value,
    [name]: { ...box, name, template: name, basis: null, openedBy: null },
  };
  await putUiState(STATE_NAME, next);
  savedComposites.value = next;
  return true;
}

export async function deleteComposite(name: string) {
  const next = { ...savedComposites.value };
  delete next[name];
  await putUiState(STATE_NAME, next);
  savedComposites.value = next;
}
