/**
 * Workspace state.
 *
 * Terminal *content* is deliberately absent from this store. Scrollback lives
 * inside each xterm instance, which owns its own buffer and renderer; putting
 * bytes into React state would mean re-rendering on every frame of output. What
 * lives here is the small, slow-moving state the chrome renders from.
 */

import { create } from "zustand";
import * as api from "./api";
import { DEFAULT_THEME_ID, findTheme } from "../design/themes";
import {
  closePane as closePaneNode,
  leaf,
  listPanes,
  nextPane as nextPaneOf,
  prevPane as prevPaneOf,
  resizeSplit,
  splitPane,
  swapPanes,
  type PaneNode,
  type SplitDirection,
} from "./panes";
import {
  buildSnapshot,
  parseSnapshot,
  planRestore,
  SESSION_ID,
  type SavedPane,
} from "./session";

/**
 * The surfaces the top bar switches between.
 *
 * Replaces the earlier rail-plus-inspector model, which was three columns. The
 * design system allows two.
 */
export type Surface = "terminal" | "plan" | "agents" | "review" | "history";

/** Which activity the rail has selected, driving the inspector's content. */
export type Activity =
  | "workspace"
  | "files"
  | "git"
  | "threads"
  | "tasks"
  | "history"
  | "connections"
  | "bridge"
  | "settings";

export type InspectorTab =
  | "thread"
  | "review"
  | "files"
  | "git"
  | "diagnostics"
  | "connections"
  | "details";

/** A pane in the terminal canvas. */
export interface Pane {
  id: string;
  title: string;
  cwd: string;
  /** Set when this pane hosts an agent rather than a shell. */
  threadId: string | null;
  exited: boolean;
  exitCode: number | null;
  /**
   * Program to run instead of the user's shell.
   *
   * Set for SSH, tmux, serial, WSL, and Tier 3 agent panes — anything resolved
   * through `session-manager` rather than opened as a plain shell.
   */
  program?: string;
  args?: string[];
  env?: [string, string][];
  /**
   * The pane id from a saved session that this pane replaces.
   *
   * Set only while restoring. Pane ids are generated per run, so saved output is keyed
   * by the old id and this is what maps it onto the pane that took its place.
   */
  restoreKey?: string;
  /**
   * True when the pane reaches beyond this machine.
   *
   * The status rail shows it, and it changes what a destructive command means.
   */
  remote?: boolean;
}

/**
 * A tab owns a pane tree.
 *
 * A tree rather than a list, because "arbitrary horizontal and vertical splits"
 * cannot be expressed as a list the moment someone splits one half of an existing
 * split — which is the second thing anyone does in a terminal.
 */
export interface Tab {
  id: string;
  title: string;
  /** Null only between creating a tab and opening its first pane. */
  root: PaneNode | null;
  activePaneId: string | null;
  /** Set when one pane is zoomed to fill the tab. */
  zoomedPaneId: string | null;
}

/** Typography and terminal behaviour, persisted to the workspace database. */
export interface Appearance {
  themeId: string;
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  ligatures: boolean;
  cursorStyle: "block" | "underline" | "bar";
  cursorBlink: boolean;
  copyOnSelect: boolean;
  /** Blocks are shown as structured units, or the pane runs continuous. */
  blocksEnabled: boolean;
  /** Lines of live scrollback held per pane. */
  scrollback: number;
  /**
   * Characters that end a word for double-click selection.
   *
   * Excludes `/`, `.`, `-` and `_` so a path or a flag selects as one token,
   * which is what a terminal user almost always wants.
   */
  wordSeparators: string;
  /** Announce output to assistive technology. */
  screenReaderMode: boolean;
  /**
   * Editing bindings for the prompt composer.
   *
   * `native` by default. A terminal user has muscle memory, but guessing which is a
   * worse failure than not guessing — someone who has not asked for vim would find
   * their text box eating keystrokes.
   */
  composerMode: "native" | "emacs" | "vim";
  /**
   * Where the tab strip lives.
   *
   * A preference rather than a decision, because it is genuinely contested: people who
   * came from iTerm expect the top, people who came from a tiling window manager often
   * want the side, and a vertical strip is the only one that stays readable with twenty
   * tabs open.
   */
  tabBarPosition: "top" | "bottom" | "left" | "right";
  /** Whether the file explorer column is shown. */
  explorerVisible: boolean;
  explorerSide: "left" | "right";
  /** Show dot-files in the explorer. */
  explorerShowHidden: boolean;
  /**
   * What the tab strip's `+` does.
   *
   * It used to add a pane, which split the view and made the tab look renamed. Now it
   * makes a tab by default — but someone who mostly splits should be able to say so
   * rather than reaching for a different control every time.
   */
  newButtonAction: "tab" | "pane";
  /**
   * Reopen the last session's tabs, panes, directories and recent output.
   *
   * On by default. Losing the arrangement you built — four panes, each in the right
   * directory, one on a remote host — is the cost that keeps people running tmux under a
   * terminal that cannot do this.
   *
   * The processes are not revived, and each restored pane says so above its new prompt.
   * Recent output is kept in the same local database as Blocks, ages out on the same
   * retention window, and is cleared the moment this is switched off.
   */
  restoreSession: boolean;
}

export const DEFAULT_APPEARANCE: Appearance = {
  themeId: DEFAULT_THEME_ID,
  // A Nerd Font first: powerlevel10k and starship prompts draw glyphs that only
  // exist in patched fonts, and falling back silently makes a prompt look broken.
  fontFamily:
    '"MesloLGS NF", "JetBrainsMono Nerd Font", "Berkeley Mono", "JetBrains Mono", "Iosevka", ui-monospace, Menlo, monospace',
  fontSize: 13,
  lineHeight: 1.4,
  ligatures: false,
  cursorStyle: "block",
  cursorBlink: true,
  copyOnSelect: false,
  blocksEnabled: true,
  scrollback: 10_000,
  wordSeparators: " ()[]{}',\"`;<>",
  screenReaderMode: false,
  composerMode: "native",
  tabBarPosition: "top",
  explorerVisible: false,
  explorerSide: "left",
  explorerShowHidden: false,
  newButtonAction: "tab",
  restoreSession: true,
};

/**
 * Whether anything is layered over the workspace.
 *
 * The terminal must not hold keyboard focus while it is. A pane holding focus
 * under a dialog means a keystroke meant for the dialog reaches the shell, and
 * `Return` in an approval sheet would run a command — so this is a correctness
 * question, not a polish one.
 */
export function overlayOpen(state: WorkspaceState): boolean {
  return (
    state.paletteOpen ||
    state.searchOpen ||
    state.settingsOpen ||
    state.connectionsOpen ||
    state.directoryJumpOpen ||
    state.pendingApprovals.length > 0
  );
}

interface WorkspaceState {
  // layout
  surface: Surface;
  /** Width of the list column in a two-column surface, in pixels. Persisted. */
  listColumnWidth: number;
  paletteOpen: boolean;
  searchOpen: boolean;
  settingsOpen: boolean;
  /** The Connections overlay: SSH hosts, tmux sessions, serial ports. */
  connectionsOpen: boolean;
  /** The directory jump overlay. */
  directoryJumpOpen: boolean;

  // terminal canvas
  tabs: Tab[];
  panes: Record<string, Pane>;
  activeTabId: string | null;

  // data
  appearance: Appearance;
  environment: api.ShellEnvironment | null;
  gitStatus: api.RepoStatus | null;
  blocks: api.BlockSummary[];
  blockFilter: api.BlockFilter;
  agents: api.AgentsOverview | null;
  activeProfileId: string | null;

  // threads
  threads: Record<string, ThreadView>;
  activeThreadId: string | null;

  // rules
  pendingApprovals: api.ApprovalRequest[];

  /**
   * A handoff briefing waiting to be sent, if one was prepared.
   *
   * Held in the store rather than pushed straight into the composer so switching
   * surfaces does not lose it.
   */
  pendingHandoff: string | null;

  /**
   * Context the user has picked but not yet sent.
   *
   * Held here rather than sent immediately, so nothing leaves the machine until
   * the prompt is actually submitted.
   */
  stagedAttachments: Record<string, unknown>[];

  connections: api.Connections | null;

  notices: string[];
}

/** A Thread as the UI holds it: identity, live state, and its timeline. */
export interface ThreadView {
  id: string;
  profileId: string;
  runtimeId: string;
  title: string;
  state: api.ThreadState;
  events: api.TervinEvent[];
  capabilities: api.Capabilities | null;
  permissions: api.PermissionState | null;
  info: api.ThreadInfo | null;
  /**
   * The pane this Thread is being observed in, for a session the user started
   * themselves rather than one Tervin launched.
   *
   * Present means read-only: Tervin has no channel to a process it did not spawn, so
   * it cannot send a prompt, answer a permission request, or cancel a turn. The
   * composer is hidden rather than shown and silently doing nothing.
   */
  paneId?: string | null;
}

interface WorkspaceActions {
  setSurface: (surface: Surface) => void;
  setListColumnWidth: (width: number) => void;
  setPalette: (open: boolean) => void;
  setSearch: (open: boolean) => void;
  setSettings: (open: boolean) => void;
  setConnections: (open: boolean) => void;
  setDirectoryJump: (open: boolean) => void;

  addPane: (pane: Pane, tabId?: string) => void;
  /**
   * Open the workspace's first pane, once.
   *
   * Idempotent by construction. A component-level guard cannot be trusted for
   * this: a ref resets whenever the component remounts, and a state flag has not
   * flushed when StrictMode re-runs the effect. The invariant — "a fresh workspace
   * has exactly one pane" — belongs where it cannot be bypassed.
   */
  ensureFirstPane: (pane: Pane) => void;
  /**
   * Write the current layout and each pane's recent output to the local database.
   *
   * Takes the serialiser rather than importing it, so this module never depends on the
   * terminal component that depends on it.
   */
  saveSession: (serialisePane: (paneId: string) => string | null) => Promise<void>;
  /**
   * Reopen the last session, returning false when there was nothing to reopen.
   *
   * `freshPane` supplies a pane for each saved one; the caller owns pane construction
   * because it knows the defaults a new pane needs.
   */
  restoreSession: (
    freshPane: (saved: SavedPane) => Pane,
  ) => Promise<boolean>;
  removePane: (paneId: string) => void;
  setActivePane: (paneId: string) => void;
  addTab: () => string;
  setActiveTab: (tabId: string) => void;
  /** Split the focused pane, returning the id the new pane should use. */
  splitFocusedPane: (direction: SplitDirection, newPane: Pane) => void;
  resizeFocusedSplit: (splitId: string, dividerIndex: number, delta: number) => void;
  focusAdjacentPane: (forward: boolean) => void;
  swapFocusedPane: () => void;
  toggleZoom: () => void;
  markPaneExited: (paneId: string, exitCode: number | null) => void;
  /**
   * A pane changed directory.
   *
   * The backend has emitted `pane://cwd` since Blocks existed and nothing listened, so a
   * pane's `cwd` stayed whatever it was at spawn — which made the status rail stale, saved
   * the wrong directory in a session, and made per-pane completion impossible.
   */
  setPaneCwd: (paneId: string, cwd: string) => void;

  setAppearance: (patch: Partial<Appearance>) => void;
  loadAppearance: () => Promise<void>;

  refreshEnvironment: () => Promise<void>;
  refreshGit: () => Promise<void>;
  refreshBlocks: (filter?: api.BlockFilter) => Promise<void>;
  refreshAgents: () => Promise<void>;
  refreshApprovals: () => Promise<void>;
  setActiveProfile: (id: string) => void;

  upsertThread: (thread: ThreadView) => void;
  appendThreadEvent: (event: api.TervinEvent) => void;
  /**
   * Register a Thread for an agent running in a pane.
   *
   * Its events arrive on the same channel as any other Thread's, and
   * `appendThreadEvent` drops events for a Thread it has never heard of — so this has
   * to land first, which the backend guarantees by emitting it before the events.
   */
  observeThread: (thread: api.ObservedThread) => void;
  setThreadState: (threadId: string, state: api.ThreadState) => void;
  setActiveThread: (threadId: string | null) => void;
  /**
   * Load a Thread that is not in memory, from the event store.
   *
   * Needed because a Thread can be reached from prompt history long after its session
   * ended — the events are persisted, but the in-memory view only holds Threads this run
   * has seen.
   */
  openStoredThread: (threadId: string) => Promise<void>;
  refreshThreadInfo: (threadId: string) => Promise<void>;

  stageAttachment: (attachment: Record<string, unknown>) => void;
  clearAttachments: () => void;
  refreshConnections: () => Promise<void>;

  pushNotice: (message: string) => void;
  /**
   * Load a prepared handoff into the composer.
   *
   * Loaded rather than sent: the user picks who receives it, and sees exactly what is
   * being shared before it leaves the machine.
   */
  setHandoff: (prompt: string | null) => void;
  dismissNotice: (index: number) => void;

  /**
   * Kept so panels can say "show me in the Review surface" without knowing the
   * surface model. Maps an old inspector tab onto a surface.
   */
  setInspectorTab: (tab: InspectorTab) => void;
}

const APPEARANCE_KEY = "appearance";

/**
 * Tell the backend whether the current theme is dark, so programs in a pane can be
 * answered and, if they asked, told when it changes.
 *
 * Read from the theme's declared `appearance` rather than measured from its background:
 * the theme's author already decided, and inferring it from a colour would disagree with
 * them on the borderline ones.
 *
 * Best-effort. A program that never asked is unaffected, and one that did will ask again.
 */
function reportColorScheme(themeId: string): void {
  // `findTheme` falls back to the default rather than returning nothing, so an unknown
  // id still produces an honest answer instead of silence.
  void api.colorSchemeSet(findTheme(themeId).appearance === "dark").catch(() => {});
}

let tabCounter = 0;
const nextTabId = () => `tab-${++tabCounter}`;

export const useWorkspace = create<WorkspaceState & WorkspaceActions>((set, get) => ({
  surface: "terminal",
  listColumnWidth: 320,
  paletteOpen: false,
  searchOpen: false,
  settingsOpen: false,
  connectionsOpen: false,
  directoryJumpOpen: false,

  tabs: [],
  panes: {},
  activeTabId: null,

  appearance: DEFAULT_APPEARANCE,
  environment: null,
  gitStatus: null,
  blocks: [],
  blockFilter: { limit: 200 },
  agents: null,
  activeProfileId: null,

  threads: {},
  activeThreadId: null,

  pendingApprovals: [],
  pendingHandoff: null,
  stagedAttachments: [],
  connections: null,
  notices: [],

  // ---------------------------------------------------------------- layout

  setSurface: (surface) => set({ surface }),

  setListColumnWidth: (width) => {
    // Clamped so a drag can never leave the list unusable or the detail pane
    // squeezed out; persisted because a layout that resets on restart is worse
    // than one that cannot be adjusted.
    const clamped = Math.max(220, Math.min(width, 640));
    set({ listColumnWidth: clamped });
    void api.settingsSet("listColumnWidth", String(clamped)).catch(() => {});
  },

  setPalette: (paletteOpen) => set({ paletteOpen }),
  setSearch: (searchOpen) => set({ searchOpen }),
  setSettings: (settingsOpen) => set({ settingsOpen }),
  setConnections: (connectionsOpen) => set({ connectionsOpen }),
  setDirectoryJump: (directoryJumpOpen) => set({ directoryJumpOpen }),
  setHandoff: (pendingHandoff) => set({ pendingHandoff }),

  // ------------------------------------------------------------------ panes

  saveSession: async (serialisePane) => {
    const state = get();
    if (!state.appearance.restoreSession) {
      // Switched off: forget what was already saved rather than leaving a stale layout
      // and old terminal output on disk indefinitely.
      await Promise.all([
        api.workspaceSave(SESSION_ID, "Last session", "").catch(() => {}),
        api.scrollbackRetain([]).catch(() => {}),
      ]);
      return;
    }

    const snapshot = buildSnapshot(state, new Date().toISOString());
    try {
      await api.workspaceSave(SESSION_ID, "Last session", JSON.stringify(snapshot));
    } catch {
      // A failed save costs the next restore, not this session. Not worth a notice.
      return;
    }

    // Each pane's visible history, keyed by the id the snapshot recorded.
    await Promise.all(
      snapshot.panes.map(async (saved) => {
        const body = serialisePane(saved.id);
        if (!body) return;
        await api
          .scrollbackSave(saved.id, saved.program ?? null, saved.cwd || null, body)
          .catch(() => {});
      }),
    );
    // Panes that are gone should not keep their output. Done after the save so a pane
    // still in the snapshot is never caught by it.
    await api.scrollbackRetain(snapshot.panes.map((p) => p.id)).catch(() => {});
  },

  restoreSession: async (freshPane) => {
    if (!get().appearance.restoreSession) return false;
    if (get().tabs.length > 0) return false;

    let snapshot;
    try {
      snapshot = parseSnapshot(await api.workspaceLoad(SESSION_ID));
    } catch {
      return false;
    }
    if (!snapshot) return false;

    const created: Pane[] = [];
    const plan = planRestore(snapshot, (saved) => {
      const pane = freshPane(saved);
      created.push(pane);
      return pane.id;
    });
    // Nothing usable in the file, so the caller opens a fresh pane instead.
    if (plan.tabs.length === 0) return false;

    // `restoreKey` is what lets each pane find the output saved against the pane it
    // replaces; the ids themselves cannot be reused, since they name dead processes.
    const byNewId = new Map(plan.panes.map((p) => [p.newId, p]));
    const panes: Record<string, Pane> = {};
    for (const pane of created) {
      const planned = byNewId.get(pane.id);
      if (!planned) continue;
      panes[pane.id] = { ...pane, restoreKey: planned.restoreKey };
    }

    const tabs: Tab[] = plan.tabs.map((tab) => ({
      id: nextTabId(),
      title: tab.title,
      root: tab.root,
      activePaneId: tab.activePaneId,
      zoomedPaneId: null,
    }));

    const active = tabs[plan.activeTabIndex] ?? tabs[0];
    if (!active) return false;
    set({ tabs, panes, activeTabId: active.id });
    return true;
  },

  ensureFirstPane: (pane) =>
    set((s) => {
      if (s.tabs.length > 0 || Object.keys(s.panes).length > 0) return {};
      const id = nextTabId();
      return {
        tabs: [
          {
            id,
            title: pane.title,
            root: leaf(pane.id),
            activePaneId: pane.id,
            zoomedPaneId: null,
          },
        ],
        activeTabId: id,
        panes: { [pane.id]: pane },
      };
    }),

  addPane: (pane, tabId) =>
    set((s) => {
      const targetId = tabId ?? s.activeTabId;
      const tabs = [...s.tabs];
      let activeTabId = s.activeTabId;

      const index = targetId ? tabs.findIndex((t) => t.id === targetId) : -1;
      if (index < 0) {
        const id = nextTabId();
        tabs.push({
          id,
          title: pane.title,
          root: leaf(pane.id),
          activePaneId: pane.id,
          zoomedPaneId: null,
        });
        activeTabId = id;
      } else {
        const tab = tabs[index]!;
        tabs[index] = tab.root
          ? {
              ...tab,
              // A pane added without an explicit split lands beside the focused
              // one, which is what "+ Pane" means.
              root: splitPane(tab.root, tab.activePaneId ?? "", "row", pane.id),
              activePaneId: pane.id,
              zoomedPaneId: null,
            }
          : { ...tab, root: leaf(pane.id), activePaneId: pane.id };
      }

      return { tabs, activeTabId, panes: { ...s.panes, [pane.id]: pane } };
    }),

  removePane: (paneId) =>
    set((s) => {
      const panes = { ...s.panes };
      delete panes[paneId];

      const tabs = s.tabs
        .map((tab) => {
          if (!tab.root || !listPanes(tab.root).includes(paneId)) return tab;
          const root = closePaneNode(tab.root, paneId);
          const remaining = root ? listPanes(root) : [];
          return {
            ...tab,
            root,
            activePaneId:
              tab.activePaneId === paneId ? (remaining[0] ?? null) : tab.activePaneId,
            // A zoom on a closed pane has nothing left to show.
            zoomedPaneId: tab.zoomedPaneId === paneId ? null : tab.zoomedPaneId,
          };
        })
        // A tab with no panes is not a tab.
        .filter((tab) => tab.root !== null);

      return {
        panes,
        tabs,
        activeTabId: tabs.some((t) => t.id === s.activeTabId)
          ? s.activeTabId
          : (tabs[0]?.id ?? null),
      };
    }),

  setActivePane: (paneId) =>
    set((s) => ({
      tabs: s.tabs.map((tab) =>
        tab.root && listPanes(tab.root).includes(paneId)
          ? { ...tab, activePaneId: paneId }
          : tab,
      ),
      activeTabId:
        s.tabs.find((t) => t.root && listPanes(t.root).includes(paneId))?.id ??
        s.activeTabId,
    })),

  addTab: () => {
    const id = nextTabId();
    set((s) => ({
      tabs: [
        ...s.tabs,
        { id, title: "Shell", root: null, activePaneId: null, zoomedPaneId: null },
      ],
      activeTabId: id,
    }));
    return id;
  },

  setActiveTab: (activeTabId) => set({ activeTabId }),

  splitFocusedPane: (direction, newPane) =>
    set((s) => {
      const tab = s.tabs.find((t) => t.id === s.activeTabId);
      if (!tab?.root || !tab.activePaneId) {
        // Nothing to split against: this is the tab's first pane.
        return {
          tabs: s.tabs.map((t) =>
            t.id === s.activeTabId
              ? { ...t, root: leaf(newPane.id), activePaneId: newPane.id }
              : t,
          ),
          panes: { ...s.panes, [newPane.id]: newPane },
        };
      }
      return {
        tabs: s.tabs.map((t) =>
          t.id === tab.id
            ? {
                ...t,
                root: splitPane(t.root!, t.activePaneId!, direction, newPane.id),
                activePaneId: newPane.id,
                // Splitting while zoomed would hide the pane just created.
                zoomedPaneId: null,
              }
            : t,
        ),
        panes: { ...s.panes, [newPane.id]: newPane },
      };
    }),

  resizeFocusedSplit: (splitId, dividerIndex, delta) =>
    set((s) => ({
      tabs: s.tabs.map((tab) =>
        tab.id === s.activeTabId && tab.root
          ? { ...tab, root: resizeSplit(tab.root, splitId, dividerIndex, delta) }
          : tab,
      ),
    })),

  focusAdjacentPane: (forward) =>
    set((s) => {
      const tab = s.tabs.find((t) => t.id === s.activeTabId);
      if (!tab?.root || !tab.activePaneId) return {};
      const next = forward
        ? nextPaneOf(tab.root, tab.activePaneId)
        : prevPaneOf(tab.root, tab.activePaneId);
      if (!next) return {};
      return {
        tabs: s.tabs.map((t) => (t.id === tab.id ? { ...t, activePaneId: next } : t)),
      };
    }),

  swapFocusedPane: () =>
    set((s) => {
      const tab = s.tabs.find((t) => t.id === s.activeTabId);
      if (!tab?.root || !tab.activePaneId) return {};
      const next = nextPaneOf(tab.root, tab.activePaneId);
      if (!next || next === tab.activePaneId) return {};
      return {
        tabs: s.tabs.map((t) =>
          t.id === tab.id ? { ...t, root: swapPanes(t.root!, t.activePaneId!, next) } : t,
        ),
      };
    }),

  toggleZoom: () =>
    set((s) => ({
      tabs: s.tabs.map((tab) =>
        tab.id === s.activeTabId
          ? {
              ...tab,
              zoomedPaneId: tab.zoomedPaneId ? null : tab.activePaneId,
            }
          : tab,
      ),
    })),

  markPaneExited: (paneId, exitCode) =>
    set((s) => {
      const pane = s.panes[paneId];
      if (!pane) return {};
      return { panes: { ...s.panes, [paneId]: { ...pane, exited: true, exitCode } } };
    }),

  setPaneCwd: (paneId, cwd) =>
    set((s) => {
      const pane = s.panes[paneId];
      // Unchanged is the common case — a prompt redraw reports the same directory — and
      // returning a new object each time would rerender every pane.
      if (!pane || pane.cwd === cwd) return {};
      return { panes: { ...s.panes, [paneId]: { ...pane, cwd } } };
    }),

  // ------------------------------------------------------------- appearance

  setAppearance: (patch) => {
    const appearance = { ...get().appearance, ...patch };
    set({ appearance });
    // Persisted best-effort: a settings write must never block the UI, and a
    // failure to save a font size is not worth an error dialog.
    void api.settingsSet(APPEARANCE_KEY, JSON.stringify(appearance)).catch(() => {});
    reportColorScheme(appearance.themeId);
  },

  loadAppearance: async () => {
    try {
      const raw = await api.settingsGet(APPEARANCE_KEY);
      if (!raw) return;
      const parsed = JSON.parse(raw) as Partial<Appearance>;
      const appearance = { ...DEFAULT_APPEARANCE, ...parsed };
      set({ appearance });
      // Told at startup as well as on change: a program that asks before the first theme
      // change would otherwise be answered with the default rather than the real theme.
      reportColorScheme(appearance.themeId);

      const width = await api.settingsGet("listColumnWidth");
      const parsedWidth = width ? Number(width) : NaN;
      if (Number.isFinite(parsedWidth)) {
        set({ listColumnWidth: Math.max(220, Math.min(parsedWidth, 640)) });
      }
    } catch {
      // Corrupt settings fall back to defaults rather than failing startup.
    }
  },

  // -------------------------------------------------------------- refreshes

  refreshEnvironment: async () => {
    try {
      const environment = await api.environment();
      set((s) => ({
        environment,
        notices: [...s.notices, ...environment.notices.filter((n) => !s.notices.includes(n))],
      }));
    } catch (e) {
      get().pushNotice(describeError(e));
    }
  },

  refreshGit: async () => {
    try {
      set({ gitStatus: await api.gitStatus() });
    } catch {
      // Not every project is a repository; that is not an error worth showing.
      set({ gitStatus: null });
    }
  },

  refreshBlocks: async (filter) => {
    const blockFilter = filter ?? get().blockFilter;
    try {
      set({ blocks: await api.blocksQuery(blockFilter), blockFilter });
    } catch (e) {
      get().pushNotice(describeError(e));
    }
  },

  refreshAgents: async () => {
    try {
      const agents = await api.agentsOverview();
      set((s) => ({
        agents,
        activeProfileId:
          s.activeProfileId ?? agents.default_profile ?? agents.profiles[0]?.id ?? null,
      }));
    } catch (e) {
      get().pushNotice(describeError(e));
    }
  },

  refreshApprovals: async () => {
    try {
      set({ pendingApprovals: await api.rulesPending() });
    } catch {
      // Non-fatal.
    }
  },

  setActiveProfile: (activeProfileId) => set({ activeProfileId }),

  // ---------------------------------------------------------------- threads

  upsertThread: (thread) =>
    set((s) => ({
      threads: { ...s.threads, [thread.id]: { ...s.threads[thread.id], ...thread } },
      activeThreadId: s.activeThreadId ?? thread.id,
    })),

  observeThread: (thread) =>
    set((s) => {
      const existing = s.threads[thread.id];
      return {
        threads: {
          ...s.threads,
          [thread.id]: {
            id: thread.id,
            profileId: existing?.profileId ?? "",
            runtimeId: thread.agent.runtime_id,
            title: thread.task_title,
            state: thread.state,
            events: existing?.events ?? [],
            // Null on purpose: there is no session to ask, and inventing them would
            // put controls on screen that cannot work.
            capabilities: null,
            permissions: null,
            info: existing?.info ?? null,
            paneId: thread.pane_id,
          },
        },
        // Not selected automatically. An agent starting in a pane must not yank the
        // surface out from under someone mid-task.
        activeThreadId: s.activeThreadId,
      };
    }),

  appendThreadEvent: (event) =>
    set((s) => {
      if (!event.thread_id) return {};
      const existing = s.threads[event.thread_id];
      if (!existing) return {};
      return {
        threads: {
          ...s.threads,
          [event.thread_id]: { ...existing, events: [...existing.events, event] },
        },
      };
    }),

  setThreadState: (threadId, state) =>
    set((s) => {
      const existing = s.threads[threadId];
      if (!existing) return {};
      return { threads: { ...s.threads, [threadId]: { ...existing, state } } };
    }),

  setActiveThread: (activeThreadId) => set({ activeThreadId }),

  openStoredThread: async (threadId) => {
    const existing = get().threads[threadId];
    if (existing && existing.events.length > 0) {
      set({ activeThreadId: threadId });
      return;
    }

    const events = await api.threadEvents(threadId, 5000);
    // The title comes from the first thing recorded, which is what the Thread was for.
    // The payload type is `Record<string, unknown>`, so every consumer narrows by hand.
    // Cast rather than assert: a stored event from an older version may not have the
    // field, and a missing title is better than a crash reading history.
    const prompt = events.find((e) => e.payload.type === "user.prompted");
    const promptText =
      typeof prompt?.payload.text === "string" ? prompt.payload.text : null;
    const title = promptText ? promptText.slice(0, 80) : "Thread";
    const started = events.find((e) => e.payload.type === "thread.started");

    // Capabilities and permissions are left null on purpose: this Thread is not running,
    // so there is no session to report them, and inventing them would put controls on
    // screen that cannot work.
    set((s) => ({
      activeThreadId: threadId,
      threads: {
        ...s.threads,
        [threadId]: {
          id: threadId,
          profileId: existing?.profileId ?? "",
          runtimeId: started?.agent.runtime_id ?? existing?.runtimeId ?? "",
          title: existing?.title ?? title,
          state: lastThreadState(events) ?? "completed",
          events,
          capabilities: existing?.capabilities ?? null,
          permissions: existing?.permissions ?? null,
          info: existing?.info ?? null,
          // Recorded on the thread.started event, so a session reached from prompt
          // history long after it ended is still known to be a pane session — and
          // still does not get a composer.
          paneId:
            existing?.paneId ??
            (started?.links.find((l) => "pane_id" in l) as { pane_id?: string } | undefined)
              ?.pane_id ??
            null,
        },
      },
    }));
  },

  refreshThreadInfo: async (threadId) => {
    try {
      const info = await api.threadInfo(threadId);
      if (!info) return;
      set((s) => {
        const existing = s.threads[threadId];
        if (!existing) return {};
        return {
          threads: {
            ...s.threads,
            [threadId]: {
              ...existing,
              info,
              capabilities: info.capabilities,
              permissions: info.permissions,
            },
          },
        };
      });
    } catch {
      // The Thread may have ended between render and fetch.
    }
  },

  // --------------------------------------------------------------- notices

  stageAttachment: (attachment) =>
    set((s) => ({ stagedAttachments: [...s.stagedAttachments, attachment] })),

  clearAttachments: () => set({ stagedAttachments: [] }),

  refreshConnections: async () => {
    try {
      set({ connections: await api.connections() });
    } catch (e) {
      get().pushNotice(describeError(e));
    }
  },

  setInspectorTab: (tab) =>
    set({
      surface:
        tab === "review" || tab === "diagnostics"
          ? "review"
          : tab === "thread"
            ? "agents"
            : "terminal",
    }),

  pushNotice: (message) =>
    set((s) => (s.notices.includes(message) ? {} : { notices: [...s.notices, message] })),

  dismissNotice: (index) =>
    set((s) => ({ notices: s.notices.filter((_, i) => i !== index) })),
}));

/** Render an IPC error as one readable sentence. */
/**
 * The last state a stored Thread reached.
 *
 * Read from the event stream rather than assumed, because a Thread that was interrupted
 * or disconnected must not be shown as completed — the whole point of an append-only
 * record is that it says what actually happened.
 */
function lastThreadState(events: api.TervinEvent[]): api.ThreadState | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const payload = events[i]!.payload;
    if (payload.type === "thread.state") {
      return typeof payload.state === "string" ? (payload.state as api.ThreadState) : null;
    }
    if (payload.type === "thread.completed") return "completed";
    if (payload.type === "thread.failed") return "failed";
  }
  return null;
}

export function describeError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return String(error);
}

/** Panes belonging to a tab, in focus order. */
export function panesOfTab(state: WorkspaceState, tabId: string | null): Pane[] {
  if (!tabId) return [];
  const tab = state.tabs.find((t) => t.id === tabId);
  if (!tab?.root) return [];
  return listPanes(tab.root)
    .map((id) => state.panes[id])
    .filter((p): p is Pane => Boolean(p));
}

/** Threads that are actively working, which is what the Deck counts. */
export function activeThreadCount(state: WorkspaceState): number {
  const working: api.ThreadState[] = [
    "starting",
    "understanding",
    "planning",
    "reading",
    "editing",
    "executing",
    "testing",
    "waiting_for_external_tool",
  ];
  return Object.values(state.threads).filter((t) => working.includes(t.state)).length;
}

/** Threads blocked on the user, which the top bar surfaces. */
export function threadsNeedingUser(state: WorkspaceState): ThreadView[] {
  const needs: api.ThreadState[] = [
    "awaiting_input",
    "waiting_for_permission",
    "review_required",
  ];
  return Object.values(state.threads).filter((t) => needs.includes(t.state));
}
