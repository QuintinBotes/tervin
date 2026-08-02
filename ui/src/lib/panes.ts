/**
 * The pane tree.
 *
 * "Arbitrary horizontal and vertical splits" means a tree, not a list. A list can
 * express `a | b | c`; it cannot express `a | (b / c)`, which is what you get the
 * moment someone splits one half of an existing split — and that is the second
 * thing anyone does in a terminal.
 *
 * The tree is a plain value with pure operations, so layouts serialise for
 * session restore, and every operation is testable without a renderer.
 *
 * Two invariants hold after every operation, enforced by `normalise`:
 *
 *  1. A split always has at least two children. Closing a pane collapses its
 *     parent rather than leaving a split wrapping one child, which would
 *     otherwise accumulate as invisible nesting that breaks resizing.
 *  2. A split's children are never a split of the same direction. Nested
 *     same-direction splits are flattened, so `a | (b | c)` becomes `a | b | c`
 *     and the divider between b and c behaves like the one between a and b.
 */

export type SplitDirection = "row" | "column";

export type PaneNode =
  | { type: "leaf"; id: string; paneId: string }
  | {
      type: "split";
      id: string;
      direction: SplitDirection;
      children: PaneNode[];
      /** Fractions summing to 1, one per child. */
      sizes: number[];
    };

/** Smallest fraction a pane may be dragged to, so it never disappears. */
const MIN_FRACTION = 0.08;

let counter = 0;
const nextId = (prefix: string) => `${prefix}-${++counter}`;

/** A tree containing a single pane. */
export function leaf(paneId: string): PaneNode {
  return { type: "leaf", id: nextId("node"), paneId };
}

/** Every pane id, left-to-right, depth-first — the order focus cycles through. */
export function listPanes(node: PaneNode): string[] {
  if (node.type === "leaf") return [node.paneId];
  return node.children.flatMap(listPanes);
}

export function paneCount(node: PaneNode): number {
  return listPanes(node).length;
}

export function containsPane(node: PaneNode, paneId: string): boolean {
  return listPanes(node).includes(paneId);
}

/**
 * Split the pane containing `targetPaneId`, inserting `newPaneId` after it.
 *
 * When the target's parent already runs in the requested direction the new pane
 * joins that split as a sibling, rather than creating a nested split — which is
 * what makes three side-by-side panes share one row of dividers.
 */
export function splitPane(
  root: PaneNode,
  targetPaneId: string,
  direction: SplitDirection,
  newPaneId: string,
): PaneNode {
  const replaced = replaceLeaf(root, targetPaneId, (target) => ({
    type: "split",
    id: nextId("split"),
    direction,
    children: [target, leaf(newPaneId)],
    sizes: [0.5, 0.5],
  }));
  return normalise(replaced);
}

/**
 * Remove a pane.
 *
 * Returns `null` when the tree becomes empty, which the caller treats as "close
 * the tab" rather than rendering nothing.
 */
export function closePane(root: PaneNode, paneId: string): PaneNode | null {
  if (root.type === "leaf") {
    return root.paneId === paneId ? null : root;
  }

  const kept: PaneNode[] = [];
  const keptSizes: number[] = [];

  root.children.forEach((child, index) => {
    const result = closePane(child, paneId);
    if (result) {
      kept.push(result);
      keptSizes.push(root.sizes[index] ?? 1 / root.children.length);
    }
  });

  if (kept.length === 0) return null;

  return normalise({
    ...root,
    children: kept,
    // Redistribute the closed pane's space across the survivors.
    sizes: rebalance(keptSizes),
  });
}

/** Exchange two panes' positions, keeping the layout otherwise identical. */
export function swapPanes(root: PaneNode, a: string, b: string): PaneNode {
  if (a === b) return root;
  const map = (node: PaneNode): PaneNode => {
    if (node.type === "leaf") {
      if (node.paneId === a) return { ...node, paneId: b };
      if (node.paneId === b) return { ...node, paneId: a };
      return node;
    }
    return { ...node, children: node.children.map(map) };
  };
  return map(root);
}

/** The pane after `paneId` in focus order, wrapping at the end. */
export function nextPane(root: PaneNode, paneId: string): string | null {
  const panes = listPanes(root);
  if (panes.length === 0) return null;
  const index = panes.indexOf(paneId);
  if (index === -1) return panes[0]!;
  return panes[(index + 1) % panes.length]!;
}

/** The pane before `paneId` in focus order, wrapping at the start. */
export function prevPane(root: PaneNode, paneId: string): string | null {
  const panes = listPanes(root);
  if (panes.length === 0) return null;
  const index = panes.indexOf(paneId);
  if (index === -1) return panes[panes.length - 1]!;
  return panes[(index - 1 + panes.length) % panes.length]!;
}

/**
 * Move a divider.
 *
 * `delta` is a fraction of the split's own extent. Both neighbours are clamped to
 * `MIN_FRACTION` so a drag can never collapse a pane to nothing — recovering
 * from that requires closing and reopening, which loses the session.
 */
export function resizeSplit(
  root: PaneNode,
  splitId: string,
  dividerIndex: number,
  delta: number,
): PaneNode {
  const map = (node: PaneNode): PaneNode => {
    if (node.type === "leaf") return node;
    if (node.id !== splitId) {
      return { ...node, children: node.children.map(map) };
    }

    const sizes = [...node.sizes];
    const before = sizes[dividerIndex];
    const after = sizes[dividerIndex + 1];
    if (before === undefined || after === undefined) return node;

    const total = before + after;
    const nextBefore = Math.min(Math.max(before + delta, MIN_FRACTION), total - MIN_FRACTION);
    sizes[dividerIndex] = nextBefore;
    sizes[dividerIndex + 1] = total - nextBefore;
    return { ...node, sizes };
  };
  return map(root);
}

/** Reset every split to equal shares. */
export function evenSizes(root: PaneNode): PaneNode {
  if (root.type === "leaf") return root;
  return {
    ...root,
    children: root.children.map(evenSizes),
    sizes: root.children.map(() => 1 / root.children.length),
  };
}

/** The split that owns a pane, for directional focus movement. */
export function parentOf(root: PaneNode, paneId: string): PaneNode | null {
  if (root.type === "leaf") return null;
  if (root.children.some((c) => c.type === "leaf" && c.paneId === paneId)) {
    return root;
  }
  for (const child of root.children) {
    const found = parentOf(child, paneId);
    if (found) return found;
  }
  return null;
}

/** Replace the leaf holding `paneId` with whatever `build` returns. */
function replaceLeaf(
  node: PaneNode,
  paneId: string,
  build: (target: PaneNode) => PaneNode,
): PaneNode {
  if (node.type === "leaf") {
    return node.paneId === paneId ? build(node) : node;
  }
  return {
    ...node,
    children: node.children.map((child) => replaceLeaf(child, paneId, build)),
  };
}

/**
 * Restore the tree's invariants.
 *
 * Called after every structural change rather than trusting each operation to
 * maintain them, because the failure mode — invisible single-child splits that
 * silently break dragging — is hard to notice and harder to trace.
 */
export function normalise(node: PaneNode): PaneNode {
  if (node.type === "leaf") return node;

  const children: PaneNode[] = [];
  const sizes: number[] = [];

  node.children.forEach((rawChild, index) => {
    const child = normalise(rawChild);
    const size = node.sizes[index] ?? 1 / node.children.length;

    // Flatten a nested split of the same direction into this one.
    if (child.type === "split" && child.direction === node.direction) {
      child.children.forEach((grandchild, gIndex) => {
        children.push(grandchild);
        sizes.push(size * (child.sizes[gIndex] ?? 1 / child.children.length));
      });
      return;
    }

    children.push(child);
    sizes.push(size);
  });

  // A split with one child is just that child.
  if (children.length === 1) return children[0]!;

  return { ...node, children, sizes: rebalance(sizes) };
}

/** Scale fractions so they sum to 1. */
function rebalance(sizes: number[]): number[] {
  const total = sizes.reduce((a, b) => a + b, 0);
  if (total <= 0 || sizes.length === 0) {
    return sizes.map(() => 1 / Math.max(sizes.length, 1));
  }
  return sizes.map((s) => s / total);
}

/** Serialise a tree for session restore. */
export function serialise(node: PaneNode): string {
  return JSON.stringify(node);
}

/**
 * Restore a tree, remapping pane ids.
 *
 * Panes cannot be revived — the processes are gone — so restore takes a factory
 * that supplies a fresh pane id per saved leaf. The *shape* survives; the
 * sessions do not, which is the honest boundary of "session restore where safe".
 */
export function deserialise(
  json: string,
  freshPaneId: (savedPaneId: string) => string | null,
): PaneNode | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    return null;
  }

  const rebuild = (node: unknown): PaneNode | null => {
    if (!node || typeof node !== "object") return null;
    const n = node as Record<string, unknown>;

    if (n.type === "leaf" && typeof n.paneId === "string") {
      const id = freshPaneId(n.paneId);
      return id ? leaf(id) : null;
    }

    if (n.type === "split" && Array.isArray(n.children)) {
      const direction: SplitDirection = n.direction === "column" ? "column" : "row";
      const rawSizes = Array.isArray(n.sizes) ? (n.sizes as number[]) : [];
      const children: PaneNode[] = [];
      const sizes: number[] = [];

      n.children.forEach((child, index) => {
        const built = rebuild(child);
        if (built) {
          children.push(built);
          sizes.push(typeof rawSizes[index] === "number" ? rawSizes[index]! : 0.5);
        }
      });

      if (children.length === 0) return null;
      return normalise({ type: "split", id: nextId("split"), direction, children, sizes });
    }

    return null;
  };

  return rebuild(parsed);
}
