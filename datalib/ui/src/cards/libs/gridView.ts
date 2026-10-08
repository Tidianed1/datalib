// `searchView()` in card source returns a CardRender for the Search
// card (cards/GridCard.ce.vue): one query, shown as a list with a
// preview or as a table. It opens on the view picked last unless `view`
// names one. A search given a `name` keeps it; one without names itself
// after the live query; no `q` opens on `DEFAULT_QUERY`.
//
// `gridView` is the same factory under its older name, which saved
// layouts and links still use, and the one that takes `url`: another
// table that pages the way the search does (the problems), drawn as the
// table alone.
import GridCard from "../GridCard.ce.vue";
import SearchList from "../SearchList.ce.vue";
import { DEFAULT_QUERY } from "../searchDefaults";
import type { SearchViewId } from "../searchViewPref";
import tableGridCss from "../tableGrid.css?inline";
import chipCss from "../chip.css?inline";
// The grid's theme has to be in the same root as the grid; head
// styles stop at the shadow boundary.
import slickCss from "@slickgrid-universal/common/dist/styles/css/slickgrid-theme-default.css?inline";
import { vueCard } from "../vueCard";
import type { CardRender } from "../types";

export type SearchOpts = {
  q?: string;
  columns?: string[];
  name?: string;
  view?: SearchViewId;
};

export function gridView(opts?: SearchOpts & { url?: string; placeholder?: string }): CardRender {
  return vueCard(
    GridCard,
    {
      q: opts?.q || (opts?.url ? "" : DEFAULT_QUERY),
      columns: opts?.columns,
      name: opts?.name,
      url: opts?.url,
      placeholder: opts?.placeholder,
      view: opts?.view,
    },
    { styleSources: [SearchList, slickCss, tableGridCss, chipCss] },
  );
}

export function searchView(opts?: SearchOpts): CardRender {
  return gridView(opts);
}
