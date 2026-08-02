import { describe, expect, it } from "vitest";
import {
  closePane,
  deserialise,
  evenSizes,
  leaf,
  listPanes,
  nextPane,
  paneCount,
  parentOf,
  prevPane,
  resizeSplit,
  serialise,
  splitPane,
  swapPanes,
  type PaneNode,
} from "./panes";

/** Render the tree's shape as a string, for readable assertions. */
function shape(node: PaneNode): string {
  if (node.type === "leaf") return node.paneId;
  const sep = node.direction === "row" ? " | " : " / ";
  return `(${node.children.map(shape).join(sep)})`;
}

/** Every split's sizes must sum to 1, or the layout drifts as panes change. */
function assertSizesSumToOne(node: PaneNode) {
  if (node.type === "leaf") return;
  const total = node.sizes.reduce((a, b) => a + b, 0);
  expect(total).toBeCloseTo(1, 6);
  expect(node.sizes).toHaveLength(node.children.length);
  node.children.forEach(assertSizesSumToOne);
}

describe("splitting", () => {
  it("splits a single pane in two", () => {
    const tree = splitPane(leaf("a"), "a", "row", "b");
    expect(shape(tree)).toBe("(a | b)");
    assertSizesSumToOne(tree);
  });

  it("supports mixed horizontal and vertical splits", () => {
    // The case a flat list cannot express: a beside a stacked pair.
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "column", "c");
    expect(shape(tree)).toBe("(a | (b / c))");
    assertSizesSumToOne(tree);
  });

  it("adds a sibling instead of nesting when the direction matches", () => {
    // Three side-by-side panes should share one row of dividers.
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "row", "c");
    expect(shape(tree)).toBe("(a | b | c)");
    if (tree.type !== "split") throw new Error("expected a split");
    expect(tree.children).toHaveLength(3);
    assertSizesSumToOne(tree);
  });

  it("splitting an unknown pane leaves the tree untouched", () => {
    const tree = splitPane(leaf("a"), "nope", "row", "b");
    expect(shape(tree)).toBe("a");
  });

  it("keeps the split proportional to the pane it replaced", () => {
    // Splitting the small half of an uneven split must not enlarge it.
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = resizeSplit(tree, (tree as never as { id: string }).id, 0, -0.25);
    if (tree.type !== "split") throw new Error("expected a split");
    const before = tree.sizes[0]!;
    const after = splitPane(tree, "a", "column", "c");
    if (after.type !== "split") throw new Error("expected a split");
    expect(after.sizes[0]).toBeCloseTo(before, 6);
  });
});

describe("closing", () => {
  it("collapses the parent split when one child remains", () => {
    // Otherwise an invisible single-child split accumulates and breaks dragging.
    let tree = splitPane(leaf("a"), "a", "row", "b");
    const closed = closePane(tree, "b")!;
    expect(shape(closed)).toBe("a");
    expect(closed.type).toBe("leaf");
  });

  it("returns null when the last pane closes", () => {
    expect(closePane(leaf("a"), "a")).toBeNull();
  });

  it("keeps the rest of the layout when closing a middle pane", () => {
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "row", "c");
    const closed = closePane(tree, "b")!;
    expect(shape(closed)).toBe("(a | c)");
    assertSizesSumToOne(closed);
  });

  it("redistributes the closed pane's space", () => {
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "row", "c");
    const closed = closePane(tree, "c")!;
    assertSizesSumToOne(closed);
  });

  it("collapses nested splits correctly", () => {
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "column", "c");
    expect(shape(tree)).toBe("(a | (b / c))");

    const closed = closePane(tree, "c")!;
    expect(shape(closed)).toBe("(a | b)");
    assertSizesSumToOne(closed);
  });

  it("closing an unknown pane changes nothing", () => {
    const tree = splitPane(leaf("a"), "a", "row", "b");
    expect(shape(closePane(tree, "zzz")!)).toBe("(a | b)");
  });
});

describe("focus order", () => {
  it("cycles left to right, depth first", () => {
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "column", "c");
    expect(listPanes(tree)).toEqual(["a", "b", "c"]);
  });

  it("wraps at both ends", () => {
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "row", "c");
    expect(nextPane(tree, "c")).toBe("a");
    expect(prevPane(tree, "a")).toBe("c");
    expect(nextPane(tree, "a")).toBe("b");
  });

  it("falls back sensibly for an unknown pane", () => {
    const tree = splitPane(leaf("a"), "a", "row", "b");
    expect(nextPane(tree, "gone")).toBe("a");
    expect(prevPane(tree, "gone")).toBe("b");
  });
});

describe("swapping", () => {
  it("exchanges two panes without changing the layout", () => {
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "column", "c");
    const swapped = swapPanes(tree, "a", "c");
    expect(shape(swapped)).toBe("(c | (b / a))");
  });

  it("swapping a pane with itself is a no-op", () => {
    const tree = splitPane(leaf("a"), "a", "row", "b");
    expect(shape(swapPanes(tree, "a", "a"))).toBe("(a | b)");
  });
});

describe("resizing", () => {
  it("moves a divider between two neighbours", () => {
    const tree = splitPane(leaf("a"), "a", "row", "b");
    if (tree.type !== "split") throw new Error("expected a split");
    const resized = resizeSplit(tree, tree.id, 0, 0.2);
    if (resized.type !== "split") throw new Error("expected a split");
    expect(resized.sizes[0]).toBeCloseTo(0.7, 6);
    expect(resized.sizes[1]).toBeCloseTo(0.3, 6);
    assertSizesSumToOne(resized);
  });

  it("never collapses a pane to nothing", () => {
    // Recovering from a zero-width pane means closing it, which loses the session.
    const tree = splitPane(leaf("a"), "a", "row", "b");
    if (tree.type !== "split") throw new Error("expected a split");
    const resized = resizeSplit(tree, tree.id, 0, -10);
    if (resized.type !== "split") throw new Error("expected a split");
    expect(resized.sizes[0]).toBeGreaterThan(0.05);
    expect(resized.sizes[1]).toBeLessThan(0.95);
    assertSizesSumToOne(resized);
  });

  it("ignores a resize of an unknown split", () => {
    const tree = splitPane(leaf("a"), "a", "row", "b");
    expect(shape(resizeSplit(tree, "no-such-split", 0, 0.3))).toBe("(a | b)");
  });

  it("evens out every split", () => {
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "row", "c");
    if (tree.type !== "split") throw new Error("expected a split");
    const even = evenSizes(resizeSplit(tree, tree.id, 0, 0.3));
    if (even.type !== "split") throw new Error("expected a split");
    even.sizes.forEach((s) => expect(s).toBeCloseTo(1 / 3, 6));
  });
});

describe("structure", () => {
  it("finds the split that owns a pane", () => {
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "column", "c");
    const parent = parentOf(tree, "c");
    expect(parent?.type).toBe("split");
    if (parent?.type === "split") expect(parent.direction).toBe("column");
  });

  it("a lone leaf has no parent", () => {
    expect(parentOf(leaf("a"), "a")).toBeNull();
  });

  it("counts panes", () => {
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "column", "c");
    expect(paneCount(tree)).toBe(3);
  });
});

describe("session restore", () => {
  it("round-trips a layout with fresh pane ids", () => {
    // The shape survives a restart; the processes do not.
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "column", "c");

    const restored = deserialise(serialise(tree), (old) => `new-${old}`)!;
    expect(shape(restored)).toBe("(new-a | (new-b / new-c))");
    assertSizesSumToOne(restored);
  });

  it("preserves divider positions across a restart", () => {
    const tree = splitPane(leaf("a"), "a", "row", "b");
    if (tree.type !== "split") throw new Error("expected a split");
    const resized = resizeSplit(tree, tree.id, 0, 0.2);

    const restored = deserialise(serialise(resized), (old) => old)!;
    if (restored.type !== "split") throw new Error("expected a split");
    expect(restored.sizes[0]).toBeCloseTo(0.7, 6);
  });

  it("drops panes the caller declines to recreate", () => {
    let tree = splitPane(leaf("a"), "a", "row", "b");
    tree = splitPane(tree, "b", "row", "c");

    // A pane whose session cannot be restored is simply left out.
    const restored = deserialise(serialise(tree), (old) => (old === "b" ? null : old))!;
    expect(shape(restored)).toBe("(a | c)");
    assertSizesSumToOne(restored);
  });

  it("returns null when nothing can be restored", () => {
    const tree = splitPane(leaf("a"), "a", "row", "b");
    expect(deserialise(serialise(tree), () => null)).toBeNull();
  });

  it("rejects malformed input instead of throwing", () => {
    expect(deserialise("not json", (id) => id)).toBeNull();
    expect(deserialise('{"type":"nonsense"}', (id) => id)).toBeNull();
    expect(deserialise("null", (id) => id)).toBeNull();
  });

  it("repairs a saved tree with a single-child split", () => {
    // An older or hand-edited layout must not resurrect the invariant violation.
    const malformed = JSON.stringify({
      type: "split",
      id: "s1",
      direction: "row",
      sizes: [1],
      children: [{ type: "leaf", id: "l1", paneId: "a" }],
    });
    const restored = deserialise(malformed, (id) => id)!;
    expect(restored.type).toBe("leaf");
  });
});
