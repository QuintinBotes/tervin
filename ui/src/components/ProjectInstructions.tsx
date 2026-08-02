/**
 * What other tools have already written into this project, and whether the runtime
 * you are about to use will actually obey it.
 *
 * The list is not the point. Every terminal and editor can find `AGENTS.md`. The
 * point is the second column: the same `CLAUDE.md` is in force for Claude Code and
 * ignored by Codex, and the same `AGENTS.md` is read natively by both while a local
 * model reads nothing at all. Presenting one list without saying who reads what is
 * how a user comes to believe an instruction file is governing an agent that has
 * never seen it.
 *
 * So the runtime is a selector, not a detail, and it defaults to the runtime of the
 * active Thread rather than to a fixed choice.
 */

import { useEffect, useMemo, useState } from "react";
import * as api from "../lib/api";
import { useWorkspace } from "../lib/store";

/** Human labels for the file kinds. */
const KIND_LABEL: Record<api.InstructionKind, string> = {
  agents: "AGENTS.md",
  claude_md: "CLAUDE.md",
  claude_local: "CLAUDE.local.md",
  cursor_rules: "Cursor rules",
  copilot_instructions: "Copilot instructions",
  gemini_md: "GEMINI.md",
  windsurf_rules: "Windsurf rules",
  cline_rules: "Cline rules",
};

const MCP_LABEL: Record<api.McpConfigKind, string> = {
  project_mcp_json: ".mcp.json",
  claude_json: "Claude Code",
  codex_toml: "Codex",
  gemini_settings: "Gemini CLI",
};

/** The runtimes worth offering, with the local model standing in for the family. */
const RUNTIMES: { id: string; label: string }[] = [
  { id: "claude-code", label: "Claude Code" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini CLI" },
  { id: "cursor-agent", label: "Cursor" },
  { id: "ollama", label: "A local model" },
];

export function ProjectInstructions() {
  const s = useWorkspace();
  const [data, setData] = useState<api.ProjectInstructions | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Default to whatever the active Thread is actually running, because that is the
  // answer the user came here for. Falls back to Claude Code only when nothing is
  // running rather than pretending a choice was made.
  const activeRuntime = s.activeThreadId
    ? s.threads[s.activeThreadId]?.runtimeId
    : undefined;
  const [runtime, setRuntime] = useState<string>(activeRuntime ?? "claude-code");
  useEffect(() => {
    if (activeRuntime) setRuntime(activeRuntime);
  }, [activeRuntime]);

  useEffect(() => {
    let live = true;
    api
      .projectInstructions()
      .then((d) => live && setData(d))
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
    // Re-read when the project changes: instruction files belong to a directory.
  }, [s.environment?.project_root]);

  const paired = useMemo(() => {
    if (!data) return [];
    return data.discovered.files.map((file) => ({
      file,
      readership: readershipFor(file.kind, runtime),
    }));
  }, [data, runtime]);

  if (error) {
    return <div className="empty">Could not read the project: {error}</div>;
  }
  if (!data) {
    return <div className="empty">Looking for instruction files…</div>;
  }

  const { discovered, adoptable } = data;
  const known = RUNTIMES.some((r) => r.id === runtime);

  return (
    <div className="col" style={{ minHeight: 0, gap: "var(--sp-4)" }}>
      <div className="panel-header">
        <span className="label">Instructions in force</span>
        <span className="meta truncate">{data.project_root}</span>
      </div>

      <div className="row" style={{ gap: "var(--sp-2)", flexWrap: "wrap" }}>
        <span className="meta">Reading as</span>
        <select
          className="input input-sm"
          value={runtime}
          onChange={(e) => setRuntime(e.target.value)}
          aria-label="Runtime to report readership for"
        >
          {RUNTIMES.map((r) => (
            <option key={r.id} value={r.id}>
              {r.label}
            </option>
          ))}
          {/* A runtime Tervin has no table entry for still needs to be selectable,
              because the honest "unknown" answer is the useful one for it. */}
          {!known && <option value={runtime}>{runtime}</option>}
        </select>
      </div>

      {discovered.files.length === 0 ? (
        <div className="empty">
          No instruction files here.{" "}
          {discovered.truncated
            ? "The nested search was capped, so there may be some deeper in the tree."
            : "An AGENTS.md at the project root is read by most agents, including Claude Code and Codex."}
        </div>
      ) : (
        <div className="col" style={{ gap: 0 }}>
          {paired.map(({ file, readership }) => (
            <div
              key={file.path}
              className="row"
              style={{
                gap: "var(--sp-2)",
                padding: "var(--sp-1) var(--sp-2)",
                alignItems: "baseline",
              }}
            >
              <span className="mono" style={{ minWidth: "11rem" }}>
                {KIND_LABEL[file.kind]}
              </span>
              <span className="meta truncate" style={{ flex: 1, minWidth: 0 }}>
                {/* The directory, not just the filename: three nested CLAUDE.md
                    files are otherwise three identical rows. */}
                {scopeLabel(file.scope)} · {formatBytes(file.bytes)}
                {file.kind === "claude_local" && " · personal, not committed"}
              </span>
              <ReadershipTag readership={readership} />
            </div>
          ))}
          {discovered.truncated && (
            <div className="meta" style={{ padding: "var(--sp-1) var(--sp-2)" }}>
              The nested search was capped, so there may be more than this.
            </div>
          )}
        </div>
      )}

      {discovered.mcp.length > 0 && (
        <div className="col" style={{ gap: 0 }}>
          <div className="panel-header">
            <span className="label">MCP servers configured elsewhere</span>
            <span className="meta truncate">
              Names only. Tervin does not read a server's command or environment.
            </span>
          </div>
          {discovered.mcp.map((cfg) => (
            <div
              key={cfg.path}
              className="row"
              style={{ gap: "var(--sp-2)", padding: "var(--sp-1) var(--sp-2)" }}
            >
              <span className="mono" style={{ minWidth: "11rem" }}>
                {MCP_LABEL[cfg.kind]}
              </span>
              <span className="meta truncate" style={{ flex: 1, minWidth: 0 }}>
                {cfg.error
                  ? // Surfaced rather than hidden: a runtime that silently ignores its
                    // own broken config leaves a user with nothing to go on.
                    `could not be parsed: ${cfg.error}`
                  : cfg.servers.length === 0
                    ? "no servers configured"
                    : cfg.servers.join(", ")}
              </span>
            </div>
          ))}
        </div>
      )}

      {adoptable.length > 0 && (
        <div className="col" style={{ gap: "var(--sp-1)" }}>
          <div className="panel-header">
            <span className="label">Could be adopted</span>
            <span className="meta truncate">
              Tervin supplies MCP servers to ACP agents, which have no config of their
              own. Copying one here adds tools to an agent, so nothing is adopted
              automatically.
            </span>
          </div>
          {adoptable.map((c) => (
            <div
              key={c.name}
              className="row"
              style={{ gap: "var(--sp-2)", padding: "0 var(--sp-2)" }}
            >
              <span className="mono">{c.name}</span>
              <span className="meta truncate" style={{ flex: 1, minWidth: 0 }}>
                from {c.source}
                {c.conflicts && " · Tervin already has this name, so adopting replaces it"}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * The readership badge.
 *
 * `native` carries its evidence in a title, because a claim about what someone
 * else's program reads should be auditable rather than asserted.
 */
function ReadershipTag({ readership }: { readership: api.Readership }) {
  switch (readership.readership) {
    case "native":
      return (
        <span className="tag tone-green" title={readership.evidence}>
          in force
        </span>
      );
    case "ignored":
      return (
        <span className="tag tone-muted" title="This runtime does not read this file.">
          not read
        </span>
      );
    case "injectable":
      return (
        <span
          className="tag"
          title="This runtime reads no instruction files, so Tervin can pass the text in. Not automatic: it changes what the agent was told."
        >
          Tervin can pass in
        </span>
      );
    case "unknown":
      return (
        <span
          className="tag tone-amber"
          title="Tervin has no verified entry for this runtime. It may read this file or ignore it, and guessing would be worse than saying so."
        >
          unknown
        </span>
      );
  }
}

/**
 * The readership table, mirrored from `crates/agent-runtime/src/instructions.rs`.
 *
 * Duplicated deliberately rather than fetched per runtime: the panel switches
 * runtime on a click and a round trip per click would make it feel broken. The Rust
 * side stays authoritative, and `instructions.readership.test.ts` asserts the two
 * agree so a change on one side cannot drift silently.
 */
export function readershipFor(
  kind: api.InstructionKind,
  runtimeId: string,
): api.Readership {
  const native = (evidence: string): api.Readership => ({
    readership: "native",
    evidence,
  });
  const ignored: api.Readership = { readership: "ignored" };

  switch (runtimeId) {
    case "claude-code":
    case "claude-code-acp":
      // One discovery mechanism covering both filenames. Verified in the shipped
      // 2.1.220 binary, which contains
      // "Claude Code hardcodes CLAUDE.md / AGENTS.md discovery".
      return kind === "agents" || kind === "claude_md" || kind === "claude_local"
        ? native(
            "Claude Code hardcodes CLAUDE.md / AGENTS.md discovery (verified in 2.1.220)",
          )
        : ignored;
    case "codex":
      return kind === "agents"
        ? native("Codex reads AGENTS.md as its instruction file")
        : ignored;
    case "gemini":
    case "gemini-acp":
      return kind === "gemini_md"
        ? native("Gemini CLI reads GEMINI.md as its context file")
        : ignored;
    case "cursor-agent":
      return kind === "cursor_rules"
        ? native("Cursor reads .cursorrules and .cursor/rules/")
        : ignored;
    case "copilot-acp":
      return kind === "copilot_instructions"
        ? native("Copilot reads .github/copilot-instructions.md")
        : ignored;
    case "lmstudio":
    case "ollama":
    case "vllm":
    case "llamacpp":
      // Conversational endpoints: they receive a prompt and nothing else.
      return { readership: "injectable" };
    default:
      return { readership: "unknown" };
  }
}

function scopeLabel(scope: api.InstructionScope): string {
  switch (scope.scope) {
    case "user":
      return "your home directory, so every project";
    case "project_root":
      return "project root";
    case "nested":
      return scope.relative_dir;
  }
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "empty";
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} kB`;
}
