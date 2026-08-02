/**
 * The Plan surface.
 *
 * A plan is the one moment where a user can change what an agent is about to do
 * while it is still cheap — before any file is written. So it is a real surface
 * with its own column, not a paragraph inside a chat log.
 *
 * The plan shown here is reconstructed from the Thread's own `plan.proposed`
 * events rather than stored separately. That matters: the event stream is
 * append-only and provider-neutral, so the plan a user sees is exactly what the
 * runtime said, and a superseded plan is a new event rather than an edit of an
 * old one. Reviewing a plan that had been quietly rewritten would be worse than
 * having no plan view.
 *
 * Steps can be reordered, edited and skipped locally. Those edits are *proposals
 * back to the agent* — Tervin cannot make a runtime follow a reordered plan, so
 * the panel says so rather than implying it has taken control.
 */

import { useEffect, useMemo, useState } from "react";
import * as api from "../lib/api";
import { describeError, useWorkspace } from "../lib/store";
import { toneForState } from "../App";
import { TwoColumn } from "../App";

interface Step {
  /** Index in the plan as the agent stated it. */
  index: number;
  description: string;
  touches: string[];
  skipped: boolean;
}

export function PlanSurface({ narrow }: { narrow: boolean }) {
  const s = useWorkspace();
  const thread = s.activeThreadId ? s.threads[s.activeThreadId] : null;

  // The latest plan the agent proposed. Append-only history means "latest" is the
  // last event, never a mutated record.
  const proposed = useMemo(() => {
    if (!thread) return null;
    for (let i = thread.events.length - 1; i >= 0; i--) {
      const event = thread.events[i]!;
      if (event.payload.type === "plan.proposed") {
        const payload = event.payload as {
          steps?: { description: string; touches?: string[] }[];
          raw_text?: string | null;
        };
        return {
          at: event.ts,
          steps: payload.steps ?? [],
          rawText: payload.raw_text ?? null,
        };
      }
    }
    return null;
  }, [thread]);

  const [steps, setSteps] = useState<Step[]>([]);
  const [selected, setSelected] = useState(0);
  const [dragging, setDragging] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  // Reset local edits whenever the agent proposes a new plan: keeping stale edits
  // over a superseded plan would show a plan nobody proposed.
  useEffect(() => {
    setSteps(
      (proposed?.steps ?? []).map((step, index) => ({
        index,
        description: step.description,
        touches: step.touches ?? [],
        skipped: false,
      })),
    );
    setSelected(0);
  }, [proposed]);

  const revision = proposed
    ? `proposed ${new Date(proposed.at).toLocaleTimeString([], { hour12: false })}`
    : "";

  if (!thread) {
    return (
      <div className="empty">
        No Thread selected. Start one on the Agents surface; when the agent proposes
        a plan it appears here, with every step and the files it expects to touch,
        before anything is written.
      </div>
    );
  }

  if (!proposed) {
    return (
      <div className="empty">
        <strong>{thread.title}</strong> has not proposed a plan.
        {thread.capabilities?.plan_mode.level === "supported" ? (
          <>
            {" "}
            This runtime supports plan mode — ask it to plan first, or switch the
            Thread to plan mode from the composer.
          </>
        ) : (
          <>
            {" "}
            This runtime does not report plans
            {thread.capabilities?.plan_mode.level === "unsupported" &&
            "reason" in thread.capabilities.plan_mode
              ? `: ${thread.capabilities.plan_mode.reason}`
              : "."}
          </>
        )}
      </div>
    );
  }

  /** Move a step, which is a proposal back to the agent rather than a command. */
  function reorder(from: number, to: number) {
    if (from === to) return;
    setSteps((current) => {
      const next = [...current];
      const [moved] = next.splice(from, 1);
      if (moved) next.splice(to, 0, moved);
      return next;
    });
  }

  const edited =
    steps.some((step, i) => step.index !== i) || steps.some((step) => step.skipped);

  // Bound outside the closure: a hoisted function body does not keep the
  // narrowing from the early returns above.
  const live = thread;

  async function sendRevision() {
    if (!live.info?.running) return;
    setBusy(true);
    try {
      const body = steps
        .filter((step) => !step.skipped)
        .map((step, i) => `${i + 1}. ${step.description}`)
        .join("\n");
      const skipped = steps.filter((step) => step.skipped).map((step) => step.description);

      await api.threadSend(
        live.id,
        [
          "Revised plan. Please follow this order:",
          body,
          skipped.length > 0 ? `\nSkip: ${skipped.join("; ")}` : "",
        ]
          .filter(Boolean)
          .join("\n"),
        [],
      );
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  const active = steps[selected];

  return (
    <TwoColumn
      narrow={narrow}
      listLabel="Steps"
      leftWidth={s.listColumnWidth}
      onResize={s.setListColumnWidth}
      left={
        <div className="col" style={{ minHeight: 0, width: "100%" }}>
          <div className="panel-header">
            <span className="truncate grow" style={{ fontSize: "var(--text-control)" }}>
              {thread.title}
            </span>
            <span className="meta tabular">{revision}</span>
            <span className={`chip chip-${toneForState(thread.state) === "amber" ? "amber" : "teal"}`}>
              {thread.state.replace(/_/g, " ")}
            </span>
          </div>

          <div className="grow" style={{ overflow: "auto", minHeight: 0 }}>
            {steps.map((step, i) => (
              <div
                key={`${step.index}-${i}`}
                draggable
                onDragStart={() => setDragging(i)}
                onDragEnd={() => setDragging(null)}
                onDragOver={(e) => e.preventDefault()}
                onDrop={() => {
                  if (dragging !== null) reorder(dragging, i);
                  setDragging(null);
                }}
                onClick={() => setSelected(i)}
                aria-selected={i === selected}
                className="list-row"
                style={{
                  alignItems: "flex-start",
                  cursor: "grab",
                  opacity: step.skipped ? 0.5 : dragging === i ? 0.4 : 1,
                }}
              >
                <span
                  className="mono dim tabular"
                  style={{ width: 18, flex: "0 0 18px", textAlign: "right" }}
                >
                  {i + 1}
                </span>
                <span
                  className={`dot ${step.skipped ? "dot-muted" : "dot-teal"}`}
                  style={{ marginTop: 5 }}
                />
                <span className="col grow" style={{ gap: 3 }}>
                  <span
                    style={{
                      fontSize: "var(--text-control)",
                      textDecoration: step.skipped ? "line-through" : undefined,
                      textWrap: "pretty",
                    }}
                  >
                    {step.description}
                  </span>
                  {step.touches.length > 0 && (
                    <span className="row" style={{ gap: "var(--sp-1)", flexWrap: "wrap" }}>
                      {step.touches.map((path) => (
                        <span key={path} className="chip mono">
                          {path}
                        </span>
                      ))}
                    </span>
                  )}
                </span>
                <button
                  className="btn btn-xs btn-ghost"
                  onClick={(e) => {
                    e.stopPropagation();
                    setSteps((current) =>
                      current.map((other, j) =>
                        j === i ? { ...other, skipped: !other.skipped } : other,
                      ),
                    );
                  }}
                >
                  {step.skipped ? "Include" : "Skip"}
                </button>
              </div>
            ))}

            <div className="meta" style={{ padding: "var(--sp-4) var(--sp-6)" }}>
              Drag a step to reorder.
            </div>
          </div>

          <div
            className="row"
            style={{
              flex: "none",
              padding: "var(--sp-4) var(--sp-6)",
              borderTop: "1px solid var(--tervin-raised)",
              gap: "var(--sp-2)",
              flexWrap: "wrap",
            }}
          >
            <button
              className="btn btn-primary"
              disabled={busy || !edited || !thread.info?.running}
              onClick={() => void sendRevision()}
              title={
                thread.info?.running
                  ? "Send the revised plan to the agent"
                  : "The Thread is not running"
              }
            >
              {busy ? "Sending…" : "Send revised plan"}
            </button>
            <span className="meta grow" style={{ textWrap: "pretty" }}>
              {/* Never imply Tervin controls the runtime. */}
              Edits here are a proposal. Tervin cannot make a runtime follow a
              reordered plan — it asks.
            </span>
          </div>
        </div>
      }
      right={
        <div className="col" style={{ minHeight: 0, width: "100%" }}>
          <div className="panel-header">
            <span className="label">Step</span>
            <span className="meta truncate grow">
              {active ? active.description : "No step selected"}
            </span>
          </div>

          <div className="grow" style={{ overflow: "auto", minHeight: 0, padding: "var(--sp-6)" }}>
            {!active ? (
              <div className="meta">Select a step to see what it says.</div>
            ) : (
              <>
                <p
                  className="selectable"
                  style={{ margin: 0, fontSize: "var(--text-body)", textWrap: "pretty" }}
                >
                  {active.description}
                </p>

                {active.touches.length > 0 && (
                  <div style={{ marginTop: "var(--sp-6)" }}>
                    <div className="label" style={{ marginBottom: "var(--sp-2)" }}>
                      Expected to touch
                    </div>
                    {active.touches.map((path) => (
                      <div key={path} className="mono meta selectable">
                        {path}
                      </div>
                    ))}
                  </div>
                )}

                {proposed.rawText && (
                  <div style={{ marginTop: "var(--sp-6)" }}>
                    <div className="label" style={{ marginBottom: "var(--sp-2)" }}>
                      As the agent wrote it
                    </div>
                    <pre
                      className="mono selectable"
                      style={{
                        margin: 0,
                        padding: "var(--sp-4)",
                        background: "var(--tervin-bg)",
                        border: "1px solid var(--tervin-line)",
                        borderRadius: "var(--radius-control)",
                        fontSize: "var(--font-mono-size)",
                        whiteSpace: "pre-wrap",
                        wordBreak: "break-word",
                        color: "var(--tervin-ink-2)",
                      }}
                    >
                      {proposed.rawText}
                    </pre>
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      }
    />
  );
}
