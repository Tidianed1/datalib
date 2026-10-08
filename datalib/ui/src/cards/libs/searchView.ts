// `searchView({ q })` in card source: the Search card opened on its
// list, each result with a preview beside it (see cards/GridCard.ce.vue
// and cards/SearchList.ce.vue). `gridView` is the same card opened on
// its table. No `q` opens on `DEFAULT_QUERY`.
import { gridView } from "./gridView";
import type { CardRender } from "../types";

export function searchView(opts?: { q?: string }): CardRender {
  return gridView({ q: opts?.q || undefined, view: "list" });
}
