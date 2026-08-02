/**
 * The command palette.
 *
 * One fuzzy-searchable surface over actions, panes, Blocks, and aliases. Ranking
 * is context-aware only in the sense that exact prefix matches win — deliberately
 * simple, because a palette that reorders unpredictably is worse than one that is
 * boring and learnable.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import * as api from "../lib/api";
import { useWorkspace, type Surface } from "../lib/store";
import { writeToPane } from "./TerminalPane";

interface Entry {
  id: string;
  label: string;
  category: string;
  hint?: string;
  run: () => void;
}

export function CommandPalette({ onNewPane }: { onNewPane: () => void }) {
  const s = useWorkspace();
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [files, setFiles] = useState<api.Completion[]>([]);

  // Project files, ranked by the same matcher the composer uses.
  //
  // Fetched rather than filtered locally: the index can hold 200k paths, and
  // shipping that to the frontend to filter would cost far more than a query.
  useEffect(() => {
    let cancelled = false;
    const handle = setTimeout(() => {
      void api
        .pathComplete(query, "any", null, 30)
        .then((next) => {
          if (!cancelled) setFiles(next);
        })
        .catch(() => {
          if (!cancelled) setFiles([]);
        });
    }, 60);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [query]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const entries = useMemo<Entry[]>(() => {
    const out: Entry[] = [];

    out.push({
      id: "pane.new",
      label: "New pane",
      category: "Workspace",
      hint: "⌘T",
      run: onNewPane,
    });
    const freshPane = () => ({
      id: crypto.randomUUID(),
      title: "Shell",
      cwd: s.environment?.project_root ?? ".",
      threadId: null,
      exited: false,
      exitCode: null,
    });
    out.push({
      id: "split.h",
      label: "Split right",
      category: "Workspace",
      hint: "⌘D",
      run: () => s.splitFocusedPane("row", freshPane()),
    });
    out.push({
      id: "split.v",
      label: "Split down",
      category: "Workspace",
      hint: "⇧⌘D",
      run: () => s.splitFocusedPane("column", freshPane()),
    });
    out.push({
      id: "pane.zoom",
      label: "Zoom pane",
      category: "Workspace",
      hint: "⇧⌘↵",
      run: () => s.toggleZoom(),
    });
    out.push({
      id: "pane.swap",
      label: "Swap pane with next",
      category: "Workspace",
      run: () => s.swapFocusedPane(),
    });
    // Surfaces, so every one is reachable from the keyboard.
    const surfaces: [Surface, string][] = [
      ["terminal", "Terminal"],
      ["plan", "Plan"],
      ["agents", "Agents"],
      ["review", "Review"],
    ];
    for (const [id, label] of surfaces) {
      out.push({
        id: `surface.${id}`,
        label: `Go to ${label}`,
        category: "Surface",
        run: () => s.setSurface(id),
      });
    }
    out.push({
      id: "settings",
      label: "Open settings",
      category: "Workspace",
      hint: "⌘,",
      run: () => s.setSettings(true),
    });

    // Agent profiles: this is the fast path for switching account or install.
    for (const profile of s.agents?.profiles ?? []) {
      out.push({
        id: `agent.${profile.id}`,
        label: `Agent: ${profile.name}`,
        category: "Agents",
        hint: profile.badge ?? profile.runtime_id,
        run: () => s.setActiveProfile(profile.id),
      });
    }

    // Recent Blocks, so a command can be found and re-run without leaving the
    // keyboard.
    for (const block of s.blocks.slice(0, 60)) {
      if (!block.command) continue;
      out.push({
        id: `block.${block.id}`,
        label: block.command,
        category: "History",
        hint: block.status,
        run: () => {
          const tab = s.tabs.find((t) => t.id === s.activeTabId);
          if (tab?.activePaneId) writeToPane(tab.activePaneId, block.command);
        },
      });
    }

    // Shell aliases are runnable things too, shown with what they expand to.
    for (const [name, expansion] of Object.entries(s.environment?.aliases.aliases ?? {})) {
      out.push({
        id: `alias.${name}`,
        label: name,
        category: "Alias",
        hint: expansion,
        run: () => {
          const tab = s.tabs.find((t) => t.id === s.activeTabId);
          if (tab?.activePaneId) writeToPane(tab.activePaneId, name);
        },
      });
    }

    return out;
  }, [s, onNewPane]);

  const results = useMemo(() => {
    const ranked = rank(entries, query).slice(0, 40);

    // File hits arrive already ranked by the Rust matcher, so they are appended
    // rather than re-scored — re-ranking them here with the simpler local scorer
    // would be strictly worse.
    const fileEntries: Entry[] = files.map((file) => ({
      id: `file.${file.path}`,
      label: file.path,
      category: file.is_dir ? "Folder" : "File",
      hint: file.is_dir ? "cd" : "open",
      run: () => {
        const tab = s.tabs.find((t) => t.id === s.activeTabId);
        if (!tab?.activePaneId) return;
        // A folder is somewhere to go; a file is something to open.
        writeToPane(
          tab.activePaneId,
          file.is_dir ? `cd ${shellQuote(file.path)}` : `${shellQuote(file.path)}`,
        );
      },
    }));

    // Interleave: when the query looks like a path, files lead.
    const looksLikePath = query.includes("/") || query.includes(".");
    return looksLikePath
      ? [...fileEntries, ...ranked].slice(0, 60)
      : [...ranked, ...fileEntries].slice(0, 60);
  }, [entries, query, files, s]);

  useEffect(() => {
    setIndex(0);
  }, [query]);

  function runSelected() {
    const entry = results[index];
    if (!entry) return;
    entry.run();
    s.setPalette(false);
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Command palette"
      onClick={() => s.setPalette(false)}
      style={{
        position: "fixed",
        inset: 0,
        background: "color-mix(in srgb, var(--tervin-bg) 60%, transparent)",
        display: "flex",
        justifyContent: "center",
        alignItems: "flex-start",
        paddingTop: "12vh",
        zIndex: 200,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="col"
        style={{
          width: "min(680px, 92vw)",
          background: "var(--tervin-raised)",
          border: "1px solid var(--tervin-line)",
          borderRadius: "var(--radius-lg)",
          overflow: "hidden",
          maxHeight: "70vh",
        }}
      >
        <input
          ref={inputRef}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setIndex((i) => Math.min(i + 1, results.length - 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setIndex((i) => Math.max(i - 1, 0));
            } else if (e.key === "Enter") {
              e.preventDefault();
              runSelected();
            } else if (e.key === "Escape") {
              s.setPalette(false);
            }
          }}
          placeholder="Actions, layouts, agents, history, aliases…"
          aria-label="Search commands"
          style={{
            border: "none",
            borderBottom: "1px solid var(--tervin-line)",
            borderRadius: 0,
            padding: "var(--sp-3) var(--sp-4)",
            fontSize: "var(--text-title)",
            background: "transparent",
          }}
        />

        <div style={{ overflow: "auto", minHeight: 0 }}>
          {results.length === 0 ? (
            <div className="empty">
              Nothing matches “{query}”. The palette covers actions, layouts, agent
              profiles, command history, your shell aliases, and every file in the
              project.
            </div>
          ) : (
            results.map((entry, i) => (
              <button
                key={entry.id}
                onMouseEnter={() => setIndex(i)}
                onClick={runSelected}
                className="row"
                style={{
                  width: "100%",
                  padding: "var(--sp-2) var(--sp-4)",
                  gap: "var(--sp-2)",
                  background: i === index ? "var(--tervin-panel)" : "transparent",
                  borderLeft:
                    i === index ? "2px solid var(--tervin-accent)" : "2px solid transparent",
                  textAlign: "left",
                }}
              >
                <span className="meta" style={{ width: 74, flex: "none" }}>
                  {entry.category}
                </span>
                <span className="truncate grow mono">{entry.label}</span>
                {entry.hint && (
                  <span className="meta truncate" style={{ maxWidth: 220 }}>
                    {entry.hint}
                  </span>
                )}
              </button>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * Rank entries against a query.
 *
 * Subsequence matching so "mcp" finds "Mission Control Pane", with exact prefix
 * and word-boundary matches promoted. Kept simple and stable on purpose.
 */
function rank(entries: Entry[], query: string): Entry[] {
  const q = query.trim().toLowerCase();
  if (!q) return entries;

  const scored: { entry: Entry; score: number }[] = [];
  for (const entry of entries) {
    const haystack = `${entry.label} ${entry.category} ${entry.hint ?? ""}`.toLowerCase();
    const label = entry.label.toLowerCase();

    let score = -1;
    if (label.startsWith(q)) score = 1000;
    else if (label.includes(q)) score = 700;
    else if (haystack.includes(q)) score = 400;
    else if (isSubsequence(q, label)) score = 200;
    else if (isSubsequence(q, haystack)) score = 100;

    if (score > 0) {
      // Shorter labels win ties: they are usually the more general action.
      scored.push({ entry, score: score - Math.min(label.length, 99) / 100 });
    }
  }
  scored.sort((a, b) => b.score - a.score);
  return scored.map((s) => s.entry);
}

/**
 * Quote a path for a shell command line.
 *
 * A path with a space typed unquoted becomes two arguments, which is a confusing
 * failure when it happens by way of a completion the user did not type.
 */
function shellQuote(path: string): string {
  return /[^\w./@-]/.test(path) ? `'${path.replace(/'/g, "'\\''")}'` : path;
}

function isSubsequence(needle: string, haystack: string): boolean {
  let i = 0;
  for (const ch of haystack) {
    if (ch === needle[i]) i++;
    if (i === needle.length) return true;
  }
  return needle.length === 0;
}
