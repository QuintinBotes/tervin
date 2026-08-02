/**
 * Every command you have run, searchable.
 *
 * A shell's own `Ctrl-R` searches one shell's history: one machine, one session's ancestry,
 * and no idea whether the command worked. Tervin already records every command with its
 * exit status, directory and project, so it can answer the question people actually have,
 * which is "that command I ran last week in the other repo".
 *
 * ## It says when a command failed last time
 *
 * The one piece of information a shell cannot give you, and the one most worth having
 * before pressing Enter on something from a week ago.
 *
 * ## It types rather than runs
 *
 * Same rule as the directory and saved-command pickers. Reusing a command from history is
 * exactly when you want to glance at it first: it may reference a branch that no longer
 * exists or a file that has moved.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import { describeError, useWorkspace } from "../lib/store";
import { writeToPane } from "./TerminalPane";

/** "3d", "2h", "now" — enough to judge whether a command is still current. */
function formatAge(hours: number): string {
  if (hours < 1) return "now";
  if (hours < 24) return `${Math.floor(hours)}h`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d`;
  return `${Math.floor(days / 30)}mo`;
}

export function CommandHistory() {
  const s = useWorkspace();
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<api.CommandSuggestion[]>([]);
  const [selected, setSelected] = useState(0);
  const [thisProject, setThisProject] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);

  const close = useCallback(() => s.setCommandHistory(false), [s]);
  // Derived from the project root's last component, which is what the backend stamps on a
  // Block. There is no separate project field to read.
  const root = s.environment?.project_root ?? "";
  const project = root.split("/").filter(Boolean).pop() ?? null;

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    const handle = setTimeout(() => {
      api
        .commandHistory(query.trim(), thisProject ? project : null, 60)
        .then((rows) => {
          setHits(rows);
          setSelected(0);
        })
        .catch((e) => s.pushNotice(describeError(e)));
    }, 90);
    return () => clearTimeout(handle);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, thisProject, project]);

  function accept(hit: api.CommandSuggestion) {
    const tab = s.tabs.find((t) => t.id === s.activeTabId);
    const paneId = tab?.activePaneId;
    if (!paneId) {
      s.pushNotice("There is no pane to send a command to.");
      return;
    }
    // Typed, not run. A command from last week may name a branch that is gone.
    writeToPane(paneId, hit.command);
    close();
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Command history"
      onClick={close}
      style={{
        position: "fixed",
        inset: 0,
        background: "color-mix(in srgb, var(--tervin-bg) 70%, transparent)",
        display: "grid",
        placeItems: "start center",
        paddingTop: "12vh",
        zIndex: 160,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="col"
        style={{
          width: "min(760px, 94vw)",
          maxHeight: "72vh",
          background: "var(--tervin-panel)",
          border: "1px solid var(--tervin-line)",
          borderRadius: "var(--radius-lg)",
          overflow: "hidden",
        }}
      >
        <input
          ref={inputRef}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || (e.key === "n" && e.ctrlKey)) {
              e.preventDefault();
              setSelected((i) => Math.min(i + 1, Math.max(0, hits.length - 1)));
            } else if (e.key === "ArrowUp" || (e.key === "p" && e.ctrlKey)) {
              e.preventDefault();
              setSelected((i) => Math.max(0, i - 1));
            } else if (e.key === "Enter") {
              e.preventDefault();
              const hit = hits[selected];
              if (hit) accept(hit);
            } else if (e.key === "Escape") {
              e.preventDefault();
              close();
            }
          }}
          placeholder="Search every command you have run"
          aria-label="Search command history"
          spellCheck={false}
          style={{
            border: "none",
            borderBottom: "1px solid var(--tervin-line)",
            borderRadius: 0,
            padding: "var(--sp-3)",
            fontSize: "var(--text-body)",
            background: "transparent",
          }}
        />

        <div className="grow" style={{ overflow: "auto", minHeight: 0 }}>
          {hits.length === 0 ? (
            <div className="empty">
              {query
                ? `No command you have run matches “${query}”.`
                : "Commands appear here as you run them, across every pane and project."}
            </div>
          ) : (
            hits.map((hit, i) => (
              <div
                key={hit.command}
                className="block-row"
                role="button"
                tabIndex={-1}
                aria-selected={i === selected}
                onMouseEnter={() => setSelected(i)}
                onClick={() => accept(hit)}
                style={{
                  cursor: "pointer",
                  background: i === selected ? "var(--tervin-hover)" : undefined,
                  borderLeft:
                    i === selected ? "2px solid var(--tervin-accent)" : "2px solid transparent",
                }}
              >
                <div
                  className="row"
                  style={{ padding: "6px 12px", gap: "var(--sp-2)", alignItems: "baseline" }}
                >
                  <span className="mono truncate grow" title={hit.command}>
                    {hit.command}
                  </span>
                  {hit.failed_last_time && (
                    // The thing a shell cannot tell you, and the thing most worth knowing
                    // before pressing Enter on something from a week ago.
                    <span className="chip tone-amber" title="This failed the last time you ran it">
                      failed last time
                    </span>
                  )}
                  <span className="meta tabular" title={`${hit.uses} runs`}>
                    ×{hit.uses}
                  </span>
                  <span className="meta tabular" style={{ width: 40, textAlign: "right" }}>
                    {formatAge(hit.age_hours)}
                  </span>
                </div>
              </div>
            ))
          )}
        </div>

        <div
          className="row"
          style={{
            flex: "none",
            padding: "var(--sp-2) var(--sp-3)",
            borderTop: "1px solid var(--tervin-line)",
            gap: "var(--sp-2)",
          }}
        >
          <span className="meta grow">
            Enter fills the command into the pane; it does not run it.
          </span>
          {project && (
            <button
              className="btn btn-xs"
              aria-pressed={thisProject}
              onClick={() => setThisProject((v) => !v)}
              title={`Only commands run in ${project}`}
              style={{
                borderColor: thisProject ? "var(--tervin-accent)" : undefined,
                color: thisProject ? "var(--tervin-accent)" : undefined,
              }}
            >
              {project} only
            </button>
          )}
          <button className="btn btn-xs" onClick={close}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
