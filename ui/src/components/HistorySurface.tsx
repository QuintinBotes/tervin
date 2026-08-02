/**
 * History.
 *
 * Every command Tervin has seen, searchable months later. This is the surface that
 * makes Blocks worth capturing: a shell's own history holds command text and nothing
 * else, so "what was the flag that fixed the build in March" is unanswerable in a
 * normal terminal — the output, the exit code, and the directory are all gone.
 *
 * Search runs against SQLite FTS5 over command *and* output, which is the part a shell
 * cannot do. Filters narrow by project, host, status, and bookmark, because on a
 * machine with a year of history a text query alone returns too much.
 *
 * ## Why the filters are what they are
 *
 * Each one answers a question people actually ask. "Failed only" is how you find the
 * error you half-remember. "Bookmarked" is the deliberate archive. "This project" is
 * the difference between your history and everyone's. There is no date picker: `since`
 * exists in the backend, and a calendar widget is a worse way to reach it than typing
 * a word into a search box that already searches output.
 */

import { useEffect, useState } from "react";
import * as api from "../lib/api";
import { useWorkspace } from "../lib/store";
import { BlocksPanel, formatBytes, formatDuration, toneForStatus } from "./BlocksPanel";
import { TwoColumn } from "../App";

export function HistorySurface({ narrow }: { narrow: boolean }) {
  const s = useWorkspace();
  const [query, setQuery] = useState("");
  const [failuresOnly, setFailuresOnly] = useState(false);
  const [bookmarkedOnly, setBookmarkedOnly] = useState(false);
  const [thisProject, setThisProject] = useState(false);
  const [tag, setTag] = useState<string | null>(null);
  const [tags, setTags] = useState<string[]>([]);

  const blockCount = s.blocks.length;

  // The tag vocabulary comes from what has actually been tagged. Offering a fixed set
  // would invite tags nobody uses.
  useEffect(() => {
    void api.blockTagsAll().then(setTags).catch(() => {});
  }, [blockCount]);

  // This surface is the *only* writer of the block query.
  //
  // Previously both this and BlocksPanel called `refreshBlocks`, which writes
  // `blockFilter` — so each re-triggered the other. The panel is now purely a renderer
  // and every filter lives here, debounced so typing does not run a query per
  // keystroke. The filter is deliberately built fresh rather than spread from the
  // store: reading `blockFilter` here would reintroduce the feedback edge.
  useEffect(() => {
    const handle = setTimeout(() => {
      void useWorkspace.getState().refreshBlocks({
        text: query.trim() || null,
        statuses: failuresOnly ? ["failed"] : [],
        bookmarked_only: bookmarkedOnly || undefined,
        project: thisProject ? currentProject() : null,
        tags: tag ? [tag] : [],
        limit: 300,
      });
    }, 140);
    return () => clearTimeout(handle);
  }, [query, failuresOnly, bookmarkedOnly, thisProject, tag]);

  return (
    <TwoColumn
      narrow={narrow}
      listLabel="History"
      leftWidth={s.listColumnWidth}
      onResize={s.setListColumnWidth}
      left={
        <div className="col" style={{ minHeight: 0, width: "100%" }}>
          <div className="panel-header">
            <span className="label">History</span>
            <span className="meta truncate grow">
              {s.blocks.length === 0
                ? "Nothing captured yet"
                : `${s.blocks.length} block${s.blocks.length === 1 ? "" : "s"}`}
            </span>
          </div>

          <div
            className="row"
            style={{
              flex: "none",
              padding: "var(--sp-2) var(--sp-3)",
              gap: "var(--sp-1)",
              flexWrap: "wrap",
              borderBottom: "1px solid var(--tervin-line)",
            }}
          >
            <Filter
              on={failuresOnly}
              onToggle={() => setFailuresOnly((v) => !v)}
              label="Failed"
              hint="Only commands that exited non-zero or produced errors"
            />
            <Filter
              on={bookmarkedOnly}
              onToggle={() => setBookmarkedOnly((v) => !v)}
              label="Bookmarked"
              hint="The commands you kept on purpose"
            />
            <Filter
              on={thisProject}
              onToggle={() => setThisProject((v) => !v)}
              label="This project"
              hint="Only commands run in the project that is currently open"
            />
            {tags.slice(0, 6).map((name) => (
              <button
                key={name}
                className="chip"
                title={`Show only blocks tagged ${name}`}
                aria-pressed={tag === name}
                onClick={() => setTag((current) => (current === name ? null : name))}
                style={{
                  borderColor: tag === name ? "var(--tervin-accent)" : undefined,
                  color: tag === name ? "var(--tervin-accent)" : undefined,
                }}
              >
                {name}
              </button>
            ))}
          </div>

          <div style={{ flex: "none", padding: "var(--sp-2) var(--sp-3) 0" }}>
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search commands and output"
              aria-label="Search history"
              style={{ width: "100%" }}
              spellCheck={false}
            />
          </div>

          <div className="grow" style={{ overflow: "auto", minHeight: 0 }}>
            <BlocksPanel failuresOnly={failuresOnly} />
          </div>
        </div>
      }
      right={<HistoryDetail />}
    />
  );
}

/**
 * What history holds that a shell's does not.
 *
 * Shown instead of a duplicate of the list, because the list already expands a row in
 * place. This column explains the difference — and states plainly when a Block is
 * incomplete, which matters because a truncated output that looks whole is worse than
 * one that says so.
 */
function HistoryDetail() {
  const s = useWorkspace();
  const total = s.blocks.length;
  const failed = s.blocks.filter((b) => b.status === "failed" || b.error_count > 0).length;
  const truncated = s.blocks.filter((b) => b.output_truncated).length;
  const withTests = s.blocks.filter((b) => b.tests !== null).length;
  const bytes = s.blocks.reduce((sum, b) => sum + b.output_total, 0);
  const slowest = [...s.blocks]
    .filter((b) => b.duration_ms !== null)
    .sort((a, b) => (b.duration_ms ?? 0) - (a.duration_ms ?? 0))
    .slice(0, 5);

  return (
    <div className="col" style={{ minHeight: 0, width: "100%" }}>
      <div className="panel-header">
        <span className="label">About these results</span>
      </div>

      <div className="grow" style={{ overflow: "auto", minHeight: 0, padding: "var(--sp-6)" }}>
        {total === 0 ? (
          <div className="empty">
            No commands captured yet. Blocks need shell integration, which Tervin
            injects per pane — open a terminal, run something, and it appears here with
            its output, exit code, duration, and the directory it ran in.
          </div>
        ) : (
          <>
            <div className="col" style={{ gap: "var(--sp-2)" }}>
              <Stat label="Blocks" value={String(total)} />
              <Stat
                label="Failures"
                value={String(failed)}
                tone={failed > 0 ? "amber" : undefined}
              />
              <Stat label="With test results" value={String(withTests)} />
              <Stat label="Output stored" value={formatBytes(bytes)} />
              {truncated > 0 && (
                <Stat
                  label="Truncated"
                  value={String(truncated)}
                  tone="amber"
                  hint="Output exceeded the inline limit and spilled to disk. Expanding a row fetches the whole thing."
                />
              )}
            </div>

            {slowest.length > 0 && (
              <div style={{ marginTop: "var(--sp-6)" }}>
                <div className="label" style={{ marginBottom: "var(--sp-2)" }}>
                  Slowest here
                </div>
                {slowest.map((block) => (
                  <div
                    key={block.id}
                    className="row"
                    style={{ gap: "var(--sp-2)", padding: "3px 0" }}
                  >
                    <span className={`dot dot-${toneForStatus(block.status)}`} />
                    <span className="mono meta truncate grow" title={block.command}>
                      {block.command}
                    </span>
                    <span className="meta tabular">
                      {formatDuration(block.duration_ms ?? 0)}
                    </span>
                  </div>
                ))}
              </div>
            )}

            <div className="meta" style={{ marginTop: "var(--sp-6)", textWrap: "pretty" }}>
              {/* The reason this surface exists at all. */}
              Search runs over command text <em>and</em> output. A shell's history holds
              only what you typed, so the flag that fixed a build six months ago is
              findable here and nowhere else.
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function Stat({
  label,
  value,
  tone,
  hint,
}: {
  label: string;
  value: string;
  tone?: string;
  hint?: string;
}) {
  return (
    <div className="row" style={{ gap: "var(--sp-2)" }} title={hint}>
      <span className="meta" style={{ width: 130, flex: "none" }}>
        {label}
      </span>
      <span className={`tabular ${tone ? `tone-${tone}` : ""}`}>{value}</span>
    </div>
  );
}

function Filter({
  on,
  onToggle,
  label,
  hint,
}: {
  on: boolean;
  onToggle: () => void;
  label: string;
  hint: string;
}) {
  return (
    <button
      className="btn btn-xs"
      onClick={onToggle}
      title={hint}
      aria-pressed={on}
      style={{
        borderColor: on ? "var(--tervin-accent)" : undefined,
        color: on ? "var(--tervin-accent)" : undefined,
      }}
    >
      {label}
    </button>
  );
}

/** The project name the workspace is currently pointed at. */
function currentProject(): string | null {
  const root = useWorkspace.getState().environment?.project_root ?? null;
  if (!root) return null;
  const parts = root.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? null;
}
