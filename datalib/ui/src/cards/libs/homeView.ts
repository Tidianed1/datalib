// `homeView()` in card source: the Home card — what needs you, the
// library's size, each source's state and the newest documents (see
// cards/HomeCard.ce.vue). The card a new window opens on.
import HomeCard from "../HomeCard.ce.vue";
import { vueCard } from "../vueCard";
import type { CardRender } from "../types";

export function homeView(): CardRender {
  return vueCard(HomeCard);
}
