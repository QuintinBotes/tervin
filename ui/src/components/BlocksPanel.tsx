/**
 * Tervin Blocks.
 *
 * Rows are quiet by default: type, spacing, and a small status marker rather
 * than a card per command. Colour appears only on the status dot and on real
 * failures, so a screen of ordinary work stays calm and a failure is findable.
 *
 * Rows render from `BlockSummary`, which carries a bounded preview. Full output
 * is fetched only when a row is expanded — a history list must never drag a
 * quarter-megabyte per row across the IPC boundary.
 */

import { useMemo, useState } from "react";
import * as api from "../lib/api";
import { useWorkspace } from "../lib/store";
import { writeToPane } from "./TerminalPane";

interface Props {
  /**
   * Client-side narrowing to failures.
   *
   * Separate from the store filter on purpose: the caller has usually already asked
   * the backend for failures, and this keeps the rendered set consistent with the
   * toggle even before the query returns.
   */
  failuresOnly?: boolean;
}

/**
 * Renders Blocks. Owns no query.
 *
 * This panel deliberately does **not** call `refreshBlocks`. It used to, while also
 * being embedded in a surface that filtered — so two components wrote `blockFilter`
 * and each re-triggered the other. Whoever mounts this owns the query; the panel
 * renders whatever is in the store.
 */
export function BlocksPanel({ failuresOnly = false }: Props) {
  const s = useWorkspace();
  const [expanded, setExpanded] = useState<string | null>(null);
  const [fullOutput, setFullOutput] = useState<Record<string, string>>({});

  const blocks = useMemo(
    () => (failuresOnly ? s.blocks.filter((b) => b.status === "failed" || b.error_count > 0) : s.blocks),
    [s.blocks, failuresOnly],
  );

  async function expand(block: api.BlockSummary) {
    if (expanded === block.id) {
      setExpanded(null);
      return;
    }
    setExpanded(block.id);
    if (!fullOutput[block.id]) {
      try {
        const text = await api.blockOutput(block.id);
        setFullOutput((prev) => ({ ...prev, [block.id]: text }));
      } catch {
        // The spill file may have been cleaned up; the preview still shows.
      }
    }
  }

  return (
    <div className="col" style={{ minHeight: 0, height: "100%" }}>
      <div className="grow" style={{ overflow: "auto", minHeight: 0 }}>
        {blocks.length === 0 ? (
          <div className="empty">
            No Blocks yet. A Block is created for each command you run, once shell
            integration is installed — Settings → Shell integration shows the one
            line it adds and why.
          </div>
        ) : (
          blocks.map((block) => (
            <BlockRow
              key={block.id}
              block={block}
              expanded={expanded === block.id}
              output={fullOutput[block.id]}
              onToggle={() => void expand(block)}
            />
          ))
        )}
      </div>
    </div>
  );
}

function BlockRow({
  block,
  expanded,
  output,
  onToggle,
}: {
  block: api.BlockSummary;
  expanded: boolean;
  output: string | undefined;
  onToggle: () => void;
}) {
  const s = useWorkspace();
  const tone = toneForStatus(block.status);

  return (
    <div className={`block-row${expanded ? " block-row-open" : ""}`}>
      <div
        className="row"
        // 9px 18px 11px 15px, per the design system. A Block is not a card: no
        // border, no background at rest — the hover state is the only affordance.
        style={{
          padding: "9px 18px 11px 15px",
          gap: 9,
          cursor: "pointer",
          alignItems: "baseline",
        }}
        onClick={onToggle}
        role="button"
        tabIndex={0}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
      >
        <span
          className={`dot dot-${tone}`}
          title={block.status}
          style={{ transform: "translateY(-1px)" }}
        />
        {/* One truncated line, never wrapped. `wordBreak: break-word` with a flex
            `min-width: 0` lets this shrink to almost nothing when the siblings claim
            their width, and in a narrow pane the result is monospace text wrapping
            one character per line. The full command is in the tooltip and in the
            expanded body below, so nothing is lost by clipping here. */}
        <code
          className="mono truncate grow"
          title={block.command || undefined}
          style={{ fontSize: "var(--text-control)" }}
        >
          {block.command || "(no command recorded)"}
        </code>

        {block.tests && (
          <span className={`chip tabular tone-${block.tests.failed > 0 ? "red" : "green"}`}>
            {block.tests.failed > 0
              ? `${block.tests.failed} failing`
              : `${block.tests.passed} passed`}
          </span>
        )}
        {block.error_count > 0 && (
          <span className="chip tone-red tabular">{block.error_count} errors</span>
        )}
        {/* Who ran it. In a mixed list this is the difference between "I did that" and
            "an agent did that", which changes how a failure is read. */}
        {block.thread_id !== null && (
          <span className="chip" title="Run by an agent, not typed in a pane">
            agent
          </span>
        )}
        {block.ports.length > 0 && (
          <span className="chip tabular" title="Ports mentioned in output">
            :{block.ports[0]}
          </span>
        )}
        <span
          className="mono dim tabular"
          style={{ whiteSpace: "nowrap", flex: "0 0 auto", fontSize: "var(--text-meta)" }}
        >
          {[
            block.exit_code !== null && block.exit_code !== 0
              ? `exit ${block.exit_code}`
              : null,
            block.duration_ms !== null ? formatDuration(block.duration_ms) : null,
            new Date(block.started_at).toLocaleTimeString([], {
              hour: "2-digit",
              minute: "2-digit",
              hour12: false,
            }),
          ]
            .filter(Boolean)
            .join(" · ")}
        </span>
        {/* Actions appear on hover, never as a permanent toolbar. */}
        <button
          className="btn btn-xs btn-ghost block-action"
          title={block.bookmarked ? "Remove bookmark" : "Bookmark"}
          onClick={(e) => {
            e.stopPropagation();
            void api
              .blockSetBookmark(block.id, !block.bookmarked)
              .then(() => s.refreshBlocks());
          }}
          style={{
            color: block.bookmarked ? "var(--tervin-accent)" : undefined,
            opacity: block.bookmarked ? 1 : undefined,
          }}
        >
          {block.bookmarked ? "Bookmarked" : "Bookmark"}
        </button>
      </div>

      {expanded && (
        <div style={{ padding: "0 var(--sp-3) var(--sp-3)" }}>
          <div className="row meta" style={{ gap: "var(--sp-3)", flexWrap: "wrap" }}>
            <span className="tabular">{new Date(block.started_at).toLocaleString()}</span>
            <span className="truncate" title={block.cwd}>{block.cwd}</span>
            {block.git_branch && <span>{block.git_branch}</span>}
            {block.exit_code !== null ? (
              <span className="tabular">exit {block.exit_code}</span>
            ) : (
              block.thread_id !== null &&
              block.status !== "running" && (
                // An agent ran this, and its runtime reported success or failure without
                // a status. Saying so is better than an absent field someone reads as a
                // rendering bug — and far better than a number nobody reported.
                <span title="The runtime reported the outcome but not an exit status">
                  no exit status reported
                </span>
              )
            )}
            <span className="tabular">{formatBytes(block.output_total)}</span>
            {block.output_truncated && (
              <span className="tone-amber">
                {block.thread_id !== null
                  ? // A different reason from a shell Block's, and the wrong one would
                    // send someone looking for a capture setting that is not involved.
                    "excerpt only — the runtime reports a bounded sample, not the full log"
                  : "output truncated at the capture limit"}
              </span>
            )}
          </div>

          <pre
            className="mono selectable"
            style={{
              margin: "var(--sp-2) 0 0 16px",
              padding: "var(--sp-4)",
              background: "var(--tervin-bg)",
              border: "1px solid var(--tervin-line)",
              borderRadius: "var(--radius-control)",
              color: "var(--tervin-ink-2)",
              maxHeight: 320,
              overflow: "auto",
              fontSize: "var(--font-mono-size)",
              lineHeight: "var(--font-mono-line-height)",
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
            }}
          >
            {output ?? block.preview}
          </pre>

          <div className="row" style={{ marginTop: "var(--sp-2)", gap: "var(--sp-2)", flexWrap: "wrap" }}>
            <button
              className="btn"
              onClick={() => void navigator.clipboard.writeText(block.command)}
            >
              Copy command
            </button>
            <button
              className="btn"
              onClick={() => void navigator.clipboard.writeText(output ?? block.preview)}
            >
              Copy output
            </button>
            <button
              className="btn"
              title="Type this command into the focused pane, without running it"
              onClick={() => {
                const tab = s.tabs.find((t) => t.id === s.activeTabId);
                if (tab?.activePaneId) writeToPane(tab.activePaneId, block.command);
              }}
            >
              Re-run
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

export function toneForStatus(status: api.BlockStatus): string {
  switch (status) {
    case "succeeded":
      return "green";
    case "failed":
      return "red";
    case "interrupted":
      return "amber";
    case "running":
      return "teal";
    default:
      return "muted";
  }
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${minutes}m ${seconds}s`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
