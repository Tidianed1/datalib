// The containers layout as a value: a tree of containers whose leaves
// are cards. Every container lays out its own children (tabs, a stack,
// a row, miller columns), and any container but the outermost can be
// solidified. A card opened from a card lands in the nearest container
// above the opener that is not solidified, so a solidified subtree keeps its
// shape and an unsolidified one grows. The decisions are pure functions
// here; ContainersView applies them.

export const LAYOUTS = ["tabs", "stack", "row", "columns"] as const;
export type Layout = (typeof LAYOUTS)[number];

export const LAYOUT_LABELS: Record<Layout, string> = {
  tabs: "Tabs",
  stack: "Stack",
  row: "Row",
  columns: "Columns",
};

type Common = {
  id: string;
  // The node's size along its container's axis, in px: its height in a
  // stack, its width in a row or columns. null shares what is left
  // (a column without one is DEFAULT_COLUMN px wide).
  basis: number | null;
  // The sibling this one was opened from, so a tabs container can list
  // its children as a tree, and closing a tab closes what it opened.
  openedBy: string | null;
};

export type CardNode = Common & {
  kind: "card";
  source: string;
  // Opaque per-card state string (see HostCommands.setState).
  state: string;
  // What the card last called itself, kept so an unmounted tab has a name.
  title: string | null;
};

export type BoxNode = Common & {
  kind: "box";
  layout: Layout;
  // Opens from inside skip this container.
  solidified: boolean;
  // This container and everything inside it count as solidified, whatever
  // their own flags say; turning it off brings those flags back.
  solidifyAll: boolean;
  children: TreeNode[];
  // The child a tabs container shows.
  selected: string | null;
  // A name the person gave it, or the composite's name.
  name: string | null;
  // The composite this was made from, so it can be reset to it.
  template: string | null;
};

export type TreeNode = CardNode | BoxNode;

export const DEFAULT_COLUMN = 480;

export function makeCard(id: string, source: string, state = ""): CardNode {
  return { kind: "card", id, source, state, title: null, basis: null, openedBy: null };
}

export function makeBox(
  id: string,
  layout: Layout,
  children: TreeNode[],
  opts: Partial<Pick<BoxNode, "solidified" | "solidifyAll" | "name" | "template" | "basis">> = {},
): BoxNode {
  return {
    kind: "box",
    id,
    layout,
    children,
    solidified: opts.solidified ?? false,
    solidifyAll: opts.solidifyAll ?? false,
    selected: layout === "tabs" ? (children[0]?.id ?? null) : null,
    name: opts.name ?? null,
    template: opts.template ?? null,
    basis: opts.basis ?? null,
    openedBy: null,
  };
}

// ---- reading ----

// The nodes from the root down to `id`, both ends included; empty when
// `id` is not in the tree.
export function pathTo(root: TreeNode, id: string): TreeNode[] {
  if (root.id === id) return [root];
  if (root.kind === "box") {
    for (const child of root.children) {
      const rest = pathTo(child, id);
      if (rest.length) return [root, ...rest];
    }
  }
  return [];
}

export function find(root: TreeNode, id: string): TreeNode | undefined {
  const path = pathTo(root, id);
  return path[path.length - 1];
}

export function parentOf(root: TreeNode, id: string): BoxNode | undefined {
  const path = pathTo(root, id);
  return path.length >= 2 ? (path[path.length - 2] as BoxNode) : undefined;
}

export function cards(root: TreeNode): CardNode[] {
  return root.kind === "card" ? [root] : root.children.flatMap(cards);
}

// Whether the box at path[k] counts as solidified. The outermost container
// never does, so an open always has somewhere to land.
function solidifiedAt(path: TreeNode[], k: number): boolean {
  if (k === 0) return false;
  const box = path[k] as BoxNode;
  return box.solidified || path.slice(0, k + 1).some((n) => n.kind === "box" && n.solidifyAll);
}

export function isSolidified(root: TreeNode, id: string): boolean {
  const path = pathTo(root, id);
  const k = path.length - 1;
  return k >= 0 && path[k].kind === "box" && solidifiedAt(path, k);
}

// Whether `id` or a container above it has "solidify all" set: such a
// node looks finished, with no card or container chrome, outside dev mode.
export function underSolidifyAll(root: TreeNode, id: string): boolean {
  return pathTo(root, id).some((n) => n.kind === "box" && n.solidifyAll);
}

// Where a card opened from `fromId` goes: the nearest container above
// it that is not solidified, and which of that container's children holds
// the opener.
export function landing(
  root: TreeNode,
  fromId: string,
): { boxId: string; branchId: string } | null {
  const path = pathTo(root, fromId);
  for (let k = path.length - 2; k >= 0; k--) {
    if (!solidifiedAt(path, k)) return { boxId: path[k].id, branchId: path[k + 1].id };
  }
  return null;
}

// What a tabs container lists, top to bottom: each child under the one
// it was opened from, siblings in the order they came.
export function tabRows(box: BoxNode): { node: TreeNode; depth: number }[] {
  const ids = new Set(box.children.map((c) => c.id));
  const out: { node: TreeNode; depth: number }[] = [];
  const walk = (parent: string | null, depth: number) => {
    for (const c of box.children) {
      const top = c.openedBy === null || !ids.has(c.openedBy);
      if (parent === null ? !top : c.openedBy !== parent) continue;
      out.push({ node: c, depth });
      walk(c.id, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

// ---- changing ----

// `root` with the node `id` replaced by what `fn` returns for it.
function mapNode(root: TreeNode, id: string, fn: (n: TreeNode) => TreeNode): TreeNode {
  if (root.id === id) return fn(root);
  if (root.kind === "card") return root;
  let changed = false;
  const children = root.children.map((c) => {
    const next = mapNode(c, id, fn);
    if (next !== c) changed = true;
    return next;
  });
  return changed ? { ...root, children } : root;
}

function mapBox(root: TreeNode, id: string, fn: (b: BoxNode) => BoxNode): TreeNode {
  return mapNode(root, id, (n) => (n.kind === "box" ? fn(n) : n));
}

// Show `id`: every tabs container on the way down selects the child
// that holds it.
export function reveal(root: TreeNode, id: string): TreeNode {
  const path = pathTo(root, id);
  let next = root;
  for (let k = 0; k < path.length - 1; k++) {
    const box = path[k] as BoxNode;
    if (box.layout === "tabs" && box.selected !== path[k + 1].id) {
      const childId = path[k + 1].id;
      next = mapBox(next, box.id, (b) => ({ ...b, selected: childId }));
    }
  }
  return next;
}

// Open `nodes` from `fromId` as a chain: the first opened by the
// opener's branch, each next one by the one before. Where they go
// within the landing container is the container's layout's call:
// columns drop what was right of the opener, the others insert beside
// it. Returns the tree unchanged when nothing is unsolidified above.
export function openFrom(root: TreeNode, fromId: string, nodes: TreeNode[]): TreeNode {
  const land = landing(root, fromId);
  if (!land || nodes.length === 0) return root;
  const chained = nodes.map((n, i) => ({
    ...n,
    openedBy: i === 0 ? land.branchId : nodes[i - 1].id,
  }));
  const next = mapBox(root, land.boxId, (box) => {
    const i = box.children.findIndex((c) => c.id === land.branchId);
    let children: TreeNode[];
    if (box.layout === "columns") children = [...box.children.slice(0, i + 1), ...chained];
    else if (box.layout === "tabs") children = [...box.children, ...chained];
    else children = [...box.children.slice(0, i + 1), ...chained, ...box.children.slice(i + 1)];
    return { ...box, children };
  });
  return reveal(next, nodes[nodes.length - 1].id);
}

// Append a child to a container, and show it.
export function addChild(root: TreeNode, boxId: string, node: TreeNode): TreeNode {
  const next = mapBox(root, boxId, (b) => ({ ...b, children: [...b.children, node] }));
  return reveal(next, node.id);
}

// What a tabs container closes along with a child: everything opened
// from it, all the way down.
function openedFrom(box: BoxNode, id: string): Set<string> {
  const out = new Set([id]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const c of box.children) {
      if (c.openedBy !== null && out.has(c.openedBy) && !out.has(c.id)) {
        out.add(c.id);
        grew = true;
      }
    }
  }
  return out;
}

// Close `id`. In a tabs container that takes what it opened with it;
// elsewhere what it opened is re-pointed at its own opener. A container
// left empty goes too; the outermost one is left empty for the caller.
export function remove(root: TreeNode, id: string): TreeNode {
  const parent = parentOf(root, id);
  if (!parent) return root;
  const gone = parent.layout === "tabs" ? openedFrom(parent, id) : new Set([id]);
  const victim = parent.children.find((c) => c.id === id)!;
  const at = parent.children.findIndex((c) => c.id === id);
  const children = parent.children
    .filter((c) => !gone.has(c.id))
    .map((c) => (c.openedBy === id ? { ...c, openedBy: victim.openedBy } : c));
  if (children.length === 0 && parent.id !== root.id) return remove(root, parent.id);
  let selected = parent.selected;
  if (selected !== null && gone.has(selected)) {
    const before = parent.children
      .slice(0, at)
      .reverse()
      .find((c) => !gone.has(c.id));
    selected = (before ?? children[0])?.id ?? null;
  }
  return mapBox(root, parent.id, (b) => ({ ...b, children, selected }));
}

export function select(root: TreeNode, id: string): TreeNode {
  return reveal(root, id);
}

export function setCard(
  root: TreeNode,
  id: string,
  patch: Partial<Pick<CardNode, "source" | "state" | "title">>,
): TreeNode {
  return mapNode(root, id, (n) => (n.kind === "card" ? { ...n, ...patch } : n));
}

export function setBasis(root: TreeNode, id: string, basis: number | null): TreeNode {
  return mapNode(root, id, (n) => ({ ...n, basis }));
}

export function setLayout(root: TreeNode, id: string, layout: Layout): TreeNode {
  return mapBox(root, id, (b) => ({
    ...b,
    layout,
    selected: layout === "tabs" ? (b.selected ?? b.children[0]?.id ?? null) : b.selected,
    // Sizes along one axis mean nothing along another.
    children: b.layout === layout ? b.children : b.children.map((c) => ({ ...c, basis: null })),
  }));
}

// Set `solidified` or `solidifyAll`. The outermost container is never solidified.
export function setFlag(
  root: TreeNode,
  id: string,
  flag: "solidified" | "solidifyAll",
  value: boolean,
): TreeNode {
  if (id === root.id) return root;
  return mapBox(root, id, (b) => ({ ...b, [flag]: value }));
}

export function rename(root: TreeNode, id: string, name: string | null): TreeNode {
  return mapNode(root, id, (n) =>
    n.kind === "box" ? { ...n, name } : { ...n, title: name ?? n.title },
  );
}

// Mark box `id` as made from the composite `template`, so it can be
// reset to it; a box with no name of its own takes the composite's.
export function setTemplate(root: TreeNode, id: string, template: string): TreeNode {
  return mapBox(root, id, (b) => ({ ...b, template, name: b.name ?? template }));
}

// Put `id` inside a new container of its own, which takes its place.
export function wrap(root: TreeNode, id: string, layout: Layout, boxId: string): TreeNode {
  return mapNode(root, id, (n) => {
    const box = makeBox(boxId, layout, [{ ...n, basis: null, openedBy: null }], {
      basis: n.basis,
    });
    return { ...box, openedBy: n.openedBy };
  });
}

// Replace container `id` with its children, in place.
export function unwrap(root: TreeNode, id: string): TreeNode {
  const parent = parentOf(root, id);
  const box = find(root, id);
  if (!parent || !box || box.kind !== "box") return root;
  const lifted = box.children.map((c, i) => ({
    ...c,
    basis: null,
    openedBy: i === 0 ? box.openedBy : (c.openedBy ?? box.openedBy),
  }));
  const at = parent.children.findIndex((c) => c.id === id);
  const children = [...parent.children.slice(0, at), ...lifted, ...parent.children.slice(at + 1)];
  const selected = parent.selected === id ? (lifted[0]?.id ?? null) : parent.selected;
  return mapBox(root, parent.id, (b) => ({ ...b, children, selected }));
}

// Move `id` one place earlier (-1) or later (+1) among its siblings.
export function move(root: TreeNode, id: string, delta: -1 | 1): TreeNode {
  const parent = parentOf(root, id);
  if (!parent) return root;
  const i = parent.children.findIndex((c) => c.id === id);
  const j = i + delta;
  if (j < 0 || j >= parent.children.length) return root;
  const children = [...parent.children];
  [children[i], children[j]] = [children[j], children[i]];
  return mapBox(root, parent.id, (b) => ({ ...b, children }));
}

// ---- composites ----

// A copy of `node` with fresh ids throughout, its openers and selection
// re-pointed to match: how a saved composite becomes a live one.
export function instantiate(node: TreeNode, freshId: () => string): TreeNode {
  const ids = new Map<string, string>();
  const assign = (n: TreeNode) => {
    ids.set(n.id, freshId());
    if (n.kind === "box") n.children.forEach(assign);
  };
  assign(node);
  const copy = (n: TreeNode): TreeNode => {
    const base = {
      id: ids.get(n.id)!,
      openedBy: n.openedBy !== null ? (ids.get(n.openedBy) ?? null) : null,
    };
    if (n.kind === "card") return { ...n, ...base };
    return {
      ...n,
      ...base,
      selected: n.selected !== null ? (ids.get(n.selected) ?? null) : null,
      children: n.children.map(copy),
    };
  };
  return { ...copy(node), openedBy: null };
}

// Put a fresh copy of `template` where `id` is, keeping its size and
// its place among its siblings.
export function resetTo(
  root: TreeNode,
  id: string,
  template: TreeNode,
  freshId: () => string,
): TreeNode {
  const fresh = instantiate(template, freshId);
  return mapNode(root, id, (n) => ({ ...fresh, basis: n.basis, openedBy: n.openedBy }));
}

// ---- storing ----

function isNode(v: unknown): v is TreeNode {
  if (typeof v !== "object" || v === null) return false;
  const n = v as Record<string, unknown>;
  if (typeof n.id !== "string") return false;
  if (n.kind === "card") return typeof n.source === "string" && typeof n.state === "string";
  return (
    n.kind === "box" &&
    LAYOUTS.includes(n.layout as Layout) &&
    Array.isArray(n.children) &&
    n.children.every(isNode)
  );
}

// A stored tree, or null for anything this build cannot read.
export function parseTree(v: unknown): BoxNode | null {
  return isNode(v) && v.kind === "box" ? v : null;
}

export function parseComposites(v: unknown): Record<string, BoxNode> {
  const out: Record<string, BoxNode> = {};
  if (typeof v !== "object" || v === null) return out;
  for (const [name, node] of Object.entries(v)) {
    if (isNode(node) && node.kind === "box") out[name] = node;
  }
  return out;
}
