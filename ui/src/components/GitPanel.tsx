/**
 * Git status.
 *
 * Reports the repository as git itself would, including the state that changes
 * what a commit means — a rebase or merge in progress is surfaced prominently
 * rather than hidden behind a changed-file count.
 */

import { useEffect } from "react";
import * as api from "../lib/api";
import { describeError, useWorkspace } from "../lib/store";

export function GitPanel() {
  const s = useWorkspace();
  const git = s.gitStatus;

  useEffect(() => {
    void s.refreshGit();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!git) {
    return <div className="empty">This project is not a git repository.</div>;
  }

  const groups: [string, api.FileStatus[]][] = [
    ["Conflicted", git.files.filter((f) => f.stage === "conflicted")],
    ["Staged", git.files.filter((f) => f.stage === "staged" || f.stage === "both")],
    ["Unstaged", git.files.filter((f) => f.stage === "unstaged" || f.stage === "both")],
    ["Untracked", git.files.filter((f) => f.stage === "untracked")],
  ];

  return (
    <div className="col" style={{ minHeight: 0 }}>
      <div style={{ padding: "var(--sp-3)", borderBottom: "1px solid var(--tervin-line)" }}>
        <div className="row" style={{ gap: "var(--sp-2)" }}>
          <strong>{git.detached ? "detached HEAD" : (git.branch ?? "no branch")}</strong>
          {git.upstream && <span className="meta">→ {git.upstream}</span>}
          <div className="grow" />
          {git.ahead > 0 && <span className="chip tabular">↑{git.ahead}</span>}
          {git.behind > 0 && <span className="chip tabular">↓{git.behind}</span>}
        </div>

        {git.operation_in_progress && (
          <div
            className="row"
            style={{
              marginTop: "var(--sp-2)",
              padding: "var(--sp-2)",
              border: "1px solid var(--tervin-amber)",
              borderRadius: "var(--radius-sm)",
              gap: "var(--sp-2)",
            }}
          >
            <span className="dot dot-amber" />
            <span className="meta">
              {git.operation_in_progress}. Committing now means something different
              from a normal commit.
            </span>
          </div>
        )}

        {git.head_sha && (
          <div className="meta mono" style={{ marginTop: "var(--sp-2)" }}>
            {git.head_sha.slice(0, 12)}
          </div>
        )}
      </div>

      {groups.map(([label, files]) =>
        files.length === 0 ? null : (
          <div key={label}>
            <div
              className="row meta"
              style={{
                padding: "var(--sp-2) var(--sp-3)",
                background: "var(--tervin-bg)",
                gap: "var(--sp-2)",
              }}
            >
              <span className="grow">
                {label} · {files.length}
              </span>
              {label === "Unstaged" && (
                <button
                  className="btn btn-ghost"
                  onClick={() =>
                    void api
                      .gitStage(files.map((f) => f.path))
                      .then(() => s.refreshGit())
                      .catch((e) => s.pushNotice(describeError(e)))
                  }
                >
                  Stage all
                </button>
              )}
              {label === "Staged" && (
                <button
                  className="btn btn-ghost"
                  onClick={() =>
                    void api
                      .gitUnstage(files.map((f) => f.path))
                      .then(() => s.refreshGit())
                      .catch((e) => s.pushNotice(describeError(e)))
                  }
                >
                  Unstage all
                </button>
              )}
            </div>
            {files.map((f) => (
              <div
                key={`${label}-${f.path}`}
                className="row"
                style={{ padding: "var(--sp-1) var(--sp-3)", gap: "var(--sp-2)" }}
              >
                <span className="meta" style={{ width: 12, flex: "none" }}>
                  {(f.worktree_change ?? f.index_change ?? "modified")[0]?.toUpperCase()}
                </span>
                <span className="mono truncate grow selectable" style={{ fontSize: "var(--text-meta)" }}>
                  {f.original_path ? `${f.original_path} → ${f.path}` : f.path}
                </span>
              </div>
            ))}
          </div>
        ),
      )}

      <div className="meta" style={{ padding: "var(--sp-3)" }}>
        Tervin never commits on your behalf. Use the terminal to commit, and the
        Review tab to inspect exactly what changed first.
      </div>
    </div>
  );
}
