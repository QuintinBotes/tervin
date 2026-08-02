/**
 * The workspace shell.
 *
 * Structured as **surfaces**, not as a permanent multi-column layout: the top bar
 * switches between Terminal, Plan, Agents and Review, and exactly one is on screen
 * at a time. Each surface then uses at most two columns internally.
 *
 * That is the design system's central layout rule — two columns maximum, a third
 * pane means something collapses — and it is a real constraint rather than a
 * stylistic one. A rail plus a terminal plus an inspector is three columns, which
 * on a laptop leaves the terminal too narrow to be the thing you work in.
 *
 * Geometry here is not approximate: top bar 42, tab strip 29, status rail 25,
 * controls 26/28, one shadow in the entire product. See docs/DESIGN.md.
 *
 * ## Keys the terminal owns
 *
 * A binding Tervin does not implement must reach the shell. `runAction` returns
 * whether it handled the action, and only a handled action calls
 * `preventDefault` — so Ctrl-A, Ctrl-E, Ctrl-K, Ctrl-W, Escape and Tab keep
 * working inside vim, emacs and readline, which is the difference between a
 * terminal and a terminal-shaped widget.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as api from "./lib/api";
import {
  activeThreadCount,
  overlayOpen,
  threadsNeedingUser,
  useWorkspace,
  type Surface,
} from "./lib/store";
import { applyTheme, findTheme } from "./design/themes";
import { listPanes, serialise as serialisePaneTree } from "./lib/panes";
import { Keymap, formatChord } from "./lib/keymap";
import {
  clearPane,
  scrollPane,
  selectAllInPane,
  serialisePane,
} from "./components/TerminalPane";
import { PaneTree } from "./components/PaneTree";
import { SearchOverlay } from "./components/SearchOverlay";
import { Mark } from "./components/Mark";
import { ThreadPanel } from "./components/ThreadPanel";
import { ReviewPanel } from "./components/ReviewPanel";
import { ApprovalSheet } from "./components/ApprovalSheet";
import { CommandPalette } from "./components/CommandPalette";
import { SettingsPanel } from "./components/SettingsPanel";
import { AgentDeck } from "./components/AgentDeck";
import { PlanSurface } from "./components/PlanSurface";
import { HistorySurface } from "./components/HistorySurface";
import { FileExplorer } from "./components/FileExplorer";
import { ConnectionsPanel } from "./components/ConnectionsPanel";
import { SavedCommands } from "./components/SavedCommands";
import { GitPanel } from "./components/GitPanel";

/** The surfaces, in the order the switcher shows them. */
const SURFACES: { id: Surface; label: string }[] = [
  { id: "terminal", label: "Terminal" },
  { id: "plan", label: "Plan" },
  { id: "agents", label: "Agents" },
  { id: "review", label: "Review" },
  // Last because it is the one you go to deliberately rather than watch.
  { id: "history", label: "History" },
];

/**
 * Below this width the supporting column hides entirely.
 *
 * Collapse, never squeeze: a two-column compromise at 700px gives two unusable
 * columns instead of one good one.
 */
const NARROW = 860;

export default function App() {
  const s = useWorkspace();
  const bootstrapped = useRef(false);
  const [narrow, setNarrow] = useState(() => window.innerWidth < NARROW);

  // Theme as CSS variables: one style recalculation, no reflow, no flash.
  useEffect(() => {
    applyTheme(findTheme(s.appearance.themeId));
    const root = document.documentElement;
    root.style.setProperty("--font-mono", s.appearance.fontFamily);
    root.style.setProperty("--font-mono-size", `${s.appearance.fontSize}px`);
    root.style.setProperty("--font-mono-line-height", String(s.appearance.lineHeight));
    root.style.setProperty(
      "--mono-ligatures",
      s.appearance.ligatures ? "common-ligatures" : "none",
    );
  }, [s.appearance]);

  useEffect(() => {
    const onResize = () => setNarrow(window.innerWidth < NARROW);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // Startup. Guarded by a ref, not state: StrictMode runs effect → cleanup →
  // effect within one commit, so a state flag still reads its old value.
  useEffect(() => {
    if (bootstrapped.current) return;
    bootstrapped.current = true;

    void (async () => {
      await s.loadAppearance();
      await Promise.all([
        s.refreshEnvironment(),
        s.refreshGit(),
        s.refreshBlocks(),
        s.refreshAgents(),
      ]);
      // Last session first, falling back to one fresh pane. Both are idempotent in
      // the store, so a remount or a double-invoked effect cannot open a second pane.
      const restored = await s.restoreSession((saved) => ({
        ...makePane(saved.cwd || undefined),
        title: saved.title,
        program: saved.program,
        args: saved.args,
        remote: saved.remote,
      }));
      if (!restored) s.ensureFirstPane(makePane());
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Backend events, subscribed once; handlers read fresh state.
  useEffect(() => {
    const store = useWorkspace.getState;
    const subs = [
      api.on<api.TervinEvent>("thread://event", (event) => {
        store().appendThreadEvent(event);
        if (event.payload.type === "permission.requested") {
          void store().refreshApprovals();
        }
      }),
      api.on<{ threadId: string; state: api.ThreadState }>("thread://state", (p) =>
        store().setThreadState(p.threadId, p.state),
      ),
      // An agent someone started in a pane. Arrives before that Thread's events,
      // which are dropped for a Thread the UI has not been told about.
      api.on<api.ObservedThread>("thread://observed", (thread) => store().observeThread(thread)),
      api.on<{ title: string; body: string }>("pane://notification", (p) => {
        // Shown in Tervin's own notice rail rather than raised as a system
        // notification: a process asking for one is not the same as the person
        // wanting one, and the request can arrive from a remote host.
        store().pushNotice(
          [p.title, p.body].filter(Boolean).join(" — ") || "A program sent a notification.",
        );
      }),
      api.on<unknown>("block://finished", () => {
        void store().refreshBlocks();
        void store().refreshGit();
      }),
      api.on<{ selection: string; text: string }>("clipboard://requested", (p) => {
        store().pushNotice(
          `A program asked to write ${p.text.length} characters to the clipboard. Tervin did not allow it automatically.`,
        );
      }),
    ];
    return () => subs.forEach((sub) => void sub.then((un) => un()));
  }, []);

  // The layout is saved whenever it changes, and once more as the window closes.
  //
  // Debounced rather than written per change: dragging a divider rewrites the tree
  // continuously, and a database write per frame would be felt. Only the structure is
  // watched, so ordinary terminal output never triggers a save.
  const layoutKey = useWorkspace((st) =>
    JSON.stringify([
      st.activeTabId,
      st.tabs.map((t) => [t.id, t.title, t.root && serialisePaneTree(t.root), t.activePaneId]),
      Object.values(st.panes).map((p) => [p.id, p.cwd, p.program]),
    ]),
  );
  const restoreEnabled = useWorkspace((st) => st.appearance.restoreSession);

  useEffect(() => {
    const handle = setTimeout(() => {
      void useWorkspace.getState().saveSession(serialisePane);
    }, 1200);
    return () => clearTimeout(handle);
  }, [layoutKey, restoreEnabled]);

  useEffect(() => {
    // The debounce above can still be pending when the window goes away, so the last
    // change is flushed here too. `pagehide` fires for a closing window where
    // `beforeunload` is unreliable inside a webview.
    const flush = () => void useWorkspace.getState().saveSession(serialisePane);
    window.addEventListener("pagehide", flush);
    window.addEventListener("beforeunload", flush);
    return () => {
      window.removeEventListener("pagehide", flush);
      window.removeEventListener("beforeunload", flush);
    };
  }, []);

  const keymap = useMemo(() => new Keymap(), []);

  const onKeyDown = useCallback(
    (e: KeyboardEvent) => {
      const store = useWorkspace.getState();
      const target = e.target as HTMLElement | null;
      const inTerminal = Boolean(target?.closest(".terminal-surface"));

      const context = overlayOpen(store)
        ? "overlay"
        : inTerminal
          ? "terminal"
          : target?.tagName === "TEXTAREA" || target?.tagName === "INPUT"
            ? "composer"
            : "global";

      const action = keymap.resolve(e, context);
      if (!action) return;

      // Copy and paste stay with the browser: intercepting ⌘C would stop it
      // sending SIGINT when there is no selection.
      if (action === "terminal.copy" && !window.getSelection()?.toString()) return;
      if (action === "terminal.paste") return;

      const tab = store.tabs.find((t) => t.id === store.activeTabId);
      // Only a handled action consumes the key. Anything else falls through to
      // the program in the pane, which is what keeps vim and emacs usable.
      if (runAction(action, tab?.activePaneId ?? null)) {
        e.preventDefault();
        e.stopPropagation();
      }
    },
    [keymap],
  );

  useEffect(() => {
    window.addEventListener("keydown", onKeyDown, { capture: true });
    return () => window.removeEventListener("keydown", onKeyDown, { capture: true });
  }, [onKeyDown]);

  const waiting = threadsNeedingUser(s);

  return (
    <div className="col" style={{ height: "100%", background: "var(--tervin-bg)" }}>
      <TopBar keymap={keymap} waitingCount={waiting.length} />
      {s.notices.length > 0 && <Notices />}

      <div className="grow" style={{ display: "flex", minHeight: 0, minWidth: 0 }}>
        {s.surface === "terminal" && <TerminalSurface />}
        {s.surface === "plan" && <PlanSurface narrow={narrow} />}
        {s.surface === "agents" && <AgentsSurface narrow={narrow} />}
        {s.surface === "review" && <ReviewSurface narrow={narrow} />}
        {s.surface === "history" && <HistorySurface narrow={narrow} />}
      </div>

      <StatusRail />

      {s.paletteOpen && (
        <CommandPalette onNewPane={() => useWorkspace.getState().addPane(makePane())} />
      )}
      {s.settingsOpen && <SettingsPanel />}
      {s.connectionsOpen && <ConnectionsOverlay />}
      {s.savedCommandsOpen && <SavedCommands />}
      {s.pendingApprovals.length > 0 && <ApprovalSheet />}
    </div>
  );
}

// ----------------------------------------------------------------- surfaces

/** Terminal: tab strip plus the pane tree. One column, full width. */
function TerminalSurface() {
  const s = useWorkspace();
  const activeTab = s.tabs.find((t) => t.id === s.activeTabId);
  const { tabBarPosition, explorerVisible, explorerSide } = s.appearance;
  const tabsVertical = tabBarPosition === "left" || tabBarPosition === "right";

  const panes = (
    <div
      className="grow"
      style={{
        display: "flex",
        // Stretch, not centre: centring leaves the divider colour showing as bands
        // above and below each pane.
        alignItems: "stretch",
        minHeight: 0,
        minWidth: 0,
        background: "var(--tervin-line)",
        position: "relative",
      }}
    >
      {s.searchOpen && <SearchOverlay paneId={activeTab?.activePaneId ?? null} />}
      {!activeTab?.root ? (
        <div className="empty">
          No panes open. <kbd>{formatChord("mod+t")}</kbd> opens a shell.
        </div>
      ) : (
        <PaneTree
          node={activeTab.root}
          activePaneId={activeTab.activePaneId}
          zoomedPaneId={activeTab.zoomedPaneId}
          onFocus={s.setActivePane}
          onResize={s.resizeFocusedSplit}
        />
      )}
    </div>
  );

  // One layout for all four tab positions: the strip and the panes go in a row or a
  // column, in one order or the other. A branch per side would drift out of step.
  const withTabs = (
    <div
      className={tabsVertical ? "row grow" : "col grow"}
      style={{ minWidth: 0, minHeight: 0, alignItems: "stretch" }}
    >
      {tabBarPosition === "top" || tabBarPosition === "left" ? (
        <>
          <TabStrip />
          {panes}
        </>
      ) : (
        <>
          {panes}
          <TabStrip />
        </>
      )}
    </div>
  );

  if (!explorerVisible) return withTabs;

  return (
    <div className="row grow" style={{ minWidth: 0, minHeight: 0, alignItems: "stretch" }}>
      {explorerSide === "left" ? (
        <>
          <ExplorerColumn />
          {withTabs}
        </>
      ) : (
        <>
          {withTabs}
          <ExplorerColumn />
        </>
      )}
    </div>
  );
}

/**
 * The file explorer's column, with a draggable edge.
 *
 * Width is persisted with the same value the two-column surfaces use, so the layout
 * does not jump when switching between them.
 */
function ExplorerColumn() {
  const s = useWorkspace();
  const width = Math.max(180, Math.min(s.listColumnWidth, 520));
  const onLeft = s.appearance.explorerSide === "left";

  const handle = (
    <ColumnHandle onDrag={(delta) => s.setListColumnWidth(width + (onLeft ? delta : -delta))} />
  );

  return (
    <>
      {!onLeft && handle}
      <div
        style={{
          width,
          flex: "none",
          minHeight: 0,
          display: "flex",
          borderRight: onLeft ? "1px solid var(--tervin-line)" : undefined,
          borderLeft: onLeft ? undefined : "1px solid var(--tervin-line)",
        }}
      >
        <FileExplorer />
      </div>
      {onLeft && handle}
    </>
  );
}

/**
 * Review: what the repository looks like, then what changed.
 *
 * `GitPanel` reports branch, upstream divergence, and any operation in progress — a
 * rebase or merge changes what a commit means, so it belongs beside the diff rather than
 * behind it. It had been written and left unreachable, which a reachability test caught.
 */
function ReviewSurface({ narrow }: { narrow: boolean }) {
  const s = useWorkspace();
  return (
    <TwoColumn
      narrow={narrow}
      listLabel="Repository"
      leftWidth={s.listColumnWidth}
      onResize={s.setListColumnWidth}
      left={
        <div className="col" style={{ minHeight: 0, width: "100%" }}>
          <div className="panel-header">
            <span className="label">Repository</span>
          </div>
          <div className="grow" style={{ overflow: "auto", minHeight: 0 }}>
            <GitPanel />
          </div>
        </div>
      }
      right={<ReviewPanel />}
    />
  );
}

/**
 * Connections as an overlay rather than a surface.
 *
 * Opening a connection is a one-shot action — pick a host, get a pane — not somewhere you
 * stay. A sixth tab for it would spend permanent space on something used occasionally.
 */
function ConnectionsOverlay() {
  const s = useWorkspace();
  const ref = useRef<HTMLDivElement | null>(null);

  // Takes the keyboard, so nothing typed here reaches the shell underneath.
  useEffect(() => {
    ref.current?.focus();
  }, []);

  return (
    <div
      ref={ref}
      role="dialog"
      aria-modal="true"
      aria-label="Connections"
      tabIndex={-1}
      onClick={() => s.setConnections(false)}
      style={{
        position: "fixed",
        inset: 0,
        background: "color-mix(in srgb, var(--tervin-bg) 70%, transparent)",
        display: "grid",
        placeItems: "center",
        padding: "var(--sp-6)",
        zIndex: 150,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="col"
        style={{
          width: "min(760px, 96vw)",
          height: "min(600px, 88vh)",
          background: "var(--tervin-panel)",
          border: "1px solid var(--tervin-line)",
          borderRadius: "var(--radius-lg)",
          overflow: "hidden",
        }}
      >
        <div className="grow" style={{ minHeight: 0, overflow: "auto" }}>
          <ConnectionsPanel />
        </div>
        <div
          className="row"
          style={{
            flex: "none",
            padding: "var(--sp-2) var(--sp-3)",
            borderTop: "1px solid var(--tervin-line)",
          }}
        >
          <div className="grow" />
          <button className="btn" onClick={() => s.setConnections(false)}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}

/** Agents: Threads list, then the selected Thread. Two columns. */
function AgentsSurface({ narrow }: { narrow: boolean }) {
  const s = useWorkspace();

  return (
    <TwoColumn
      narrow={narrow}
      leftWidth={s.listColumnWidth}
      onResize={s.setListColumnWidth}
      listLabel="Threads"
      left={
        <div className="col" style={{ minHeight: 0, width: "100%" }}>
          <div className="panel-header">
            <span className="label">Threads</span>
            <span className="meta truncate">Waiting first, then running, then done</span>
          </div>
          <div className="grow" style={{ overflow: "auto", minHeight: 0 }}>
            <AgentDeck />
          </div>
        </div>
      }
      right={<ThreadPanel />}
    />
  );
}

/**
 * Two columns with a draggable divider, collapsing to one when narrow.
 *
 * The supporting column hides entirely below the threshold and returns via a
 * button, rather than shrinking — squeezing produces two columns that are both
 * too small to use.
 */
export function TwoColumn({
  narrow,
  left,
  right,
  leftWidth,
  onResize,
  listLabel,
}: {
  narrow: boolean;
  left: React.ReactNode;
  right: React.ReactNode;
  leftWidth: number;
  onResize: (width: number) => void;
  listLabel: string;
}) {
  const [showLeft, setShowLeft] = useState(false);

  if (narrow) {
    return (
      <div className="col grow" style={{ minWidth: 0, minHeight: 0 }}>
        <div
          className="row"
          style={{
            padding: "var(--sp-2) var(--sp-6)",
            flex: "none",
            borderBottom: "1px solid var(--tervin-raised)",
          }}
        >
          <button className="btn btn-sm" onClick={() => setShowLeft((v) => !v)}>
            {showLeft ? "Back" : listLabel}
          </button>
        </div>
        <div className="grow" style={{ minHeight: 0, display: "flex", minWidth: 0 }}>
          {showLeft ? left : right}
        </div>
      </div>
    );
  }

  return (
    <div
      className="grow"
      style={{ display: "flex", alignItems: "stretch", minWidth: 0, minHeight: 0 }}
    >
      <div
        style={{
          width: leftWidth,
          minWidth: 220,
          maxWidth: "45%",
          flex: "none",
          minHeight: 0,
          display: "flex",
        }}
      >
        {left}
      </div>
      <ColumnHandle onDrag={(delta) => onResize(leftWidth + delta)} />
      <div className="grow" style={{ minWidth: 0, minHeight: 0, display: "flex" }}>
        {right}
      </div>
    </div>
  );
}

/**
 * A 5px vertical drag handle.
 *
 * Pointer capture is on `window` so a fast drag cannot outrun the element and
 * leave the divider stuck to the cursor.
 */
function ColumnHandle({ onDrag }: { onDrag: (delta: number) => void }) {
  const ref = useRef<HTMLDivElement | null>(null);

  return (
    <div
      ref={ref}
      className="handle-col"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize columns"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "ArrowLeft") onDrag(-16);
        else if (e.key === "ArrowRight") onDrag(16);
        else return;
        e.preventDefault();
      }}
      onPointerDown={(event) => {
        event.preventDefault();
        ref.current?.setAttribute("data-dragging", "true");
        let last = event.clientX;
        const move = (e: PointerEvent) => {
          const delta = e.clientX - last;
          last = e.clientX;
          if (delta !== 0) onDrag(delta);
        };
        const up = () => {
          ref.current?.removeAttribute("data-dragging");
          window.removeEventListener("pointermove", move);
          window.removeEventListener("pointerup", up);
          document.body.style.cursor = "";
        };
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", up);
        document.body.style.cursor = "col-resize";
      }}
    />
  );
}

// -------------------------------------------------------------------- chrome

function TopBar({ keymap, waitingCount }: { keymap: Keymap; waitingCount: number }) {
  const s = useWorkspace();
  const [profileOpen, setProfileOpen] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const active = activeThreadCount(s);
  const git = s.gitStatus;
  const profile = s.agents?.profiles.find((p) => p.id === s.activeProfileId);
  const changed = git?.files.filter((f) => f.stage !== "untracked").length ?? 0;

  // Any click outside closes an open menu.
  useEffect(() => {
    if (!profileOpen && !moreOpen) return;
    const close = () => {
      setProfileOpen(false);
      setMoreOpen(false);
    };
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [profileOpen, moreOpen]);

  return (
    <header
      className="row"
      data-tauri-drag-region
      style={{
        height: "var(--topbar-h)",
        flex: "none",
        // Room for the traffic lights under an overlay title bar.
        padding: "0 10px 0 84px",
        borderBottom: "1px solid var(--tervin-raised)",
        gap: "var(--sp-3)",
      }}
    >
      <Mark size={16} />

      <button
        className="btn btn-sm"
        onClick={() => s.setSettings(true)}
        title={s.environment?.project_root ?? "Choose a project"}
        style={{ borderColor: "var(--tervin-raised)" }}
      >
        <span className="truncate" style={{ maxWidth: 170 }}>
          {shortenPath(s.environment?.project_root ?? "—")}
        </span>
      </button>

      {git && (
        <span className="chip" title={git.operation_in_progress ?? "Git"}>
          <span className={`dot ${git.dirty ? "dot-amber" : "dot-muted"}`} />
          {git.detached ? "detached" : (git.branch ?? "no branch")}
          {changed > 0 && ` · ${changed} changed`}
        </span>
      )}

      {/* Surface switcher: 26px pills, the active one filled. */}
      <div className="row" style={{ gap: 2, marginLeft: "var(--sp-2)" }}>
        {SURFACES.map((surface) => (
          <button
            key={surface.id}
            className="segment"
            aria-selected={s.surface === surface.id}
            onClick={() => s.setSurface(surface.id)}
          >
            {surface.label}
            {surface.id === "agents" && active > 0 && (
              <span className="mono dim tabular" style={{ fontSize: "var(--text-tag)" }}>
                {active}
              </span>
            )}
            {surface.id === "review" && changed > 0 && (
              <span className="mono dim tabular" style={{ fontSize: "var(--text-tag)" }}>
                {changed}
              </span>
            )}
          </button>
        ))}
      </div>

      <div className="grow" />

      {waitingCount > 0 && (
        <button
          className="chip chip-amber"
          onClick={() => s.setSurface("agents")}
          title="An agent is blocked on you"
        >
          <span className="dot dot-amber" />
          {waitingCount} waiting on you
        </button>
      )}

      {/* Agent profile: how multiple accounts and installs are switched. */}
      <div style={{ position: "relative" }} onClick={(e) => e.stopPropagation()}>
        <button
          className="btn btn-sm"
          onClick={() => setProfileOpen((v) => !v)}
          title={`Agent profile · ${formatChord("mod+shift+p")}`}
          style={{ borderColor: "var(--tervin-raised)" }}
        >
          <span className={`dot ${profile?.sensitive ? "dot-amber" : "dot-teal"}`} />
          <span className="truncate" style={{ maxWidth: 130 }}>
            {profile?.name ?? "No agent"}
          </span>
          <span className="dim">▾</span>
        </button>

        {profileOpen && (
          <div
            className="overlay-surface col"
            style={{
              position: "absolute",
              top: "calc(100% + 4px)",
              right: 0,
              width: 300,
              zIndex: 200,
            }}
          >
            <div
              className="row"
              style={{
                padding: "var(--sp-2) var(--sp-6)",
                borderBottom: "1px solid var(--tervin-raised)",
              }}
            >
              <span className="label grow">Agent profile</span>
              <kbd>{formatChord("mod+shift+p")}</kbd>
            </div>
            {(s.agents?.profiles ?? []).map((p) => (
              <button
                key={p.id}
                className="list-row"
                aria-selected={p.id === s.activeProfileId}
                onClick={() => {
                  s.setActiveProfile(p.id);
                  setProfileOpen(false);
                }}
              >
                <span className={`dot ${p.sensitive ? "dot-amber" : "dot-teal"}`} />
                <span className="col grow" style={{ gap: 2 }}>
                  <span className="truncate" style={{ fontSize: "var(--text-control)" }}>
                    {p.name}
                  </span>
                  <span className="mono meta truncate">
                    {Object.entries(p.env)
                      .map(([k, v]) => `${k}=${v}`)
                      .join(" ") || p.binary}
                  </span>
                </span>
                {p.id === s.activeProfileId && <span className="tone-teal">✓</span>}
              </button>
            ))}
            <div className="row" style={{ padding: "var(--sp-2) var(--sp-6)" }}>
              <span className="meta grow">New Threads use this profile</span>
              <button
                className="btn btn-xs"
                onClick={() => {
                  setProfileOpen(false);
                  s.setSettings(true);
                }}
              >
                Manage
              </button>
            </div>
          </div>
        )}
      </div>

      <button className="btn btn-sm" onClick={() => s.setPalette(true)}>
        Search
        <kbd>{keymap.keysFor("palette.open") ?? ""}</kbd>
      </button>

      {/* Overflow, so the bar never clips its own controls. */}
      <div style={{ position: "relative" }} onClick={(e) => e.stopPropagation()}>
        <button
          className="btn btn-sm btn-ghost"
          onClick={() => setMoreOpen((v) => !v)}
          title="More"
        >
          ⋯
        </button>
        {moreOpen && (
          <div
            className="overlay-surface col"
            style={{
              position: "absolute",
              top: "calc(100% + 4px)",
              right: 0,
              width: 210,
              zIndex: 200,
            }}
          >
            {(
              [
                ["Settings", () => s.setSettings(true)],
                ["Command palette", () => s.setPalette(true)],
                ["Find in terminal", () => { s.setSurface("terminal"); s.setSearch(true); }],
              ] as [string, () => void][]
            ).map(([label, run]) => (
              <button
                key={label}
                className="list-row"
                onClick={() => {
                  run();
                  setMoreOpen(false);
                }}
              >
                <span style={{ fontSize: "var(--text-control)" }}>{label}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    </header>
  );
}

function TabStrip() {
  const s = useWorkspace();
  if (s.tabs.length === 0) return null;

  const position = s.appearance.tabBarPosition;
  const vertical = position === "left" || position === "right";
  const active = s.tabs.find((t) => t.id === s.activeTabId);
  const zoomed = Boolean(active?.zoomedPaneId);

  /** What `+` does, which is a preference rather than a decision. */
  function addNew() {
    const store = useWorkspace.getState();
    if (store.appearance.newButtonAction === "pane") {
      store.addPane(makePane());
      return;
    }
    // A tab, and a pane inside it — a tab with no pane is not a usable thing.
    store.addTab();
    store.addPane(makePane());
  }

  return (
    <div
      className={vertical ? "col" : "row"}
      role="tablist"
      aria-orientation={vertical ? "vertical" : "horizontal"}
      style={
        vertical
          ? {
              // Wide enough for a truncated title. A vertical strip is the only
              // arrangement that stays readable with twenty tabs open.
              width: 168,
              flex: "none",
              minHeight: 0,
              overflow: "auto",
              padding: "var(--sp-2) 0",
              gap: 1,
              borderRight:
                position === "left" ? "1px solid var(--tervin-raised)" : undefined,
              borderLeft:
                position === "right" ? "1px solid var(--tervin-raised)" : undefined,
            }
          : {
              height: "var(--tabstrip-h)",
              flex: "none",
              borderBottom:
                position === "top" ? "1px solid var(--tervin-raised)" : undefined,
              borderTop:
                position === "bottom" ? "1px solid var(--tervin-raised)" : undefined,
              padding: "0 var(--sp-3)",
              gap: 0,
            }
      }
    >
      {s.tabs.map((tab) => {
        const selected = tab.id === s.activeTabId;
        const panes = tab.root ? listPanes(tab.root) : [];
        const anyExited = panes.some((id) => s.panes[id]?.exited);
        return (
          <button
            key={tab.id}
            role="tab"
            aria-selected={selected}
            onClick={() => s.setActiveTab(tab.id)}
            className="row"
            style={{
              height: "var(--tabstrip-h)",
              padding: vertical ? "0 var(--sp-3)" : "0 11px",
              gap: 7,
              flex: "none",
              // Vertical tabs mark the active one on the edge nearest the panes;
              // horizontal ones underline. Never a fill and a border at once.
              borderBottom: vertical
                ? undefined
                : selected
                  ? "1px solid var(--tervin-accent)"
                  : "1px solid transparent",
              borderLeft:
                vertical && position === "left"
                  ? `2px solid ${selected ? "var(--tervin-accent)" : "transparent"}`
                  : undefined,
              borderRight:
                vertical && position === "right"
                  ? `2px solid ${selected ? "var(--tervin-accent)" : "transparent"}`
                  : undefined,
              background: selected ? "var(--tervin-block-hover)" : "transparent",
              color: selected ? "var(--tervin-ink)" : "var(--tervin-muted)",
              fontFamily: "var(--font-mono)",
              fontSize: "var(--text-meta)",
              justifyContent: "flex-start",
            }}
          >
            <span
              className={`dot ${anyExited ? "dot-muted" : "dot-green"}`}
              style={{ width: 5, height: 5, flex: "0 0 5px" }}
            />
            <span className="truncate">{tab.title}</span>
            {/* Only when split, and marked as a count: a bare number after the title
                reads as part of the name, which made a new pane look like a rename. */}
            {panes.length > 1 && (
              <span className="dim tabular" title={`${panes.length} panes`}>
                ·{panes.length}
              </span>
            )}
          </button>
        );
      })}

      <button
        className="btn btn-ghost btn-xs"
        onClick={addNew}
        title={
          s.appearance.newButtonAction === "pane"
            ? `New pane · ${formatChord("mod+t")}`
            : `New tab · ${formatChord("mod+shift+n")}`
        }
        aria-label={s.appearance.newButtonAction === "pane" ? "New pane" : "New tab"}
        style={{ flex: "none" }}
      >
        +
      </button>

      <div className="grow" />

      {/* Split controls sit next to the panes they act on. Hidden in a vertical strip,
          where there is no room and where they would read as tabs. */}
      {!vertical && (
        <>
          <button
            className="btn btn-ghost btn-xs"
            onClick={() => s.splitFocusedPane("row", makePane())}
            title={`Split right · ${formatChord("mod+d")}`}
          >
            Split right
          </button>
          <button
            className="btn btn-ghost btn-xs"
            onClick={() => s.splitFocusedPane("column", makePane())}
            title={`Split down · ${formatChord("mod+shift+d")}`}
          >
            Split down
          </button>
          <button
            className="btn btn-ghost btn-xs"
            onClick={() => s.toggleZoom()}
            title={`Zoom the focused pane · ${formatChord("mod+shift+z")}`}
            style={{ color: zoomed ? "var(--tervin-accent)" : undefined }}
          >
            Zoom
          </button>
        </>
      )}
    </div>
  );
}

function StatusRail() {
  const s = useWorkspace();
  const thread = s.activeThreadId ? s.threads[s.activeThreadId] : null;
  const profile = s.agents?.profiles.find((p) => p.id === s.activeProfileId);
  const pane = (() => {
    const tab = s.tabs.find((t) => t.id === s.activeTabId);
    return tab?.activePaneId ? s.panes[tab.activePaneId] : undefined;
  })();
  const meta = thread?.info?.metadata;

  return (
    <footer
      className="row"
      style={{
        height: "var(--statusrail-h)",
        flex: "none",
        padding: "0 var(--sp-6)",
        borderTop: "1px solid var(--tervin-raised)",
        gap: "var(--sp-7)",
        fontSize: "var(--text-meta)",
        color: "var(--tervin-muted)",
      }}
    >
      <span className="truncate" title={pane?.cwd}>
        {s.environment?.shell ?? "shell"} ·{" "}
        {shortenPath(pane?.cwd ?? s.environment?.project_root ?? "")}
      </span>

      <span>{pane?.remote ? "remote" : "local"}</span>

      {s.gitStatus && (
        <span className="tabular">
          {s.gitStatus.branch ?? "detached"}
          {s.gitStatus.ahead > 0 && ` ↑${s.gitStatus.ahead}`}
          {s.gitStatus.behind > 0 && ` ↓${s.gitStatus.behind}`}
          {s.gitStatus.operation_in_progress && ` · ${s.gitStatus.operation_in_progress}`}
        </span>
      )}

      <div className="grow" />

      {profile && (
        <span title={profile.sensitive ? "Shared or work account" : undefined}>
          {profile.name}
          {profile.sensitive && <span className="tone-amber"> · shared</span>}
        </span>
      )}

      {thread && (
        <span className={`tone-${toneForState(thread.state)}`}>
          {thread.state.replace(/_/g, " ")}
        </span>
      )}

      {meta?.model && <span className="tabular">{meta.model}</span>}
      {meta?.resume_id && <span className="tabular dim">resumable</span>}
      <span className="tabular">{s.blocks.length} blocks</span>
    </footer>
  );
}

function Notices() {
  const s = useWorkspace();
  return (
    <div style={{ flex: "none", borderBottom: "1px solid var(--tervin-raised)" }}>
      {s.notices.map((notice, i) => (
        <div
          key={notice}
          className="row"
          style={{
            padding: "var(--sp-2) var(--sp-6)",
            background: "var(--tervin-block-hover)",
            fontSize: "var(--text-meta)",
          }}
        >
          <span className="dot dot-amber" />
          <span className="grow selectable" style={{ textWrap: "pretty" }}>
            {notice}
          </span>
          <button className="btn btn-xs btn-ghost" onClick={() => s.dismissNotice(i)}>
            Dismiss
          </button>
        </div>
      ))}
    </div>
  );
}

// ------------------------------------------------------------------ actions

/** A fresh shell pane, in the given directory or the project root. */
function makePane(cwd?: string) {
  const state = useWorkspace.getState();
  return {
    id: crypto.randomUUID(),
    title: "Shell",
    cwd: cwd ?? state.environment?.project_root ?? ".",
    threadId: null,
    exited: false,
    exitCode: null,
  };
}

/**
 * Run a keymap action.
 *
 * Returns whether it was handled. An unhandled action falls through to the
 * program in the pane, which is what keeps vim, emacs and readline working.
 */
function runAction(action: string, pane: string | null): boolean {
  const s = useWorkspace.getState();

  switch (action) {
    case "pane.new":
    case "tab.new":
      s.setSurface("terminal");
      s.addPane(makePane());
      return true;
    case "pane.split.right":
      s.setSurface("terminal");
      s.splitFocusedPane("row", makePane());
      return true;
    case "pane.split.down":
      s.setSurface("terminal");
      s.splitFocusedPane("column", makePane());
      return true;
    case "pane.close":
      if (pane) s.removePane(pane);
      return true;
    case "pane.focus.next":
      s.focusAdjacentPane(true);
      return true;
    case "pane.focus.prev":
      s.focusAdjacentPane(false);
      return true;
    case "pane.swap":
      s.swapFocusedPane();
      return true;
    case "pane.zoom":
      s.toggleZoom();
      return true;
    case "pane.duplicate": {
      const current = pane ? s.panes[pane] : undefined;
      s.splitFocusedPane("row", makePane(current?.cwd));
      return true;
    }

    case "palette.open":
      s.setPalette(true);
      return true;
    case "search.open":
      s.setSurface("terminal");
      s.setSearch(true);
      return true;
    case "overlay.close":
      s.setPalette(false);
      s.setSearch(false);
      s.setSettings(false);
      return true;

    case "settings.open":
      s.setSettings(true);
      return true;

    case "surface.terminal":
      s.setSurface("terminal");
      return true;
    case "surface.plan":
      s.setSurface("plan");
      return true;
    case "surface.agents":
      s.setSurface("agents");
      return true;
    case "surface.review":
      s.setSurface("review");
      return true;
    case "surface.history":
      s.setSurface("history");
      return true;
    // With surfaces there is no inspector to toggle; these switch surface, which
    // is the same intent expressed in a two-column layout.
    case "commands.saved":
      s.setSavedCommands(true);
      return true;
    case "connections.open":
      // Previously switched surface, which quietly meant the Connections panel — SSH
      // hosts, tmux sessions, serial ports — was written and never reachable.
      s.setConnections(true);
      return true;
    case "inspector.toggle":
      s.setSurface(s.surface === "agents" ? "terminal" : "agents");
      return true;
    case "rail.toggle":
      s.setSurface(s.surface === "review" ? "terminal" : "review");
      return true;

    case "agent.stop":
      if (s.activeThreadId) void api.threadInterrupt(s.activeThreadId).catch(() => {});
      return true;
    case "agent.approve":
      if (s.pendingApprovals.length > 0) s.setSurface("agents");
      return true;
    case "agent.profile":
      s.setSettings(true);
      return true;

    case "terminal.clear":
      if (pane) clearPane(pane);
      return true;
    case "terminal.selectAll":
      if (pane) selectAllInPane(pane);
      return true;
    case "block.prev":
      if (pane) scrollPane(pane, -10);
      return true;
    case "block.next":
      if (pane) scrollPane(pane, 10);
      return true;

    case "terminal.zoomIn":
      s.setAppearance({ fontSize: Math.min(s.appearance.fontSize + 1, 32) });
      return true;
    case "terminal.zoomOut":
      s.setAppearance({ fontSize: Math.max(s.appearance.fontSize - 1, 9) });
      return true;
    case "terminal.zoomReset":
      s.setAppearance({ fontSize: 13 });
      return true;

    default:
      // Unhandled: the keystroke belongs to whatever is running.
      return false;
  }
}

// ------------------------------------------------------------------ helpers

export function toneForState(state: api.ThreadState): string {
  if (["awaiting_input", "waiting_for_permission", "review_required"].includes(state))
    return "amber";
  if (state === "completed") return "green";
  if (["failed", "interrupted", "disconnected"].includes(state)) return "red";
  if (state === "unknown" || state === "idle") return "muted";
  return "teal";
}

export function shortenPath(path: string): string {
  if (!path) return "";
  const parts = path.split("/").filter(Boolean);
  return parts.length <= 2 ? path : `…/${parts.slice(-2).join("/")}`;
}
