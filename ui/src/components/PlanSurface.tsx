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
        {/* Not "<title> has not proposed a plan": a title is derived from the first
            prompt, so it is arbitrary user text and reads as gibberish mid-sentence.
            The Thread is already named in the header above this panel. */}
        This Thread has not proposed a plan.
        {thread.capabilities?.plan_mode.level === "supported" ? (
          <>
            {" "}
            This runtime supports plan mode, but a plan only appears if the Thread
            was <strong>started</strong> in it: an agent proposes one by calling
            `ExitPlanMode`, and it only does that when planning was the mode it
            began with. Switching mode now will not produce one. Start a new Thread
            with <strong>Start mode: Plan</strong> in the composer.
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

  /**
   * Tell the agent to proceed as planned.
   *
   * Plan mode stops the agent and waits, and until now nothing in this surface
   * said so or offered a way to release it. The only control sent a *revision* and
   * was disabled until a step had been edited, so agreeing with a plan left you
   * with no button at all and no indication that the Thread was blocked on you.
   *
   * Sent as a turn rather than as a protocol approval, because that is what the
   * runtime is actually waiting for and it keeps the record honest: the transcript
   * shows what was said, not an approval Tervin invented.
   */
  async function approve() {
    const live = s.activeThreadId ? s.threads[s.activeThreadId] : null;
    if (!live?.info?.running) return;
    setBusy(true);
    try {
      await api.threadSend(live.id, "Approved. Go ahead with this plan.", []);
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  /**
   * Whether the agent has actually stopped and is waiting on this plan.
   *
   * `running` is not the question. A Thread that proposed a plan and then carried
   * on editing is running *and* past the decision, so "Approve and continue" sent
   * a turn into a live run and appeared to do nothing — which is exactly what was
   * reported. Plan mode stops the agent, and that stop is the state to test.
   */
  const awaitingDecision =
    thread.info?.running === true && thread.state === "waiting_for_permission";

  const guidance = !thread.info?.running
    ? "This Thread has ended. The plan is kept as a record."
    : awaitingDecision
      ? "The agent is waiting. Approve, revise the steps, or hand off from the Thread header."
      : "The agent has moved on from this plan and is working. Nothing to approve.";

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
            {/* The agent wrote a plan; the parser only recognises bullets and
                numbered lines. When it recognises none, the panel used to render
                nothing at all — an empty column under a heading saying a plan had
                been proposed. Showing the text as written is strictly better than
                showing that the parse failed. */}
            {steps.length === 0 && proposed.rawText && (
              <div className="col" style={{ gap: "var(--sp-2)", padding: "var(--sp-3)" }}>
                <span className="meta">
                  This plan is not written as a list, so there are no steps to
                  reorder. It is shown as the agent wrote it.
                </span>
                <pre
                  className="selectable"
                  style={{ whiteSpace: "pre-wrap", margin: 0, textWrap: "pretty" }}
                >
                  {proposed.rawText}
                </pre>
              </div>
            )}
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
            {/* The plan is a decision point, so the decisions are the controls.
                Previously the only button sent a *revision* and was disabled until
                something had been edited — so a plan you simply agreed with offered
                nothing to press, and the surface gave no clue what happened next. */}
            <button
              className="btn btn-primary"
              disabled={busy || !awaitingDecision}
              onClick={() => void approve()}
              title={
                awaitingDecision
                  ? "Tell the agent to go ahead with this plan"
                  : "The agent is not waiting on a decision right now"
              }
            >
              {busy ? "Sending…" : "Approve and continue"}
            </button>
            <button
              className="btn"
              disabled={busy || !edited || !thread.info?.running}
              onClick={() => void sendRevision()}
              title={
                edited
                  ? "Send the reordered plan back to the agent"
                  : "Reorder, edit or skip a step first"
              }
            >
              {busy ? "Sending…" : "Send revised plan"}
            </button>
            {/* One short line. The previous wording was three sentences in a column
                narrow enough to break them into single words, which is its own way
                of saying nothing. */}
            <span className="meta grow truncate" title={guidance}>
              {guidance}
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
