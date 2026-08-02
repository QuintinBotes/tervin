/**
 * Saving and restoring the workspace across a restart.
 *
 * Closing a terminal and losing the arrangement you built — four panes, each in the
 * right directory, one on a remote host — is a real cost, and it is the reason people
 * keep tmux running under a terminal that cannot do this.
 *
 * ## What is restored, and what cannot be
 *
 * The *shape* survives: tabs, the split tree, each pane's directory, and the visible
 * history. The *processes* do not — they exited when the app did, and pretending
 * otherwise would be the dishonest kind of restore, where a pane looks alive and the
 * first keystroke reveals it is not. Each restored pane starts a fresh shell in its old
 * directory, below its old output, with a line saying so.
 *
 * ## Why the logic here is pure
 *
 * Everything in this file is a plain function over plain values, so the awkward cases —
 * a truncated file, a layout referring to panes that are not in the snapshot, a tab with
 * no panes left — are tested directly rather than by driving a renderer. A saved session
 * is read from disk on every launch, which makes it exactly the kind of input that has to
 * survive being wrong.
 */

import { deserialise, leaf, listPanes, serialise, type PaneNode } from "./panes";

/** The current snapshot format. Bumped when a change would misread an old file. */
export const SESSION_VERSION = 1;

/** The workspace id the snapshot is stored under. */
export const SESSION_ID = "last-session";

/** One pane, as saved. Only what a fresh pane needs to be started the same way. */
export interface SavedPane {
  id: string;
  title: string;
  cwd: string;
  /** Set for SSH, tmux, serial and agent panes; absent for a plain shell. */
  program?: string;
  args?: string[];
  /** True when the pane reaches beyond this machine. */
  remote?: boolean;
}

export interface SavedTab {
  title: string;
  /** The split tree, as `panes.serialise` writes it. */
  tree: string;
  activePaneId: string | null;
}

export interface SessionSnapshot {
  version: number;
  savedAt: string;
  tabs: SavedTab[];
  panes: SavedPane[];
  activeTabIndex: number;
}

/** The subset of workspace state a snapshot is built from. */
export interface SnapshotSource {
  tabs: Array<{
    id: string;
    title: string;
    root: PaneNode | null;
    activePaneId: string | null;
  }>;
  activeTabId: string | null;
  panes: Record<
    string,
    {
      id: string;
      title: string;
      cwd: string;
      program?: string;
      args?: string[];
      remote?: boolean;
      threadId: string | null;
    }
  >;
}

/**
 * Build a snapshot of the current workspace.
 *
 * `savedAt` is passed in rather than read from the clock, so a test can assert the whole
 * value without freezing time.
 */
export function buildSnapshot(state: SnapshotSource, savedAt: string): SessionSnapshot {
  const tabs: SavedTab[] = [];
  const kept = new Set<string>();

  for (const tab of state.tabs) {
    if (!tab.root) continue;
    const ids = listPanes(tab.root).filter((id) => {
      const pane = state.panes[id];
      // A pane hosting an agent Tervin launched is not restorable: the session is
      // gone, and reopening it as a bare shell would be a different thing wearing its
      // title. Those Threads are on disk and reachable from History instead.
      return pane && !pane.threadId;
    });
    if (ids.length === 0) continue;

    // The tree is filtered to the panes being kept, so a layout can never refer to a
    // pane the snapshot does not carry.
    const pruned = deserialise(serialise(tab.root), (id) => (ids.includes(id) ? id : null));
    if (!pruned) continue;

    ids.forEach((id) => kept.add(id));
    tabs.push({
      title: tab.title,
      tree: serialise(pruned),
      activePaneId:
        tab.activePaneId && ids.includes(tab.activePaneId) ? tab.activePaneId : (ids[0] ?? null),
    });
  }

  const activeTabIndex = Math.max(
    0,
    state.tabs.filter((t) => t.root).findIndex((t) => t.id === state.activeTabId),
  );

  return {
    version: SESSION_VERSION,
    savedAt,
    tabs,
    panes: [...kept].flatMap((id) => {
      const pane = state.panes[id];
      // Only ids that came from `kept` reach here, so this cannot be missing — but the
      // index signature says it can, and asserting would be a lie rather than a check.
      if (!pane) return [];
      const saved: SavedPane = { id, title: pane.title, cwd: pane.cwd };
      // Written only when set, so a snapshot of plain shells stays small and readable.
      if (pane.program) saved.program = pane.program;
      if (pane.args?.length) saved.args = pane.args;
      if (pane.remote) saved.remote = true;
      return [saved];
    }),
    // Clamped: the index is into the *kept* tabs, and a tab may have been dropped.
    activeTabIndex: Math.min(activeTabIndex, Math.max(0, tabs.length - 1)),
  };
}

/**
 * Read a snapshot back, returning null for anything unusable.
 *
 * Deliberately forgiving field by field rather than all-or-nothing: a snapshot missing a
 * title should still restore the layout, because losing four panes over one bad string
 * is a worse outcome than a pane called "Shell".
 */
export function parseSnapshot(json: string | null | undefined): SessionSnapshot | null {
  if (!json) return null;
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return null;
  }
  if (!raw || typeof raw !== "object") return null;
  const obj = raw as Record<string, unknown>;

  // A future version may mean anything. Refusing to guess is better than restoring a
  // layout that was described differently.
  if (typeof obj.version !== "number" || obj.version > SESSION_VERSION) return null;

  const panes: SavedPane[] = [];
  if (Array.isArray(obj.panes)) {
    for (const entry of obj.panes) {
      if (!entry || typeof entry !== "object") continue;
      const p = entry as Record<string, unknown>;
      if (typeof p.id !== "string" || !p.id) continue;
      panes.push({
        id: p.id,
        title: typeof p.title === "string" && p.title ? p.title : "Shell",
        cwd: typeof p.cwd === "string" ? p.cwd : "",
        ...(typeof p.program === "string" && p.program ? { program: p.program } : {}),
        ...(Array.isArray(p.args) ? { args: p.args.filter((a) => typeof a === "string") } : {}),
        ...(p.remote === true ? { remote: true } : {}),
      });
    }
  }

  const tabs: SavedTab[] = [];
  if (Array.isArray(obj.tabs)) {
    for (const entry of obj.tabs) {
      if (!entry || typeof entry !== "object") continue;
      const t = entry as Record<string, unknown>;
      if (typeof t.tree !== "string") continue;
      tabs.push({
        title: typeof t.title === "string" && t.title ? t.title : "Shell",
        tree: t.tree,
        activePaneId: typeof t.activePaneId === "string" ? t.activePaneId : null,
      });
    }
  }

  if (tabs.length === 0 || panes.length === 0) return null;

  return {
    version: obj.version,
    savedAt: typeof obj.savedAt === "string" ? obj.savedAt : "",
    tabs,
    panes,
    activeTabIndex:
      typeof obj.activeTabIndex === "number" && obj.activeTabIndex >= 0
        ? Math.min(obj.activeTabIndex, tabs.length - 1)
        : 0,
  };
}

/** A pane to create while restoring, carrying the saved id it stands in for. */
export interface RestoredPane extends SavedPane {
  /** The new id. */
  newId: string;
  /** The saved id, used to fetch that pane's stored output. */
  restoreKey: string;
}

export interface RestoredTab {
  title: string;
  root: PaneNode;
  activePaneId: string | null;
  paneIds: string[];
}

export interface RestorePlan {
  tabs: RestoredTab[];
  panes: RestoredPane[];
  activeTabIndex: number;
}

/**
 * Turn a snapshot into the tabs and panes to create.
 *
 * `freshId` supplies a new pane id per saved pane, because the old ones belong to
 * processes that no longer exist. Tabs whose panes have all been dropped are left out
 * rather than restored empty — an empty tab is a puzzle, not a restored session.
 */
export function planRestore(
  snapshot: SessionSnapshot,
  freshId: (savedPane: SavedPane) => string,
): RestorePlan {
  const bySavedId = new Map(snapshot.panes.map((p) => [p.id, p]));
  const created = new Map<string, RestoredPane>();

  const tabs: RestoredTab[] = [];
  for (const tab of snapshot.tabs) {
    const idsInTab: string[] = [];
    const root = deserialise(tab.tree, (savedId) => {
      const saved = bySavedId.get(savedId);
      // A leaf with no matching pane entry cannot be started, so it is dropped and the
      // tree collapses around it — which `deserialise` already handles.
      if (!saved) return null;
      // One new pane per saved pane, even if a malformed tree names it twice.
      let pane = created.get(savedId);
      if (!pane) {
        pane = { ...saved, newId: freshId(saved), restoreKey: savedId };
        created.set(savedId, pane);
      } else if (idsInTab.includes(pane.newId)) {
        return null;
      }
      idsInTab.push(pane.newId);
      return pane.newId;
    });
    if (!root || idsInTab.length === 0) continue;

    const activeSaved = tab.activePaneId ? created.get(tab.activePaneId) : undefined;
    tabs.push({
      title: tab.title,
      root,
      activePaneId:
        activeSaved && idsInTab.includes(activeSaved.newId)
          ? activeSaved.newId
          : (idsInTab[0] ?? null),
      paneIds: idsInTab,
    });
  }

  // Only the panes that ended up in a tab. A pane entry no tree references would
  // otherwise be created and never shown.
  const used = new Set(tabs.flatMap((t) => t.paneIds));
  const panes = [...created.values()].filter((p) => used.has(p.newId));

  return {
    tabs,
    panes,
    activeTabIndex: Math.min(Math.max(0, snapshot.activeTabIndex), Math.max(0, tabs.length - 1)),
  };
}

/** A single-pane tab, for the fallback when there is nothing to restore. */
export function singlePaneTab(paneId: string, title: string): RestoredTab {
  return { title, root: leaf(paneId), activePaneId: paneId, paneIds: [paneId] };
}
