/**
 * One terminal pane.
 *
 * Rendering is delegated to xterm.js rather than reimplemented. That is the most
 * important decision in the UI: xterm already handles the unglamorous parts of
 * terminal correctness — VT/ANSI state, wide characters and combining marks,
 * bracketed paste, mouse reporting, the alternate screen, reflow on resize — and
 * a hand-rolled renderer would spend years catching up while breaking Neovim and
 * tmux along the way.
 *
 * Bytes are written straight into xterm and never pass through React state. A
 * build log can emit megabytes a second; a `setState` per chunk would make the
 * app unusable exactly when the user most needs to watch output.
 *
 * What this component adds on top of xterm is the behaviour a terminal is
 * expected to have but xterm deliberately leaves to its host: smart selection,
 * paste safety, copy-on-select, a context menu, and search.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { Terminal, type ILink, type IViewportRange } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { CanvasAddon } from "@xterm/addon-canvas";
import { ImageAddon } from "@xterm/addon-image";
import "@xterm/xterm/css/xterm.css";

import * as api from "../lib/api";
import { overlayOpen, useWorkspace, type Appearance } from "../lib/store";
import { findLinks, pasteNeedsConfirmation, type LinkMatch } from "../lib/links";
import { chooseRenderer, markRendererAttempt } from "../lib/renderer";
import { findTheme, toXtermTheme } from "../design/themes";

interface Props {
  paneId: string;
  active: boolean;
  onFocus: () => void;
}

/** Per-pane handles, held outside React so effects can reach them cheaply. */
interface Handles {
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  backendPaneId: string | null;
  disposed: boolean;
}

const handlesByPane = new Map<string, Handles>();

/**
 * Resize the terminal to its container, safely.
 *
 * `fit()` reaches into xterm's renderer to measure a cell. Called before the
 * renderer is attached — or while the container still measures zero, which is the
 * normal state for a flex child on its first layout pass — it throws
 * `undefined is not an object (evaluating 'this._renderer.value.dimensions')`.
 *
 * Both conditions are transient, so this checks for them and lets the next
 * `ResizeObserver` callback try again rather than failing the pane.
 */
function safeFit(handles: Handles, host: HTMLElement): boolean {
  if (handles.disposed) return false;
  if (host.clientWidth < 2 || host.clientHeight < 2) return false;
  try {
    handles.fit.fit();
    return true;
  } catch {
    return false;
  }
}

// ------------------------------------------------------------ pane commands
// Exposed as functions rather than context, so the palette and keymap can drive
// a pane without the component tree needing to know about them.

export interface SearchOptions {
  regex?: boolean;
  caseSensitive?: boolean;
  wholeWord?: boolean;
}

export function searchInPane(
  paneId: string,
  query: string,
  forward = true,
  options: SearchOptions = {},
): boolean {
  const handles = handlesByPane.get(paneId);
  if (!handles || !query) return false;
  const opts = {
    regex: options.regex ?? false,
    caseSensitive: options.caseSensitive ?? false,
    wholeWord: options.wholeWord ?? false,
    decorations: {
      matchBackground: "#D5AB68",
      matchOverviewRuler: "#D5AB68",
      activeMatchBackground: "#68AEA5",
      activeMatchColorOverviewRuler: "#68AEA5",
    },
  };
  try {
    return forward
      ? handles.search.findNext(query, opts)
      : handles.search.findPrevious(query, opts);
  } catch {
    // An in-progress regex such as `[` is a parse error, not a failure worth
    // surfacing — the user is still typing.
    return false;
  }
}

export function clearSearch(paneId: string): void {
  handlesByPane.get(paneId)?.search.clearDecorations();
}

export function selectionInPane(paneId: string): string {
  return handlesByPane.get(paneId)?.term.getSelection() ?? "";
}

/** Type text into a pane without submitting it. */
export function writeToPane(paneId: string, text: string): void {
  const handles = handlesByPane.get(paneId);
  if (!handles?.backendPaneId) return;
  void api.ptyWrite(handles.backendPaneId, new TextEncoder().encode(text));
}

export function clearPane(paneId: string): void {
  handlesByPane.get(paneId)?.term.clear();
}

export function selectAllInPane(paneId: string): void {
  handlesByPane.get(paneId)?.term.selectAll();
}

export function scrollPane(paneId: string, lines: number): void {
  handlesByPane.get(paneId)?.term.scrollLines(lines);
}

/** Whether the pane is showing a full-screen program. */
export function paneUsesAltScreen(paneId: string): boolean {
  const term = handlesByPane.get(paneId)?.term;
  return term ? term.buffer.active.type === "alternate" : false;
}

// ------------------------------------------------------------------ component

interface ContextMenuState {
  x: number;
  y: number;
  selection: string;
  link: LinkMatch | null;
}

interface PasteConfirm {
  text: string;
  reason: string;
  lines: number;
}

export function TerminalPane({ paneId, active, onFocus }: Props) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const appearance = useWorkspace((s) => s.appearance);
  const cwd = useWorkspace((s) => s.panes[paneId]?.cwd);
  const threadId = useWorkspace((s) => s.panes[paneId]?.threadId ?? null);
  const program = useWorkspace((s) => s.panes[paneId]?.program ?? null);
  const paneArgs = useWorkspace((s) => s.panes[paneId]?.args);
  const paneEnv = useWorkspace((s) => s.panes[paneId]?.env);
  const markPaneExited = useWorkspace((s) => s.markPaneExited);
  const pushNotice = useWorkspace((s) => s.pushNotice);
  /** True while a dialog is layered over the workspace. */
  const blocked = useWorkspace(overlayOpen);

  const [menu, setMenu] = useState<ContextMenuState | null>(null);
  const [pasteConfirm, setPasteConfirm] = useState<PasteConfirm | null>(null);

  // Kept in a ref so the xterm callbacks below always read the current value
  // without needing to be rebuilt when settings change.
  const appearanceRef = useRef(appearance);
  appearanceRef.current = appearance;

  const sendPaste = useCallback((text: string) => {
    const handles = handlesByPane.get(paneId);
    if (!handles?.backendPaneId) return;
    // Let xterm frame the paste: it adds the bracketed-paste markers when the
    // application has asked for them, which is the whole safety mechanism.
    handles.term.paste(text);
  }, [paneId]);

  // Create the terminal and its PTY exactly once per pane.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const a = appearanceRef.current;
    const term = new Terminal({
      allowProposedApi: true,
      cursorBlink: a.cursorBlink,
      cursorStyle: a.cursorStyle,
      fontFamily: a.fontFamily,
      fontSize: a.fontSize,
      lineHeight: a.lineHeight,
      scrollback: a.scrollback,
      theme: toXtermTheme(findTheme(a.themeId)),
      convertEol: false,
      macOptionIsMeta: true,
      rightClickSelectsWord: false,
      // What counts as a word for double-click. Configurable because a shell
      // command line tokenises differently from prose.
      wordSeparator: a.wordSeparators,
      // Announce output to assistive technology.
      screenReaderMode: a.screenReaderMode,
      fontLigatures: a.ligatures,
    } as ConstructorParameters<typeof Terminal>[0]);

    const fit = new FitAddon();
    const search = new SearchAddon();
    term.loadAddon(fit);
    term.loadAddon(search);
    // Unicode 11 widths: without it, CJK and emoji misalign the whole grid.
    term.loadAddon(new Unicode11Addon());
    term.unicode.activeVersion = "11";

    // Sixel and iTerm2 inline images. Skipped on the DOM renderer, where the
    // addon has no canvas to composite into and is the likely crash suspect
    // anyway if we got here by falling back.
    try {
      if (chooseRenderer().mode !== "dom") term.loadAddon(
        new ImageAddon({
          // Bounded, because a program can emit images faster than they are
          // scrolled away and this is a per-pane cost.
          storageLimit: 48,
          enableSizeReports: true,
          sixelSupport: true,
          iipSupport: true,
        }),
      );
    } catch {
      // Images are additive; a failure here must not stop the pane opening.
    }

    // A host element must contain exactly one terminal.
    //
    // `Terminal.dispose()` does not reliably remove its own DOM, and React
    // StrictMode mounts every effect twice in development — so without this the
    // host accumulates orphaned `.xterm` elements. They stack vertically in
    // normal block layout, which is what put empty grey bands above and below
    // the live terminal.
    host.replaceChildren();
    term.open(host);

    if (host.childElementCount !== 1) {
      api.uiLog(
        "warn",
        `pane ${paneId}: host holds ${host.childElementCount} children after open`,
      );
    }

    // Renderer selection.
    //
    // WebGL is what keeps a streaming log at frame rate, but it drives a real GPU
    // driver from inside a system webview and can take the whole web content
    // process down with it — a failure no `catch` here can see, because there is
    // no exception, just a dead process and a blank window.
    //
    // So the attempt is recorded durably first. If this run never paints, the
    // next launch reads that record and steps down to a safer renderer. See
    // lib/renderer for the full reasoning.
    const { mode: rendererMode, reason: rendererReason } = chooseRenderer();
    markRendererAttempt(rendererMode);

    if (rendererMode === "webgl") {
      try {
        const webgl = new WebglAddon();
        // Context loss is recoverable and distinct from a crash: drop the addon
        // and let the DOM renderer take over for this pane.
        webgl.onContextLoss(() => webgl.dispose());
        term.loadAddon(webgl);
      } catch {
        try {
          term.loadAddon(new CanvasAddon());
        } catch {
          // The DOM renderer stays active: slower, still correct.
        }
      }
    } else if (rendererMode === "canvas") {
      try {
        term.loadAddon(new CanvasAddon());
      } catch {
        // As above.
      }
    }
    // "dom" needs no addon; it is xterm's built-in renderer.

    if (rendererReason) {
      pushNotice(rendererReason);
    }

    const handles: Handles = { term, fit, search, backendPaneId: null, disposed: false };
    handlesByPane.set(paneId, handles);

    // ----------------------------------------------------------- smart links
    // One provider for every kind, so overlap resolution happens once and two
    // providers can never half-cover the same span.
    const linkDisposable = term.registerLinkProvider({
      provideLinks(lineNumber, callback) {
        const line = term.buffer.active.getLine(lineNumber - 1);
        if (!line) {
          callback(undefined);
          return;
        }
        const text = line.translateToString(true);
        const matches = findLinks(text);
        if (matches.length === 0) {
          callback(undefined);
          return;
        }

        const links: ILink[] = matches.map((match) => ({
          // xterm ranges are 1-based and inclusive at both ends.
          range: {
            start: { x: match.start + 1, y: lineNumber },
            end: { x: match.end, y: lineNumber },
          },
          text: match.text,
          activate: () => void activateLink(match, cwd ?? "."),
          hover: (_e: MouseEvent, _t: string) => undefined,
          leave: () => undefined,
        }));
        callback(links);
      },
    });

    // ---------------------------------------------------------- paste safety
    // Intercepted on the textarea rather than left to xterm, because the
    // decision to warn depends on whether the *application* enabled bracketed
    // paste — which xterm knows but does not act on.
    const onPaste = (event: ClipboardEvent) => {
      const text = event.clipboardData?.getData("text") ?? "";
      if (!text) return;
      const verdict = pasteNeedsConfirmation(text, term.modes.bracketedPasteMode);
      if (!verdict.needed) return;
      event.preventDefault();
      event.stopPropagation();
      setPasteConfirm({ text, reason: verdict.reason ?? "", lines: verdict.lines });
    };
    term.textarea?.addEventListener("paste", onPaste);

    // -------------------------------------------------------- copy on select
    const selectionSub = term.onSelectionChange(() => {
      if (!appearanceRef.current.copyOnSelect) return;
      const selection = term.getSelection();
      if (selection) void navigator.clipboard.writeText(selection).catch(() => {});
    });

    // ------------------------------------------------------------- PTY wiring
    //
    // Deferred to after the first layout: the shell is told its size once, at
    // startup, so starting it before the pane has measured would give it the
    // wrong geometry and make the first prompt wrap badly.
    const startPty = () => {
      if (handles.disposed || handles.backendPaneId) return;
      void api
      .ptySpawn(
        {
          cwd: cwd ?? null,
          cols: term.cols,
          rows: term.rows,
          // Absent for a plain shell; set for SSH, tmux, serial and agent panes.
          program,
          args: paneArgs ?? [],
          env: paneEnv ?? [],
          thread_id: threadId,
        },
        (bytes) => {
          if (!handles.disposed) term.write(bytes);
        },
      )
      .then((response) => {
        handles.backendPaneId = response.pane_id;
        // Integration is injected automatically, so this only fires when it
        // genuinely could not be — and then it says why.
        if (!response.integration_installed) {
          pushNotice(
            response.integration_note ??
              `Commands in this pane will not be captured as Blocks: Tervin has no shell hook for ${
                response.shell ?? "this shell"
              }.`,
          );
        }
      })
      .catch((e) => {
        term.writeln(`\x1b[31mCould not start a shell: ${String(e)}\x1b[0m`);
      });
    };

    // Two frames: one for the browser to lay the flex tree out, one for xterm to
    // attach its renderer. Only then can the terminal be measured.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        safeFit(handles, host);
        startPty();
      });
    });

    const dataSub = term.onData((data) => {
      if (handles.backendPaneId) {
        void api.ptyWrite(handles.backendPaneId, new TextEncoder().encode(data));
      }
    });

    // Binary data from some paste and mouse paths must not be UTF-8 encoded.
    const binarySub = term.onBinary((data) => {
      if (!handles.backendPaneId) return;
      const bytes = new Uint8Array(data.length);
      for (let i = 0; i < data.length; i++) bytes[i] = data.charCodeAt(i) & 0xff;
      void api.ptyWrite(handles.backendPaneId, bytes);
    });

    const resizeSub = term.onResize(({ cols, rows }) => {
      if (handles.backendPaneId) void api.ptyResize(handles.backendPaneId, cols, rows);
    });

    // ResizeObserver rather than a window listener: a pane also changes size
    // when a sibling moves or the inspector opens.
    const observer = new ResizeObserver(() => {
      // Also the recovery path for a pane whose first layout was zero-sized: the
      // observer fires again once it has real dimensions.
      if (safeFit(handles, host)) startPty();
    });
    observer.observe(host);

    const exitUnlisten = api.on<{ paneId: string; exitCode: number | null }>(
      "pane://exited",
      (payload) => {
        if (payload.paneId !== handles.backendPaneId) return;
        markPaneExited(paneId, payload.exitCode);
        term.writeln(
          `\r\n\x1b[90m[process exited${
            payload.exitCode === null ? "" : ` with status ${payload.exitCode}`
          }]\x1b[0m`,
        );
      },
    );

    return () => {
      handles.disposed = true;
      observer.disconnect();
      linkDisposable.dispose();
      selectionSub.dispose();
      dataSub.dispose();
      binarySub.dispose();
      resizeSub.dispose();
      term.textarea?.removeEventListener("paste", onPaste);
      void exitUnlisten.then((un) => un());
      if (handles.backendPaneId) void api.ptyClose(handles.backendPaneId).catch(() => {});
      handlesByPane.delete(paneId);
      term.dispose();
      // Belt and braces: dispose is not guaranteed to detach the element.
      host.replaceChildren();
    };
    // Keyed only on paneId: appearance is applied in place below, and
    // re-creating the terminal on a font change would kill the shell.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paneId]);

  // Apply appearance in place, so a theme or font change is instant and does not
  // disturb the running session.
  useEffect(() => {
    const handles = handlesByPane.get(paneId);
    const host = hostRef.current;
    if (!handles || !host) return;
    applyAppearance(handles.term, appearance);
    // A font change alters cell size, so the grid has to be remeasured.
    safeFit(handles, host);
  }, [paneId, appearance]);

  // Focus follows the active pane — but never while something is layered over the
  // workspace. A pane holding focus under a dialog sends the dialog's keystrokes to
  // the shell instead, and `Return` in an approval sheet would run a command. The
  // overlays focus their own controls; this is the guard that makes a future one
  // safe by default.
  useEffect(() => {
    const term = handlesByPane.get(paneId)?.term;
    if (!term) return;
    if (blocked) {
      term.blur();
    } else if (active) {
      term.focus();
    }
  }, [active, blocked, paneId]);

  // Close the context menu on any outside interaction.
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("resize", close);
    };
  }, [menu]);

  const onContextMenu = (event: React.MouseEvent) => {
    event.preventDefault();
    const term = handlesByPane.get(paneId)?.term;
    const selection = term?.getSelection() ?? "";
    // Offer to act on whatever is under the pointer, not just the selection.
    const link = selection ? (findLinks(selection)[0] ?? null) : null;
    setMenu({ x: event.clientX, y: event.clientY, selection, link });
  };

  return (
    <div
      className="terminal-surface grow"
      style={{
        background: "var(--tervin-terminal-bg)",
        padding: "var(--sp-3)",
        minHeight: 0,
        minWidth: 0,
        overflow: "hidden",
        position: "relative",
      }}
      onMouseDown={onFocus}
      onContextMenu={onContextMenu}
    >
      <div
        ref={hostRef}
        // `display: flex` rather than block: if anything ever does leave a second
        // element behind, it cannot silently stack and shrink the real terminal.
        style={{ width: "100%", height: "100%", display: "flex", minHeight: 0 }}
      />

      {menu && (
        <ContextMenu
          state={menu}
          paneId={paneId}
          cwd={cwd ?? "."}
          onClose={() => setMenu(null)}
        />
      )}

      {pasteConfirm && (
        <PasteConfirmDialog
          paste={pasteConfirm}
          onCancel={() => setPasteConfirm(null)}
          onConfirm={() => {
            sendPaste(pasteConfirm.text);
            setPasteConfirm(null);
          }}
        />
      )}
    </div>
  );
}

// -------------------------------------------------------------------- pieces

function ContextMenu({
  state,
  paneId,
  cwd,
  onClose,
}: {
  state: ContextMenuState;
  paneId: string;
  cwd: string;
  onClose: () => void;
}) {
  const s = useWorkspace();
  const items: { label: string; run: () => void; disabled?: boolean }[] = [
    {
      label: "Copy",
      disabled: !state.selection,
      run: () => void navigator.clipboard.writeText(state.selection),
    },
    {
      label: "Paste",
      run: () => {
        void navigator.clipboard.readText().then((text) => writeToPane(paneId, text));
      },
    },
    { label: "Select all", run: () => selectAllInPane(paneId) },
  ];

  if (state.link) {
    items.unshift({
      label: state.link.hint,
      run: () => void activateLink(state.link!, cwd),
    });
  }

  if (state.selection) {
    items.push({
      label: "Send selection to agent",
      run: () => {
        s.setInspectorTab("thread");
        s.stageAttachment({ kind: "selection", text: state.selection });
      },
    });
  }

  items.push(
    { label: "Search in this pane", run: () => s.setSearch(true) },
    { label: "Clear terminal", run: () => clearPane(paneId) },
  );

  return (
    <div
      className="overlay-surface"
      role="menu"
      onClick={(e) => e.stopPropagation()}
      style={{
        position: "fixed",
        // Nudged inside the viewport so a menu near an edge stays reachable.
        left: Math.min(state.x, window.innerWidth - 240),
        top: Math.min(state.y, window.innerHeight - items.length * 26 - 16),
        minWidth: 220,
        padding: "var(--sp-1) 0",
        zIndex: 300,
      }}
    >
      {items.map((item) => (
        <button
          key={item.label}
          role="menuitem"
          disabled={item.disabled}
          onClick={() => {
            item.run();
            onClose();
          }}
          className="row truncate"
          style={{
            width: "100%",
            height: 26,
            padding: "0 var(--sp-6)",
            fontSize: "var(--text-control)",
            color: item.disabled ? "var(--tervin-dim)" : "var(--tervin-ink)",
            textAlign: "left",
          }}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}

/**
 * The multi-line paste warning.
 *
 * Shows the text itself, because the question "is this safe to run" cannot be
 * answered without seeing what it is.
 */
function PasteConfirmDialog({
  paste,
  onCancel,
  onConfirm,
}: {
  paste: PasteConfirm;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div
      className="overlay-scrim"
      role="dialog"
      aria-modal="true"
      aria-label="Confirm paste"
      onClick={onCancel}
      style={{ display: "grid", placeItems: "center", padding: "var(--sp-9)" }}
    >
      <div
        className="overlay-surface col"
        onClick={(e) => e.stopPropagation()}
        style={{ width: "min(560px, 96vw)", maxHeight: "70vh" }}
      >
        <div
          className="row"
          style={{
            padding: "var(--sp-5) var(--sp-7)",
            borderBottom: "1px solid var(--tervin-line)",
          }}
        >
          <span className="dot dot-amber" />
          <strong style={{ fontSize: "var(--text-subsection)" }}>Paste {paste.lines} lines?</strong>
        </div>

        <div style={{ padding: "var(--sp-7)", overflow: "auto" }}>
          <p className="meta" style={{ margin: 0, textWrap: "pretty" }}>
            {paste.reason}
          </p>
          <pre
            className="mono selectable"
            style={{
              margin: "var(--sp-5) 0 0",
              padding: "var(--sp-4)",
              background: "var(--tervin-bg)",
              border: "1px solid var(--tervin-line)",
              borderRadius: "var(--radius-control)",
              maxHeight: 260,
              overflow: "auto",
              fontSize: "var(--font-mono-size)",
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
            }}
          >
            {paste.text}
          </pre>
        </div>

        <div
          className="row"
          style={{
            padding: "var(--sp-5) var(--sp-7)",
            borderTop: "1px solid var(--tervin-line)",
          }}
        >
          <span className="meta grow">
            Enabling bracketed paste in your shell removes this prompt.
          </span>
          <button className="btn" onClick={onCancel}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={onConfirm} autoFocus>
            Paste
          </button>
        </div>
      </div>
    </div>
  );
}

// ------------------------------------------------------------------- helpers

/** Act on a recognised span. */
async function activateLink(match: LinkMatch, cwd: string): Promise<void> {
  const opener = await import("@tauri-apps/plugin-opener").catch(() => null);

  switch (match.kind) {
    case "url":
      await opener?.openUrl(match.text);
      return;

    case "port":
      await opener?.openUrl(`http://localhost:${match.port}`);
      return;

    case "email":
      await opener?.openUrl(`mailto:${match.text}`);
      return;

    case "file":
    case "stack-frame": {
      if (!match.path) return;
      const absolute = match.path.startsWith("/")
        ? match.path
        : `${cwd.replace(/\/$/, "")}/${match.path.replace(/^\.\//, "")}`;
      // Opened with the system handler, which respects the user's own default
      // editor rather than Tervin picking one.
      await opener?.openPath(absolute).catch(() => {});
      return;
    }

    case "commit":
    case "issue":
      // No unambiguous destination exists — the remote and issue tracker are
      // project-specific — so the reference is copied rather than guessed at.
      await navigator.clipboard.writeText(match.text).catch(() => {});
      return;
  }
}

function applyAppearance(term: Terminal, a: Appearance): void {
  const theme = findTheme(a.themeId);
  term.options.theme = toXtermTheme(theme);
  term.options.fontFamily = a.fontFamily;
  term.options.fontSize = a.fontSize;
  term.options.lineHeight = a.lineHeight;
  term.options.cursorStyle = a.cursorStyle;
  term.options.cursorBlink = a.cursorBlink;
  term.options.scrollback = a.scrollback;
  term.options.wordSeparator = a.wordSeparators;
  term.options.screenReaderMode = a.screenReaderMode;
}

export type { IViewportRange };
