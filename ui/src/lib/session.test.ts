/**
 * Session snapshots.
 *
 * A saved session is read from disk on every launch, which makes it the kind of input
 * that has to survive being wrong: truncated by a crash, written by an older build, or
 * describing panes that are no longer in it. Getting that wrong does not fail loudly —
 * it produces an empty tab, or a layout missing half its panes, on startup.
 */

import { describe, expect, it } from "vitest";
import { leaf, listPanes, paneCount, serialise, splitPane } from "./panes";
import {
  buildSnapshot,
  parseSnapshot,
  planRestore,
  SESSION_VERSION,
  type SavedPane,
  type SnapshotSource,
} from "./session";

const WHEN = "2026-08-02T12:00:00.000Z";

function pane(id: string, over: Partial<SnapshotSource["panes"][string]> = {}) {
  return {
    id,
    title: id,
    cwd: `/proj/${id}`,
    threadId: null,
    ...over,
  };
}

/** A workspace with two tabs: one split in two, one single pane. */
function twoTabs(): SnapshotSource {
  // p1, split horizontally so p2 sits beside it.
  const b = splitPane(leaf("p1"), "p1", "row", "p2");
  return {
    activeTabId: "t2",
    tabs: [
      { id: "t1", title: "work", root: b, activePaneId: "p2" },
      { id: "t2", title: "logs", root: leaf("p3"), activePaneId: "p3" },
    ],
    panes: {
      p1: pane("p1"),
      p2: pane("p2", { program: "ssh", args: ["build-box"], remote: true }),
      p3: pane("p3"),
    },
  };
}

let counter = 0;
const fresh = () => `new-${++counter}`;

describe("buildSnapshot", () => {
  it("records the layout, each pane's directory, and how it was started", () => {
    const snap = buildSnapshot(twoTabs(), WHEN);

    expect(snap.version).toBe(SESSION_VERSION);
    expect(snap.tabs.map((t) => t.title)).toEqual(["work", "logs"]);
    expect(snap.panes.map((p) => p.id).sort()).toEqual(["p1", "p2", "p3"]);
    // Enough to start the same thing again, not a copy of the whole pane.
    expect(snap.panes.find((p) => p.id === "p2")).toEqual({
      id: "p2",
      title: "p2",
      cwd: "/proj/p2",
      program: "ssh",
      args: ["build-box"],
      remote: true,
    });
    // A plain shell carries no program, so the snapshot stays readable.
    expect(snap.panes.find((p) => p.id === "p1")).toEqual({
      id: "p1",
      title: "p1",
      cwd: "/proj/p1",
    });
    // The second tab was active.
    expect(snap.activeTabIndex).toBe(1);
  });

  it("leaves out a pane hosting an agent Tervin launched", () => {
    // The session is gone, and reopening it as a bare shell would be a different thing
    // wearing its title. Those Threads are on disk and reachable from History.
    const state = twoTabs();
    state.panes.p2 = pane("p2", { threadId: "thr_1" });

    const snap = buildSnapshot(state, WHEN);
    expect(snap.panes.map((p) => p.id)).not.toContain("p2");
    // Its tab survives, holding the pane that is restorable.
    const work = snap.tabs.find((t) => t.title === "work")!;
    expect(listPanes(JSON.parse(work.tree))).toEqual(["p1"]);
  });

  it("drops a tab whose every pane is unrestorable rather than saving it empty", () => {
    const state = twoTabs();
    state.panes.p3 = pane("p3", { threadId: "thr_1" });

    const snap = buildSnapshot(state, WHEN);
    expect(snap.tabs.map((t) => t.title)).toEqual(["work"]);
    // The active index pointed at the tab that is gone, so it has to be brought back
    // into range or restore would select nothing.
    expect(snap.activeTabIndex).toBe(0);
  });

  it("never references a pane it does not carry", () => {
    // The invariant that matters: a tree naming a pane with no entry restores as a gap.
    const state = twoTabs();
    state.panes.p1 = pane("p1", { threadId: "thr_1" });

    const snap = buildSnapshot(state, WHEN);
    const known = new Set(snap.panes.map((p) => p.id));
    for (const tab of snap.tabs) {
      for (const id of listPanes(JSON.parse(tab.tree))) {
        expect(known.has(id)).toBe(true);
      }
    }
  });

  it("falls back to a real pane when the active one is not restorable", () => {
    const state = twoTabs();
    // p2 was active in that tab.
    state.panes.p2 = pane("p2", { threadId: "thr_1" });
    const snap = buildSnapshot(state, WHEN);
    expect(snap.tabs.find((t) => t.title === "work")!.activePaneId).toBe("p1");
  });

  it("survives a tab that has no pane tree yet", () => {
    // The state between creating a tab and opening its first pane.
    const state = twoTabs();
    state.tabs.push({ id: "t3", title: "new", root: null, activePaneId: null });
    expect(() => buildSnapshot(state, WHEN)).not.toThrow();
    expect(buildSnapshot(state, WHEN).tabs).toHaveLength(2);
  });
});

describe("parseSnapshot", () => {
  const round = (state: SnapshotSource) => JSON.stringify(buildSnapshot(state, WHEN));

  it("round-trips what buildSnapshot wrote", () => {
    const parsed = parseSnapshot(round(twoTabs()))!;
    expect(parsed.tabs).toHaveLength(2);
    expect(parsed.panes).toHaveLength(3);
    expect(parsed.activeTabIndex).toBe(1);
  });

  it("returns null for anything unusable rather than throwing", () => {
    // Each of these is a real way the file ends up wrong: never written, emptied,
    // truncated by a crash, or holding something else entirely.
    for (const input of [
      null,
      undefined,
      "",
      "not json",
      '{"version":1}',
      '{"version":1,"tabs":[],"panes":[]}',
      "[]",
      "null",
      '{"tabs":[{"tree":"{}"}],"panes":[{"id":"p1"}]}', // no version
    ]) {
      expect(parseSnapshot(input as string | null)).toBeNull();
    }
  });

  it("refuses a snapshot from a newer build", () => {
    // A future version may describe the layout differently. Refusing to guess beats
    // restoring something that was written to mean something else.
    const future = JSON.stringify({
      ...buildSnapshot(twoTabs(), WHEN),
      version: SESSION_VERSION + 1,
    });
    expect(parseSnapshot(future)).toBeNull();
  });

  it("keeps the layout when a single field is missing", () => {
    // Losing four panes over one absent string would be the worse outcome.
    const snapshot = {
      version: 1,
      tabs: [{ tree: serialise(leaf("p1")) }],
      panes: [{ id: "p1" }],
    };
    const parsed = parseSnapshot(JSON.stringify(snapshot))!;
    expect(parsed.panes[0]!).toEqual({ id: "p1", title: "Shell", cwd: "" });
    expect(parsed.tabs[0]!.title).toBe("Shell");
    expect(parsed.tabs[0]!.activePaneId).toBeNull();
  });

  it("skips entries that are the wrong shape, keeping the rest", () => {
    const snapshot = {
      version: 1,
      tabs: [
        { title: "good", tree: serialise(leaf("p1")), activePaneId: "p1" },
        { title: "no tree" },
        "not an object",
      ],
      panes: [{ id: "p1", title: "one", cwd: "/a" }, { title: "no id" }, 42],
    };
    const parsed = parseSnapshot(JSON.stringify(snapshot))!;
    expect(parsed.tabs).toHaveLength(1);
    expect(parsed.panes).toHaveLength(1);
  });

  it("brings an out-of-range active index back into range", () => {
    const snapshot = {
      version: 1,
      activeTabIndex: 99,
      tabs: [{ title: "only", tree: serialise(leaf("p1")) }],
      panes: [{ id: "p1" }],
    };
    expect(parseSnapshot(JSON.stringify(snapshot))!.activeTabIndex).toBe(0);
  });
});

describe("planRestore", () => {
  const snapshotOf = (state: SnapshotSource) => parseSnapshot(JSON.stringify(buildSnapshot(state, WHEN)))!;

  it("recreates the shape with new pane ids, keyed back to the saved ones", () => {
    const plan = planRestore(snapshotOf(twoTabs()), fresh);

    expect(plan.tabs.map((t) => t.title)).toEqual(["work", "logs"]);
    expect(paneCount(plan.tabs[0]!.root)).toBe(2);
    expect(plan.panes).toHaveLength(3);

    // Processes cannot be revived, so every pane is new — but each remembers which
    // saved pane it replaces, which is how its stored output is found.
    for (const p of plan.panes) {
      expect(p.newId).not.toBe(p.restoreKey);
      expect(p.restoreKey).toMatch(/^p\d$/);
    }
    // The tree refers to the new ids, not the saved ones.
    expect(listPanes(plan.tabs[0]!.root).every((id) => id.startsWith("new-"))).toBe(true);
  });

  it("carries the program forward, so an SSH pane restores as SSH", () => {
    const plan = planRestore(snapshotOf(twoTabs()), fresh);
    const remote = plan.panes.find((p) => p.restoreKey === "p2")!;
    expect(remote.program).toBe("ssh");
    expect(remote.args).toEqual(["build-box"]);
    expect(remote.remote).toBe(true);
  });

  it("drops a leaf whose pane entry is missing and collapses the tree around it", () => {
    // The layout and the pane list are stored separately, so they can disagree — a
    // partial write, or an edited file.
    const snapshot = snapshotOf(twoTabs());
    snapshot.panes = snapshot.panes.filter((p) => p.id !== "p1");

    const plan = planRestore(snapshot, fresh);
    const work = plan.tabs.find((t) => t.title === "work")!;
    // One pane left, and no split wrapping a single child.
    expect(paneCount(work.root)).toBe(1);
    expect(work.root.type).toBe("leaf");
  });

  it("leaves out a tab with nothing left in it", () => {
    const snapshot = snapshotOf(twoTabs());
    snapshot.panes = snapshot.panes.filter((p) => p.id === "p3");

    const plan = planRestore(snapshot, fresh);
    // An empty tab is a puzzle, not a restored session.
    expect(plan.tabs.map((t) => t.title)).toEqual(["logs"]);
    expect(plan.activeTabIndex).toBe(0);
  });

  it("creates one pane per saved pane even if a tree names it twice", () => {
    // A malformed tree must not produce two panes sharing an id, which would make
    // focus and close operations act on the wrong one.
    const snapshot = parseSnapshot(
      JSON.stringify({
        version: 1,
        // A tree naming the same pane in both leaves.
        tabs: [{ title: "t", tree: serialise(splitPane(leaf("p1"), "p1", "row", "p1")) }],
        panes: [{ id: "p1", cwd: "/a" }],
      }),
    )!;

    const plan = planRestore(snapshot, fresh);
    const ids = listPanes(plan.tabs[0]!.root);
    expect(new Set(ids).size).toBe(ids.length);
    expect(plan.panes).toHaveLength(1);
  });

  it("does not create a pane no tab references", () => {
    const snapshot = snapshotOf(twoTabs());
    snapshot.panes.push({ id: "orphan", title: "orphan", cwd: "/x" } as SavedPane);

    const plan = planRestore(snapshot, fresh);
    expect(plan.panes.map((p) => p.restoreKey)).not.toContain("orphan");
  });

  it("restores which tab and pane were focused", () => {
    const plan = planRestore(snapshotOf(twoTabs()), fresh);
    expect(plan.activeTabIndex).toBe(1);
    const work = plan.tabs[0]!;
    const wasActive = plan.panes.find((p) => p.restoreKey === "p2")!;
    expect(work.activePaneId).toBe(wasActive.newId);
  });

  it("picks a real pane when the focused one is gone", () => {
    const snapshot = snapshotOf(twoTabs());
    snapshot.panes = snapshot.panes.filter((p) => p.id !== "p2");
    const plan = planRestore(snapshot, fresh);
    const work = plan.tabs.find((t) => t.title === "work")!;
    expect(work.activePaneId).toBe(listPanes(work.root)[0]);
  });
});
