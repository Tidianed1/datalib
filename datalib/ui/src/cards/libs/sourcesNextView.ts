// `sourcesNextView()` in card source: the redesigned Sources card, beside
// `sourcesView()` until it replaces it (see cards/SourcesNextCard.ce.vue).
// Same logic, same table engine, every action the old card has. The
// panels it opens are teleported out of the shadow root, so the old
// card's stylesheet — which styles them — is imported into the head too.
import SourcesNextCard from "../SourcesNextCard.ce.vue";
import sourcesCardCss from "../sourcesCard.css?inline";
import sourcesNextCardCss from "../sourcesNextCard.css?inline";
import tableGridCss from "../tableGrid.css?inline";
import slickCss from "@slickgrid-universal/common/dist/styles/css/slickgrid-theme-default.css?inline";
import "../sourcesCard.css";
import { vueCard } from "../vueCard";
import type { CardRender } from "../types";

export function sourcesNextView(): CardRender {
  return vueCard(
    SourcesNextCard,
    {},
    { styleSources: [slickCss, tableGridCss, sourcesCardCss, sourcesNextCardCss] },
  );
}
