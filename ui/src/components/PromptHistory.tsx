/**
 * Prompt history.
 *
 * The thing nothing else keeps. A shell records what you typed; no agent records what
 * you *asked* it in a form you can search. A session ends and the conversation goes with
 * it — so "what did I ask about the auth timeout last week" has no answer anywhere,
 * which is the specific frustration this exists for.
 *
 * Searches user prompts and agent replies over SQLite FTS5. Reasoning passages are
 * excluded by the indexer: they are long, model-specific, and would bury the thing you
 * actually wrote under a model's thinking about it.
 *
 * ## Retention is visible, and asymmetric
 *
 * Agent history is kept for a window — thirty days by default — and the control for it
 * lives next to the results rather than buried in Settings, because a search that
 * silently cannot see beyond a month is worse than one that says where it stops.
 *
 * Blocks are never pruned, and the panel says so. A command and its output are small and
 * stay useful for years; a transcript is large and stops being useful quickly. Treating
 * them the same would throw away the valuable half to save the expensive one.
 */

import { useCallback, useEffect, useState } from "react";
import * as api from "../lib/api";
import { describeError, useWorkspace } from "../lib/store";

export function PromptHistory() {
  const s = useWorkspace();
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<api.PromptHit[]>([]);
  const [loading, setLoading] = useState(false);
  const [retention, setRetention] = useState<api.RetentionInfo | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);

  const search = useCallback((text: string) => {
    setLoading(true);
    api
      .promptsSearch(text, 200)
      .then(setHits)
      .catch((e) => useWorkspace.getState().pushNotice(describeError(e)))
      .finally(() => setLoading(false));
  }, []);

  // Debounced, so typing does not run a query per keystroke. An empty box shows the
  // most recent rather than everything, which is the useful default when you are
  // looking for something you did earlier today.
  useEffect(() => {
    const handle = setTimeout(() => search(query.trim()), 140);
    return () => clearTimeout(handle);
  }, [query, search]);

  useEffect(() => {
    void api.historyRetention().then(setRetention).catch(() => {});
  }, []);

  async function changeRetention(days: number) {
    try {
      const removed = await api.historySetRetention(days);
      setRetention(await api.historyRetention());
      // Says what was deleted. Silently discarding history on a settings change is the
      // kind of thing people discover much later and cannot undo.
      s.pushNotice(
        removed > 0
          ? `Kept the last ${days} days of agent history and removed ${removed} older event(s). Blocks were not touched.`
          : days === 0
            ? "Agent history will be kept indefinitely."
            : `Agent history is kept for ${days} days. Nothing was old enough to remove.`,
      );
      search(query.trim());
    } catch (e) {
      s.pushNotice(describeError(e));
    }
  }

  return (
    <div className="col" style={{ minHeight: 0, height: "100%", width: "100%" }}>
      <div className="panel-header">
        <span className="label">Prompts</span>
        <span className="meta truncate grow">
          {loading
            ? "Searching…"
            : hits.length === 0
              ? query
                ? "No matches"
                : "Nothing recorded yet"
              : `${hits.length} result${hits.length === 1 ? "" : "s"}`}
        </span>
      </div>

      <div style={{ flex: "none", padding: "var(--sp-2) var(--sp-3)" }}>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search what you asked, and what agents answered"
          aria-label="Search prompts"
          style={{ width: "100%" }}
          spellCheck={false}
        />
      </div>

      <div className="grow" style={{ overflow: "auto", minHeight: 0 }}>
        {hits.length === 0 && !loading ? (
          <div className="empty">
            {query ? (
              <>Nothing matches “{query}”. Search covers your prompts and what agents replied.</>
            ) : (
              <>
                No agent prompts recorded yet. Every Thread's prompts and replies are kept
                here and searchable — which is the part a shell's history cannot do.
              </>
            )}
          </div>
        ) : (
          hits.map((hit) => {
            const mine = hit.kind === "user.prompted";
            const open = expanded === hit.event_id;
            return (
              <div
                key={hit.event_id}
                className="block-row"
                onClick={() => setExpanded(open ? null : hit.event_id)}
                role="button"
                tabIndex={0}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    setExpanded(open ? null : hit.event_id);
                  }
                }}
                style={{ cursor: "pointer" }}
              >
                <div
                  className="row"
                  style={{ padding: "7px 14px 8px 12px", gap: 9, alignItems: "baseline" }}
                >
                  {/* Who said it, in one glance. Your own prompts are the ones people
                      are usually looking for, so they get the accent. */}
                  <span
                    className="meta tabular"
                    style={{
                      width: 44,
                      flex: "0 0 44px",
                      color: mine ? "var(--tervin-accent)" : "var(--tervin-muted)",
                    }}
                  >
                    {mine ? "you" : "agent"}
                  </span>
                  <span
                    className="grow"
                    style={{
                      fontSize: "var(--text-meta)",
                      color: mine ? "var(--tervin-ink)" : "var(--tervin-ink-2)",
                      // Collapsed rows stay one line so a list of forty is scannable;
                      // the whole text is one click away.
                      display: "-webkit-box",
                      WebkitLineClamp: open ? "unset" : 2,
                      WebkitBoxOrient: "vertical",
                      overflow: open ? "visible" : "hidden",
                      whiteSpace: open ? "pre-wrap" : undefined,
                      wordBreak: "break-word",
                    }}
                  >
                    {hit.text}
                  </span>
                  {hit.project && (
                    <span className="chip" title="Project this was asked in">
                      {hit.project}
                    </span>
                  )}
                  <span className="meta tabular" style={{ flex: "none", whiteSpace: "nowrap" }}>
                    {formatWhen(hit.ts)}
                  </span>
                </div>

                {open && (
                  <div
                    className="row"
                    style={{
                      padding: "0 14px 9px 66px",
                      gap: "var(--sp-2)",
                      flexWrap: "wrap",
                    }}
                  >
                    <span className="meta">{hit.runtime_id}</span>
                    <span className="meta">{new Date(hit.ts).toLocaleString()}</span>
                    <button
                      className="btn btn-xs"
                      onClick={(e) => {
                        e.stopPropagation();
                        void navigator.clipboard.writeText(hit.text);
                      }}
                    >
                      Copy
                    </button>
                    <button
                      className="btn btn-xs"
                      title="Send this to the agent again"
                      onClick={(e) => {
                        e.stopPropagation();
                        // Loaded into the composer, not sent: a prompt written for a
                        // different state of the repository usually needs a word changed.
                        s.setHandoff(hit.text);
                        s.setSurface("agents");
                      }}
                    >
                      Reuse
                    </button>
                    {hit.thread_id && (
                      <button
                        className="btn btn-xs"
                        onClick={(e) => {
                          e.stopPropagation();
                          // Loaded from the event store: the Thread may have ended long
                          // ago and only exist on disk.
                          void s
                            .openStoredThread(hit.thread_id!)
                            .then(() => s.setSurface("agents"))
                            .catch((err) => s.pushNotice(describeError(err)));
                        }}
                      >
                        Open Thread
                      </button>
                    )}
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>

      {retention && (
        <div
          className="col"
          style={{
            flex: "none",
            padding: "var(--sp-2) var(--sp-3)",
            borderTop: "1px solid var(--tervin-line)",
            gap: "var(--sp-1)",
          }}
        >
          <div className="row" style={{ gap: "var(--sp-2)", flexWrap: "wrap" }}>
            <span className="meta" style={{ flex: "none" }}>
              Keep agent history for
            </span>
            {[7, 30, 90, 365, 0].map((days) => (
              <button
                key={days}
                className="btn btn-xs"
                onClick={() => void changeRetention(days)}
                aria-pressed={retention.days === days}
                title={
                  days === 0
                    ? "Never delete agent history"
                    : `Delete agent events older than ${days} days`
                }
                style={{
                  borderColor: retention.days === days ? "var(--tervin-accent)" : undefined,
                  color: retention.days === days ? "var(--tervin-accent)" : undefined,
                }}
              >
                {days === 0 ? "forever" : `${days}d`}
              </button>
            ))}
          </div>
          <span className="meta" style={{ textWrap: "pretty" }}>
            {/* The asymmetry is deliberate and worth stating: people assume a retention
                setting applies to everything. */}
            Commands and their output are never deleted — only agent transcripts, which
            are large and stop being useful quickly.
          </span>
        </div>
      )}
    </div>
  );
}

/**
 * A timestamp for scanning rather than for reading precisely.
 *
 * "3d" beats a date when the question is "was this recent"; the exact time is in the
 * expanded row for when it is not.
 */
function formatWhen(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const minutes = Math.floor((Date.now() - then) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d`;
  return new Date(iso).toLocaleDateString([], { month: "short", day: "numeric" });
}
