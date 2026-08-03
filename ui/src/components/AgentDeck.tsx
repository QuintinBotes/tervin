/**
 * Tervin Deck: every agent and what it is doing.
 *
 * One row per Thread, ordered so anything waiting on the user comes first. The
 * point of the Deck is that background work stays visible without taking over
 * the workspace, so rows are dense and carry no decoration.
 */

import * as api from "../lib/api";
import { useWorkspace } from "../lib/store";
import { toneForState } from "../App";

export function AgentDeck() {
  const s = useWorkspace();
  const threads = Object.values(s.threads).sort((a, b) => rank(a.state) - rank(b.state));

  /**
   * Starting a Thread was previously implicit: type into the composer while
   * nothing is running, and the Send button quietly reads "Start Thread" instead.
   * That is undiscoverable, and worse, unreachable — with a Thread selected and
   * running there was no way back to "none", so a second Thread could not be
   * started at all while the first was working.
   */
  const newThread = (
    <div className="row" style={{ padding: "var(--sp-2) var(--sp-3)", gap: "var(--sp-2)" }}>
      <button
        className="btn btn-xs"
        onClick={() => {
          s.startNewThread();
          s.setInspectorTab("thread");
        }}
        title="Start a Thread, leaving any running one to carry on (⌘⇧I)"
      >
        New Thread
      </button>
      <div className="grow" />
      {s.activeThreadId === null && <span className="meta">composer is ready</span>}
    </div>
  );

  if (threads.length === 0) {
    return (
      <div className="col" style={{ minHeight: 0 }}>
        {newThread}
        <div className="empty">
          No agent Threads yet. When several are running, this shows each one's
          purpose, state, current action, and whether it needs you.
        </div>
      </div>
    );
  }

  return (
    <div className="col" style={{ minHeight: 0 }}>
      {newThread}
      {threads.map((t) => {
        const last = [...t.events].reverse().find((e) => e.payload.type !== "thread.state");
        const profile = s.agents?.profiles.find((p) => p.id === t.profileId);
        return (
          <button
            key={t.id}
            className="row"
            onClick={() => {
              s.setActiveThread(t.id);
              s.setInspectorTab("thread");
            }}
            style={{
              width: "100%",
              padding: "var(--sp-2) var(--sp-3)",
              borderBottom: "1px solid var(--tervin-line)",
              gap: "var(--sp-2)",
              textAlign: "left",
              background: t.id === s.activeThreadId ? "var(--tervin-raised)" : "transparent",
            }}
          >
            <span className={`dot dot-${toneForState(t.state)}`} />
            <span className="truncate" style={{ width: 150, flex: "none" }} title={t.title}>
              {t.title}
            </span>
            <span className="meta" style={{ width: 118, flex: "none" }}>
              {profile?.name ?? t.runtimeId}
            </span>
            <span className={`meta tone-${toneForState(t.state)}`} style={{ width: 132, flex: "none" }}>
              {t.state.replace(/_/g, " ")}
            </span>
            <span className="meta truncate grow">{last?.summary ?? "—"}</span>
            {t.info?.permissions && !t.info.permissions.tervin_can_intercept && (
              <span className="chip meta" title={t.info.permissions.explanation}>
                provider-gated
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}

/** Anything blocked on the user sorts to the top. */
function rank(state: api.ThreadState): number {
  if (["waiting_for_permission", "review_required", "awaiting_input"].includes(state)) return 0;
  if (["failed", "disconnected", "interrupted"].includes(state)) return 1;
  if (state === "completed") return 3;
  return 2;
}
