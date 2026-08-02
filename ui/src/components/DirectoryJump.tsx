/**
 * Jump to a directory you have actually been in.
 *
 * `cd` is the most-typed command in a terminal and the one with the least help. Shell
 * completion can only offer what is *below* where you already are, so getting from
 * `~/Projects/tervin/ui/src` to a sibling repository means typing the whole path or
 * installing `z`. This is that, built in and without a shell plugin.
 *
 * ## Ranking
 *
 * Two signals combined, not chosen between: how well the path matches what was typed,
 * and frecency — visits discounted by age. Matching alone puts a directory visited once
 * above the one you live in; frecency alone ignores what you typed. So an empty box shows
 * "where I usually am" and a typed one shows "the thing I mean".
 *
 * ## It types rather than executes
 *
 * Accepting a directory writes `cd <path>` into the pane and leaves the newline to the
 * user. Running a command in someone's shell because they pressed Enter in a picker is a
 * different and more surprising thing than filling it in for them — and if the directory
 * is wrong, an unsent line is trivially fixed.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import { describeError, useWorkspace } from "../lib/store";
import { writeToPane } from "./TerminalPane";

/** Shell-quote a path, for the same reason the file explorer does. */
function quoteForShell(path: string): string {
  // A path with no surprises is left bare so the typed line stays readable; anything
  // else is single-quoted, with embedded quotes closed and reopened.
  if (/^[A-Za-z0-9._\-/~]+$/.test(path)) return path;
  return `'${path.replace(/'/g, `'\\''`)}'`;
}

/** "3d", "2h", "now" — enough to judge whether a directory is still current. */
function formatAge(hours: number): string {
  if (hours < 1) return "now";
  if (hours < 24) return `${Math.floor(hours)}h`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d`;
  return `${Math.floor(days / 30)}mo`;
}

export function DirectoryJump() {
  const s = useWorkspace();
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<api.DirSuggestion[]>([]);
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);

  const close = useCallback(() => s.setDirectoryJump(false), [s]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Debounced, and the selection resets on every new result set — leaving it where it
  // was would mean the highlighted row silently becomes a different directory.
  useEffect(() => {
    const handle = setTimeout(() => {
      api
        .recentDirectories(query.trim(), 40)
        .then((rows) => {
          setHits(rows);
          setSelected(0);
        })
        .catch((e) => s.pushNotice(describeError(e)));
    }, 90);
    return () => clearTimeout(handle);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query]);

  function accept(hit: api.DirSuggestion) {
    if (hit.missing) {
      // Offered rather than done silently: the row is the only place the user learns the
      // directory is gone, so removing it under them would look like a lost result.
      void api
        .forgetDirectory(hit.path)
        .then(() => setHits((rows) => rows.filter((r) => r.path !== hit.path)))
        .catch((e) => s.pushNotice(describeError(e)));
      return;
    }

    const tab = s.tabs.find((t) => t.id === s.activeTabId);
    const paneId = tab?.activePaneId;
    if (!paneId) {
      s.pushNotice("There is no pane to change the directory of.");
      return;
    }
    // Typed, not run. The newline is the user's to send.
    writeToPane(paneId, `cd ${quoteForShell(hit.path)}`);
    close();
  }

  function onKeyDown(e: React.KeyboardEvent) {
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
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Jump to a directory"
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
          width: "min(680px, 94vw)",
          maxHeight: "70vh",
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
          onKeyDown={onKeyDown}
          placeholder="Jump to a directory you have been in"
          aria-label="Directory query"
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
                ? `No directory you have visited matches “${query}”.`
                : // The honest empty state: this fills up by using the terminal, and
                  // saying so beats an empty box that looks broken on a fresh install.
                  "Directories appear here as you cd into them."}
            </div>
          ) : (
            hits.map((hit, i) => (
              <div
                key={hit.path}
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
                    i === selected
                      ? "2px solid var(--tervin-accent)"
                      : "2px solid transparent",
                }}
              >
                <div
                  className="row"
                  style={{ padding: "6px 12px", gap: "var(--sp-2)", alignItems: "baseline" }}
                >
                  <span
                    style={{
                      fontSize: "var(--text-body)",
                      color: hit.missing ? "var(--tervin-muted)" : "var(--tervin-ink)",
                      textDecoration: hit.missing ? "line-through" : undefined,
                    }}
                  >
                    {hit.name}
                  </span>
                  <span className="meta truncate grow" title={hit.path}>
                    {hit.path}
                  </span>
                  {hit.missing ? (
                    // Stated, not hidden. A directory that has been deleted is exactly
                    // what someone needs to know before wondering why `cd` failed.
                    <span className="chip" title="This directory no longer exists">
                      gone — click to forget
                    </span>
                  ) : (
                    <>
                      <span className="meta tabular" title={`${hit.visits} visits`}>
                        ×{hit.visits}
                      </span>
                      <span className="meta tabular" style={{ width: 40, textAlign: "right" }}>
                        {formatAge(hit.age_hours)}
                      </span>
                    </>
                  )}
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
            Ranked by how often you go there and how recently — then by what you typed.
            Enter fills in <code>cd</code>; it does not run it.
          </span>
          <button className="btn btn-xs" onClick={close}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
