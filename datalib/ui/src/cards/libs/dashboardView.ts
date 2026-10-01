// `dashboardView()` in card source: the Dashboard card — what needs you, the
// library's size, each source's state and the newest documents (see
// cards/DashboardCard.ce.vue). `dashboardView({ part: "sources" })` shows
// one of those sections alone, which is how the containers layout builds
// the Dashboard as a stack of four cards.
import DashboardCard from "../DashboardCard.ce.vue";
import { DASHBOARD_PARTS, type DashboardPart } from "../dashboard";
import { vueCard } from "../vueCard";
import type { CardRender } from "../types";

export function dashboardView(opts?: { part?: DashboardPart }): CardRender {
  const part = DASHBOARD_PARTS.find((p) => p === opts?.part);
  return vueCard(DashboardCard, part ? { part } : {});
}
