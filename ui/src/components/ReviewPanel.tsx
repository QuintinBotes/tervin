/**
 * Tervin Review: diffs, with hunk-level accept and revert.
 *
 * Unified and side-by-side are two renderings of the same parsed hunk data, so a
 * link from a timeline event to an exact hunk resolves identically in both.
 *
 * Reverting is a destructive edit to the working tree, so it is confirmed and
 * always states the rollback path — never applied on a single click.
 */

import { useEffect, useState } from "react";
import * as api from "../lib/api";
import { describeError, useWorkspace } from "../lib/store";

export function ReviewPanel() {
  const s = useWorkspace();
  const [mode, setMode] = useState<api.DiffMode>("working_tree");
  const [side, setSide] = useState(false);
  const [diffs, setDiffs] = useState<api.FileDiff[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  async function load() {
    setLoading(true);
    try {
      const next = await api.gitDiff(mode);
      setDiffs(next);
      setSelected((prev) => (prev && next.some((d) => d.path === prev) ? prev : next[0]?.path ?? null));
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode]);

  const active = diffs.find((d) => d.path === selected);

  return (
    // `width: 100%` and `minWidth: 0` are load-bearing: this renders as a flex item,
    // and a flex item with no width sizes to its content — which left the right-hand
    // half of the surface empty.
    <div className="col" style={{ height: "100%", minHeight: 0, width: "100%", minWidth: 0 }}>
      <div
        className="row"
        style={{
          padding: "var(--sp-2)",
          borderBottom: "1px solid var(--tervin-line)",
          flex: "none",
          gap: "var(--sp-2)",
        }}
      >
        <select value={mode} onChange={(e) => setMode(e.target.value as api.DiffMode)} aria-label="Diff scope">
          <option value="working_tree">All uncommitted</option>
          <option value="unstaged">Unstaged</option>
          <option value="staged">Staged</option>
        </select>
        <button className="btn" onClick={() => setSide((v) => !v)}>
          {side ? "Unified" : "Side by side"}
        </button>
        <div className="grow" />
        <button className="btn btn-ghost" onClick={() => void load()} disabled={loading}>
          {loading ? "…" : "Refresh"}
        </button>
      </div>

      {diffs.length === 0 ? (
        <div className="empty">
          No uncommitted changes. When an agent edits files, the changed-file tree
          and its diffs appear here, and Tervin never commits on your behalf.
        </div>
      ) : (
        <>
          {/* Changed-file tree. */}
          <div style={{ flex: "none", maxHeight: 180, overflow: "auto", borderBottom: "1px solid var(--tervin-line)" }}>
            {diffs.map((d) => (
              <button
                key={d.path}
                className="row"
                onClick={() => setSelected(d.path)}
                style={{
                  width: "100%",
                  padding: "var(--sp-1) var(--sp-3)",
                  gap: "var(--sp-2)",
                  textAlign: "left",
                  background: d.path === selected ? "var(--tervin-raised)" : "transparent",
                }}
              >
                <span className="meta" style={{ width: 12, flex: "none" }} title={d.kind}>
                  {markerFor(d.kind)}
                </span>
                <span className="mono truncate grow" style={{ fontSize: "var(--text-meta)" }}>
                  {d.path}
                </span>
                {d.binary ? (
                  <span className="meta">binary</span>
                ) : (
                  <span className="meta tabular">
                    <span className="tone-green">+{d.added_lines}</span>{" "}
                    <span className="tone-red">−{d.removed_lines}</span>
                  </span>
                )}
              </button>
            ))}
          </div>

          <div className="grow" style={{ overflow: "auto", minHeight: 0 }}>
            {!active ? (
              <div className="empty">Select a file to see its diff.</div>
            ) : active.binary ? (
              <div className="empty">
                {active.path} is a binary file. Tervin shows that it changed rather
                than rendering an empty diff.
              </div>
            ) : (
              active.hunks.map((hunk, i) => (
                <HunkView
                  key={`${active.path}-${i}`}
                  path={active.path}
                  hunk={hunk}
                  index={i}
                  mode={mode}
                  sideBySide={side}
                  onApplied={() => void load()}
                />
              ))
            )}
          </div>
        </>
      )}
    </div>
  );
}

function HunkView({
  path,
  hunk,
  index,
  mode,
  sideBySide,
  onApplied,
}: {
  path: string;
  hunk: api.Hunk;
  index: number;
  mode: api.DiffMode;
  sideBySide: boolean;
  onApplied: () => void;
}) {
  const s = useWorkspace();
  const [confirming, setConfirming] = useState(false);

  async function apply(reverse: boolean) {
    try {
      await api.gitApplyHunks(path, mode, [index], reverse, !reverse);
      onApplied();
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setConfirming(false);
    }
  }

  return (
    <div style={{ borderBottom: "1px solid var(--tervin-line)" }}>
      <div
        className="row meta"
        style={{
          padding: "var(--sp-1) var(--sp-3)",
          background: "var(--tervin-bg)",
          gap: "var(--sp-2)",
        }}
      >
        <code className="mono tabular grow truncate">
          @@ −{hunk.old_start},{hunk.old_lines} +{hunk.new_start},{hunk.new_lines} @@
          {hunk.section ? ` ${hunk.section}` : ""}
        </code>
        <button className="btn btn-ghost" onClick={() => void apply(false)} title="Stage only this hunk">
          Stage hunk
        </button>
        {confirming ? (
          <>
            <span className="tone-amber">Discard these lines from the working tree?</span>
            <button className="btn btn-danger" onClick={() => void apply(true)}>
              Revert
            </button>
            <button className="btn btn-ghost" onClick={() => setConfirming(false)}>
              Cancel
            </button>
          </>
        ) : (
          <button
            className="btn btn-ghost tone-red"
            onClick={() => setConfirming(true)}
            title="Reverting edits the working tree; you will be asked to confirm"
          >
            Revert hunk
          </button>
        )}
      </div>

      {sideBySide ? <SideBySide hunk={hunk} /> : <Unified hunk={hunk} />}
    </div>
  );
}

function Unified({ hunk }: { hunk: api.Hunk }) {
  return (
    <div className="mono selectable" style={{ fontSize: "var(--font-mono-size)" }}>
      {hunk.lines.map((line, i) => (
        <div
          key={i}
          className="row"
          style={{ background: bgFor(line.kind), whiteSpace: "pre", gap: 0 }}
        >
          <span className="meta tabular" style={{ width: 44, flex: "none", textAlign: "right", paddingRight: 6 }}>
            {line.old_lineno ?? ""}
          </span>
          <span className="meta tabular" style={{ width: 44, flex: "none", textAlign: "right", paddingRight: 6 }}>
            {line.new_lineno ?? ""}
          </span>
          <span style={{ width: 12, flex: "none", color: "var(--tervin-muted)" }}>
            {signFor(line.kind)}
          </span>
          <span style={{ overflowX: "auto" }}>{line.content}</span>
        </div>
      ))}
    </div>
  );
}

/**
 * Side-by-side, derived from the same hunk.
 *
 * Removed and added lines are paired positionally so a modified line sits
 * opposite its replacement rather than drifting down the page.
 */
function SideBySide({ hunk }: { hunk: api.Hunk }) {
  const rows: { left?: api.DiffLine; right?: api.DiffLine }[] = [];
  let i = 0;
  while (i < hunk.lines.length) {
    const line = hunk.lines[i]!;
    if (line.kind === "context") {
      rows.push({ left: line, right: line });
      i++;
      continue;
    }
    const removals: api.DiffLine[] = [];
    const additions: api.DiffLine[] = [];
    while (i < hunk.lines.length && hunk.lines[i]!.kind === "removed") removals.push(hunk.lines[i++]!);
    while (i < hunk.lines.length && hunk.lines[i]!.kind === "added") additions.push(hunk.lines[i++]!);
    if (removals.length === 0 && additions.length === 0) i++;
    const pairs = Math.max(removals.length, additions.length);
    for (let p = 0; p < pairs; p++) {
      rows.push({ left: removals[p], right: additions[p] });
    }
  }

  return (
    <div className="mono selectable" style={{ fontSize: "var(--font-mono-size)", display: "grid", gridTemplateColumns: "1fr 1fr" }}>
      {rows.map((row, i) => (
        <div key={i} style={{ display: "contents" }}>
          <div
            style={{
              background: row.left ? bgFor(row.left.kind) : "transparent",
              whiteSpace: "pre",
              overflowX: "auto",
              borderRight: "1px solid var(--tervin-line)",
              padding: "0 var(--sp-1)",
            }}
          >
            <span className="meta tabular" style={{ marginRight: 8 }}>{row.left?.old_lineno ?? ""}</span>
            {row.left?.content ?? ""}
          </div>
          <div
            style={{
              background: row.right ? bgFor(row.right.kind) : "transparent",
              whiteSpace: "pre",
              overflowX: "auto",
              padding: "0 var(--sp-1)",
            }}
          >
            <span className="meta tabular" style={{ marginRight: 8 }}>{row.right?.new_lineno ?? ""}</span>
            {row.right?.content ?? ""}
          </div>
        </div>
      ))}
    </div>
  );
}

/** Diff backgrounds are tinted, not saturated: the text has to stay readable. */
function bgFor(kind: api.DiffLineKind): string {
  switch (kind) {
    case "added":
      return "color-mix(in srgb, var(--tervin-green) 14%, transparent)";
    case "removed":
      return "color-mix(in srgb, var(--tervin-red) 14%, transparent)";
    default:
      return "transparent";
  }
}

function signFor(kind: api.DiffLineKind): string {
  return kind === "added" ? "+" : kind === "removed" ? "−" : kind === "no_newline" ? "\\" : " ";
}

function markerFor(kind: api.ChangeKind): string {
  const map: Record<string, string> = {
    added: "A",
    modified: "M",
    deleted: "D",
    renamed: "R",
    copied: "C",
    type_changed: "T",
    untracked: "?",
    unmerged: "!",
  };
  return map[kind] ?? "M";
}
