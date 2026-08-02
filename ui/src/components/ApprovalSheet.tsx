/**
 * The approval sheet.
 *
 * Shows everything an approval must show: the exact action, where it will run,
 * why it is being asked about, its risk level and categories, its predicted side
 * effects, and — critically — whether a decision here can actually stop it.
 *
 * When `interceptable` is false the sheet says so in plain language and labels
 * the button "Acknowledge" rather than "Approve". Presenting an un-enforceable
 * observation as a gate would teach users to trust something that is not there,
 * which is worse than showing no gate at all.
 */

import { useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import { describeError, useWorkspace } from "../lib/store";

export function ApprovalSheet() {
  const s = useWorkspace();
  const request = s.pendingApprovals[0];
  const [edited, setEdited] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const dialogRef = useRef<HTMLDivElement | null>(null);

  // Take the keyboard as soon as the sheet appears. This is the one dialog where
  // leaking focus is dangerous rather than annoying: with focus left in the pane
  // underneath, `Return` would run a command in the shell instead of answering the
  // question about running a command.
  useEffect(() => {
    if (request) dialogRef.current?.focus();
  }, [request?.id]);

  if (!request) return null;

  const risk = request.risk;
  const tone = risk.level === "critical" ? "red" : risk.level === "low" ? "muted" : "amber";
  const command = edited ?? request.action;
  // Bound outside the closure: a hoisted function body does not keep the
  // narrowing from the early return above.
  const requestId = request.id;

  async function resolve(outcome: Record<string, unknown>) {
    setBusy(true);
    try {
      await api.rulesResolve(requestId, outcome);
      setEdited(null);
      await s.refreshApprovals();
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      ref={dialogRef}
      role="dialog"
      aria-modal="true"
      aria-label="Approval required"
      // Focusable so the sheet itself can hold the keyboard; never in the tab
      // order, so Tab still walks the real controls.
      tabIndex={-1}
      style={{
        position: "fixed",
        inset: 0,
        background: "color-mix(in srgb, var(--tervin-bg) 72%, transparent)",
        display: "grid",
        placeItems: "center",
        padding: "var(--sp-6)",
        zIndex: 100,
      }}
    >
      <div
        className="col"
        style={{
          width: "min(620px, 100%)",
          background: "var(--tervin-raised)",
          border: `1px solid var(--tervin-${tone === "muted" ? "line" : tone})`,
          borderRadius: "var(--radius-lg)",
          maxHeight: "80vh",
          overflow: "auto",
        }}
      >
        <div
          className="row"
          style={{
            padding: "var(--sp-3) var(--sp-4)",
            borderBottom: "1px solid var(--tervin-line)",
            gap: "var(--sp-2)",
          }}
        >
          <span className={`dot dot-${tone}`} />
          <strong style={{ fontSize: "var(--text-title)" }}>
            {request.interceptable ? "Approval required" : "Action observed"}
          </strong>
          <div className="grow" />
          <span className={`chip tone-${tone}`}>{risk.level} risk</span>
          {s.pendingApprovals.length > 1 && (
            <span className="meta tabular">1 of {s.pendingApprovals.length}</span>
          )}
        </div>

        <div style={{ padding: "var(--sp-4)" }}>
          {/* The exact action, editable before running. */}
          <label className="meta" htmlFor="approval-action">
            {request.kind === "command" ? "Command" : request.kind.replace(/_/g, " ")}
          </label>
          <textarea
            id="approval-action"
            className="mono selectable"
            value={command}
            onChange={(e) => setEdited(e.target.value)}
            rows={Math.min(6, command.split("\n").length + 1)}
            style={{ width: "100%", marginTop: "var(--sp-1)", resize: "vertical" }}
          />

          <div className="row meta" style={{ gap: "var(--sp-4)", marginTop: "var(--sp-2)", flexWrap: "wrap" }}>
            <span title="Working directory">{request.cwd}</span>
            <span title="Host">{request.host}</span>
            <span title="Who requested this">{request.actor}</span>
          </div>

          {/* Why it is being asked about. */}
          <Section title="Why you are being asked">
            <div className="selectable">{request.reason}</div>
            {risk.reasons.map((r) => (
              <div key={r} className="meta">· {r}</div>
            ))}
          </Section>

          {risk.categories.length > 0 && (
            <Section title="What it affects">
              <div className="row" style={{ gap: "var(--sp-1)", flexWrap: "wrap" }}>
                {risk.categories.map((c) => (
                  <span key={c} className={`chip tone-${tone}`}>
                    {c.replace(/_/g, " ")}
                  </span>
                ))}
              </div>
            </Section>
          )}

          {risk.side_effects.length > 0 && (
            <Section title="Expected side effects">
              {risk.side_effects.map((r) => (
                <div key={r} className="meta selectable">→ {r}</div>
              ))}
            </Section>
          )}

          {/* The honesty disclosure. */}
          {!request.interceptable && (
            <div
              style={{
                marginTop: "var(--sp-3)",
                padding: "var(--sp-3)",
                border: "1px solid var(--tervin-amber)",
                borderRadius: "var(--radius-sm)",
              }}
            >
              <div className="row" style={{ gap: "var(--sp-2)", alignItems: "flex-start" }}>
                <span className="dot dot-amber" style={{ marginTop: 5 }} />
                <div className="meta selectable">
                  <strong>Tervin cannot block this action.</strong> It is run by the
                  agent's own runtime, which decides its own permissions. Tervin
                  classified it and recorded it, and can stop the whole Thread — but
                  choosing "Deny" here does not prevent this particular action. This
                  is not a sandbox.
                </div>
              </div>
            </div>
          )}

          {risk.matched_rule && (
            <div className="meta" style={{ marginTop: "var(--sp-2)" }}>
              Matched rule: {risk.matched_rule}
            </div>
          )}
        </div>

        {/* Decisions. */}
        <div
          className="row"
          style={{
            padding: "var(--sp-3) var(--sp-4)",
            borderTop: "1px solid var(--tervin-line)",
            gap: "var(--sp-2)",
            flexWrap: "wrap",
          }}
        >
          <button
            className="btn btn-danger"
            disabled={busy}
            onClick={() => void resolve({ outcome: "deny", reason: "Denied by the user." })}
          >
            Deny
          </button>

          <div className="grow" />

          {edited !== null && edited !== request.action ? (
            <button
              className="btn btn-primary"
              disabled={busy}
              title="The edited command is re-checked against Tervin Rules before it runs"
              onClick={() => void resolve({ outcome: "edit_and_run", command: edited })}
            >
              Run edited command
            </button>
          ) : (
            <>
              {request.available_scopes.some((sc) => sc.scope === "task") && (
                <button
                  className="btn"
                  disabled={busy}
                  onClick={() => {
                    const scope = request.available_scopes.find((sc) => sc.scope === "task");
                    void resolve({ outcome: "approve", scope });
                  }}
                >
                  {request.interceptable ? "Approve for this task" : "Acknowledge for this task"}
                </button>
              )}
              <button
                className="btn"
                disabled={busy}
                title="Applies to this exact action only, not to similar ones"
                onClick={() => void resolve({ outcome: "approve", scope: { scope: "workspace" } })}
              >
                Approve for this workspace
              </button>
              <button
                className="btn btn-primary"
                disabled={busy}
                onClick={() => void resolve({ outcome: "approve", scope: { scope: "once" } })}
              >
                {request.interceptable ? "Approve once" : "Acknowledge"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div style={{ marginTop: "var(--sp-3)" }}>
      <div className="meta" style={{ marginBottom: "var(--sp-1)", color: "var(--tervin-muted)" }}>
        {title}
      </div>
      {children}
    </div>
  );
}
