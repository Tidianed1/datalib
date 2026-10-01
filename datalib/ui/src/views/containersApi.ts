// What ContainersView (the host) provides down to the recursive
// ContainerNode, so ContainerNode needs only its node. Its own module to
// keep the host and the recursive component from importing each other.
import type { InjectionKey } from "vue";
import type { CardCtx } from "@/cards/types";
import type { BoxNode, CardNode, Layout, TreeNode } from "./containerTree";

export type MenuItem = { label: string; run: () => void; checked?: boolean } | "separator";

export type ContainersApi = {
  ctxFor(card: CardNode): CardCtx;
  titleOf(node: TreeNode): string;
  // Register (or, with null, drop) the element a card's DOM is moved
  // into; the cards live in one pool in the host and are teleported, so
  // rearranging containers moves a card without remounting it.
  setSlot(id: string, el: Element | null): void;
  // Whether a card or container shows its chrome: always in dev mode,
  // and otherwise only outside a "solidify all" subtree.
  chromeShown(id: string): boolean;
  isSolidified(id: string): boolean;
  select(id: string): void;
  close(id: string): void;
  commitSource(card: CardNode, e: Event): void;
  setLayout(id: string, layout: Layout): void;
  toggleFlag(box: BoxNode, flag: "solidified" | "solidifyAll"): void;
  addCard(boxId: string): void;
  openMenu(ev: MouseEvent, items: MenuItem[]): void;
  boxMenu(box: BoxNode): MenuItem[];
  cardMenu(card: CardNode): MenuItem[];
  // Drag the edge after child `id` to resize it along `axis`.
  startResize(id: string, axis: "x" | "y", ev: PointerEvent): void;
};

export const CONTAINERS_API: InjectionKey<ContainersApi> = Symbol("containersApi");
