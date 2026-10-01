// `dashboardView()` in card source: the Dashboard card — what needs you, the
// library's size, each source's state and the newest documents (see
// cards/DashboardCard.ce.vue). The card a new window opens on.
import DashboardCard from "../DashboardCard.ce.vue";
import { vueCard } from "../vueCard";
import type { CardRender } from "../types";

export function dashboardView(): CardRender {
  return vueCard(DashboardCard);
}
