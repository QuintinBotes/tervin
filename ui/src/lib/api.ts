/**
 * Typed wrappers over the Rust IPC surface.
 *
 * Every call the UI makes goes through here, so the boundary has exactly one
 * definition and a rename in Rust breaks the build rather than failing at
 * runtime. The types mirror the `serde` shapes on the other side.
 */

import { invoke, Channel } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ------------------------------------------------------------------ types

export type BlockStatus =
  | "running"
  | "succeeded"
  | "failed"
  | "interrupted"
  | "unknown";

export type Severity = "error" | "warning" | "info" | "hint";

export interface TestSummary {
  runner: string;
  passed: number;
  failed: number;
  skipped: number;
}

export interface BlockSummary {
  id: string;
  pane_id: string;
  thread_id: string | null;
  command: string;
  cwd: string;
  host: string;
  project: string | null;
  started_at: string;
  duration_ms: number | null;
  exit_code: number | null;
  status: BlockStatus;
  bookmarked: boolean;
  tags: string[];
  note: string | null;
  output_total: number;
  output_truncated: boolean;
  git_branch: string | null;
  error_count: number;
  warning_count: number;
  tests: TestSummary | null;
  ports: number[];
  preview: string;
}

export interface ParsedDiagnostic {
  severity: Severity;
  message: string;
  path: string | null;
  line: number | null;
  column: number | null;
  source: string | null;
}

export interface Block extends Omit<BlockSummary, "preview"> {
  shell: string | null;
  ended_at: string | null;
  output: {
    inline: string;
    spill_path: string | null;
    total_bytes: number;
    truncated: boolean;
  };
  git: {
    repo_root: string | null;
    branch: string | null;
    dirty: boolean | null;
    head_sha: string | null;
  };
  parsed: {
    paths: { path: string; line: number | null; exists: boolean }[];
    urls: string[];
    ports: number[];
    diagnostics: ParsedDiagnostic[];
    tests: TestSummary | null;
    error_count: number;
    warning_count: number;
  };
  artifacts: string[];
}

export interface BlockFilter {
  text?: string | null;
  project?: string | null;
  cwd_prefix?: string | null;
  host?: string | null;
  thread_id?: string | null;
  pane_id?: string | null;
  statuses?: BlockStatus[];
  tags?: string[];
  command_contains?: string | null;
  bookmarked_only?: boolean;
  since?: string | null;
  until?: string | null;
  sort?: "newest_first" | "oldest_first" | "longest_first";
  limit?: number;
  offset?: number;
}

export type ChangeKind =
  | "added"
  | "modified"
  | "deleted"
  | "renamed"
  | "copied"
  | "type_changed"
  | "untracked"
  | "unmerged";

export type StageState =
  | "staged"
  | "unstaged"
  | "both"
  | "untracked"
  | "ignored"
  | "conflicted";

export interface FileStatus {
  path: string;
  original_path: string | null;
  stage: StageState;
  index_change: ChangeKind | null;
  worktree_change: ChangeKind | null;
}

export interface RepoStatus {
  root: string;
  branch: string | null;
  head_sha: string | null;
  detached: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  files: FileStatus[];
  dirty: boolean;
  operation_in_progress: string | null;
}

export type DiffLineKind = "context" | "added" | "removed" | "no_newline";

export interface DiffLine {
  kind: DiffLineKind;
  content: string;
  old_lineno: number | null;
  new_lineno: number | null;
}

export interface Hunk {
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  section: string | null;
  lines: DiffLine[];
}

export interface FileDiff {
  path: string;
  old_path: string | null;
  kind: ChangeKind;
  binary: boolean;
  hunks: Hunk[];
  added_lines: number;
  removed_lines: number;
  raw_header: string[];
}

export type DiffMode = "unstaged" | "staged" | "working_tree";

export type RiskLevel = "low" | "moderate" | "high" | "critical";

export interface RiskAssessment {
  level: RiskLevel;
  categories: string[];
  reasons: string[];
  side_effects: string[];
  matched_rule: string | null;
  /** False when a decision here cannot actually stop the action. */
  enforceable: boolean;
}

export type ApprovalScope =
  | { scope: "once" }
  | { scope: "task"; thread_id: string }
  | { scope: "workspace" };

export interface ApprovalRequest {
  id: string;
  action: string;
  kind: string;
  cwd: string;
  host: string;
  thread_id: string | null;
  actor: string;
  risk: RiskAssessment;
  reason: string;
  interceptable: boolean;
  created_at: string;
  available_scopes: ApprovalScope[];
}

export type Decision =
  | { decision: "allow"; reason: string; matched_rule?: string }
  | { decision: "require_approval"; request: ApprovalRequest }
  | { decision: "deny"; reason: string; matched_rule?: string };

export interface PolicyRule {
  id: string;
  name: string;
  pattern: Record<string, unknown>;
  effect: "allow" | "require_approval" | "deny";
  reason: string;
  enabled: boolean;
}

export interface AuditRecord {
  id: string;
  ts: string;
  thread_id: string | null;
  actor: string;
  action: string;
  phase: string;
  decision: string | null;
  authority: string | null;
  scope: string | null;
  risk: string | null;
  detail: string | null;
}

export type CapabilityLevel =
  | { level: "supported" }
  | { level: "partial"; note: string }
  | { level: "unsupported"; reason: string }
  | { level: "unknown" };

export interface Capabilities {
  tier: "structured" | "enhanced_cli" | "generic_terminal" | "conversational";
  plan_mode: CapabilityLevel;
  resume: CapabilityLevel;
  tool_events: CapabilityLevel;
  file_edits: CapabilityLevel;
  native_permission_bridge: CapabilityLevel;
  mcp: CapabilityLevel;
  hooks: CapabilityLevel;
  subagents: CapabilityLevel;
  image_input: CapabilityLevel;
  cost_reporting: CapabilityLevel;
  model_selection: CapabilityLevel;
  remote_execution: CapabilityLevel;
  multi_turn: CapabilityLevel;
  interrupt: CapabilityLevel;
}

export interface AgentProfile {
  id: string;
  name: string;
  runtime_id: string;
  binary: string;
  args: string[];
  env: Record<string, string>;
  model?: string | null;
  permission_mode?: string | null;
  badge?: string | null;
  sensitive: boolean;
}

export interface Discovery {
  runtime_id: string;
  display_name: string;
  available: boolean;
  version: string | null;
  path: string | null;
  notes: string[];
  capabilities: Capabilities;
}

export interface ImportCandidate {
  profile: AgentProfile;
  source: string;
}

export interface AgentsOverview {
  profiles: AgentProfile[];
  default_profile: string | null;
  discovered: Discovery[];
  import_candidates: ImportCandidate[];
  /** Resolved paths, because they differ by platform. Never hard-code them. */
  profiles_path: string;
  mcp_path: string;
}

export interface PermissionState {
  mode: string;
  tervin_can_intercept: boolean;
  explanation: string;
  denials: string[];
}

/** A mode the running session will accept. Reported, never assumed. */
export interface SessionMode {
  id: string;
  name: string;
  description?: string | null;
}

/** One execution of one of the user's own hooks. */
export interface HookRun {
  name: string;
  event: string;
  exit_code: number;
  outcome: string;
  message?: string | null;
  /** True for Tervin's own permission gate, not the user's configuration. */
  is_tervin: boolean;
}

export interface SessionMetadata {
  resume_id: string | null;
  model: string | null;
  permission_mode: string | null;
  runtime_version: string | null;
  tools: string[];
  mcp_servers: { name: string; status: string }[];
  slash_commands: string[];
  instruction_sources: string[];
  /** The user's own hooks, as they actually ran. */
  hook_runs: HookRun[];
  /**
   * The modes this session offers. Empty means the runtime reported none, and the
   * UI shows no mode control rather than guessing at one.
   */
  modes: SessionMode[];
}

export type ThreadState =
  | "idle"
  | "starting"
  | "awaiting_input"
  | "understanding"
  | "planning"
  | "reading"
  | "editing"
  | "executing"
  | "testing"
  | "waiting_for_permission"
  | "waiting_for_external_tool"
  | "review_required"
  | "completed"
  | "failed"
  | "interrupted"
  | "disconnected"
  | "unknown";

export interface TervinEvent {
  id: string;
  thread_id: string | null;
  ts: string;
  agent: {
    runtime_id: string;
    display_name: string;
    tier: string;
    model?: string;
    version?: string;
  };
  project: string | null;
  cwd: string | null;
  summary: string;
  raw: { kind: string; pointer: string; byte_len: number; redacted: boolean } | null;
  links: Record<string, unknown>[];
  payload: { type: string } & Record<string, unknown>;
}

export interface ThreadInfo {
  thread_id: string;
  profile_id: string;
  runtime_id: string;
  running: boolean;
  metadata: SessionMetadata;
  permissions: PermissionState;
  capabilities: Capabilities;
  diagnostics: { severity: Severity; message: string; at: string }[];
}

export interface Expansion {
  original: string;
  expanded: string;
  applied: { name: string; expansion: string }[];
}

export interface ShellAliases {
  aliases: Record<string, string>;
  functions: string[];
  global_aliases: Record<string, string>;
  shell: string | null;
  notes: string[];
  /**
   * Whether the shell was actually asked.
   *
   * Distinguishes "you have no aliases" from "Tervin could not read them" — the same
   * empty list otherwise. It matters because alias discovery is how a second agent
   * account gets found, so a silent failure means never learning a profile was there.
   */
  enumerated: boolean;
}

export interface IntegrationStatus {
  shell: "zsh" | "bash" | "fish" | "power_shell";
  script_written: boolean;
  script_path: string;
  installed: boolean;
  rc_path: string | null;
  proposed_line: string;
}

export interface ShellEnvironment {
  shell: string | null;
  integration: IntegrationStatus[];
  aliases: ShellAliases;
  project_root: string;
  home: string | null;
  notices: string[];
}

export interface CommandError {
  message: string;
  code: string;
}

// --------------------------------------------------------------- terminal

/**
 * A Thread for an agent the user started in a pane, as the backend records it.
 *
 * Mirrors the `Thread` struct's fields that the UI needs. Arrives on
 * `thread://observed` before any of the Thread's events, because the events are
 * dropped for a Thread the UI has not been told about.
 */
export interface ObservedThread {
  id: string;
  agent: {
    runtime_id: string;
    display_name: string;
    tier: string;
    model?: string;
    version?: string;
  };
  state: ThreadState;
  task_title: string;
  project: string | null;
  cwd: string;
  /** The pane it is running in. Its presence is what marks a Thread read-only. */
  pane_id: string | null;
  /** The agent's own session id — what `claude --resume` takes. */
  resume_id: string | null;
}

export interface SpawnRequest {
  cwd?: string | null;
  cols: number;
  rows: number;
  program?: string | null;
  args?: string[];
  env?: [string, string][];
  thread_id?: string | null;
  title?: string | null;
}

export interface SpawnResponse {
  pane_id: string;
  shell: string | null;
  /** Whether the shell hook will load in this pane. */
  integration_installed: boolean;
  /** Why it will not, when it will not. */
  integration_note: string | null;
}

/**
 * Open a pane. `onOutput` receives raw terminal bytes.
 *
 * The payload arrives as an `ArrayBuffer` rather than a string: terminal output
 * is bytes, and decoding it to UTF-16 and back on every frame would both cost
 * time and corrupt a multi-byte character split across two reads.
 */
export async function ptySpawn(
  request: SpawnRequest,
  onOutput: (bytes: Uint8Array) => void,
): Promise<SpawnResponse> {
  const channel = new Channel<ArrayBuffer | number[]>();
  channel.onmessage = (message) => {
    onOutput(
      message instanceof ArrayBuffer
        ? new Uint8Array(message)
        : Uint8Array.from(message as number[]),
    );
  };
  return invoke<SpawnResponse>("pty_spawn", { request, onOutput: channel });
}

export const ptyWrite = (paneId: string, data: Uint8Array) =>
  invoke<void>("pty_write", { paneId, data: Array.from(data) });

export const ptyResize = (paneId: string, cols: number, rows: number) =>
  invoke<void>("pty_resize", { paneId, cols, rows });

export const ptyClose = (paneId: string) => invoke<void>("pty_close", { paneId });

// ----------------------------------------------------------------- blocks

export const blocksQuery = (filter: BlockFilter) =>
  invoke<BlockSummary[]>("blocks_query", { filter });

export const blockGet = (blockId: string) =>
  invoke<Block | null>("block_get", { blockId });

export const blockOutput = (blockId: string) =>
  invoke<number[]>("block_output", { blockId }).then((b) => Uint8Array.from(b));

export const blockSetBookmark = (blockId: string, bookmarked: boolean) =>
  invoke<void>("block_set_bookmark", { blockId, bookmarked });

export const blockSetTags = (blockId: string, tags: string[]) =>
  invoke<void>("block_set_tags", { blockId, tags });

export const blockSetNote = (blockId: string, note: string | null) =>
  invoke<void>("block_set_note", { blockId, note });

export const blockTagsAll = () => invoke<string[]>("block_tags_all");

/** One prompt or agent reply found by search. */
export interface PromptHit {
  event_id: string;
  thread_id: string | null;
  /** `user.prompted` or `agent.message`. */
  kind: string;
  text: string;
  ts: string;
  runtime_id: string;
  project: string | null;
}

/**
 * Search past prompts and agent replies.
 *
 * The gap this fills: a shell keeps command history, and no agent keeps a searchable
 * record of what you asked it — sessions end and the conversation goes with them.
 * Reasoning passages are excluded, because they would swamp a search for something you
 * actually wrote.
 */
export const promptsSearch = (query: string, limit: number) =>
  invoke<PromptHit[]>("prompts_search", { query, limit });

export interface RetentionInfo {
  /** Days of agent history kept. Zero means nothing is pruned. */
  days: number;
  default_days: number;
}

export const historyRetention = () => invoke<RetentionInfo>("history_retention");

/** Change the window, pruning immediately. Returns how many events went. */
export const historySetRetention = (days: number) =>
  invoke<number>("history_set_retention", { days });

/** One entry in a directory listing. */
export interface DirEntry {
  name: string;
  path: string;
  is_dir: boolean;
  hidden: boolean;
  symlink: boolean;
}

/**
 * List one directory for the file explorer.
 *
 * One level at a time on purpose: a tree that reads a whole repository up front spends
 * seconds inside `node_modules` before drawing anything.
 */
export const fsListDir = (path: string | null) =>
  invoke<DirEntry[]>("fs_list_dir", { path });

// -------------------------------------------------------------------- git

export const gitStatus = (path?: string) =>
  invoke<RepoStatus | null>("git_status", { path: path ?? null });

export const gitDiff = (mode: DiffMode, path?: string) =>
  invoke<FileDiff[]>("git_diff", { mode, path: path ?? null });

export const gitBranches = () => invoke<unknown[]>("git_branches");

export const gitLog = (limit: number) => invoke<unknown[]>("git_log", { limit });

export const gitStage = (paths: string[]) => invoke<void>("git_stage", { paths });

export const gitUnstage = (paths: string[]) => invoke<void>("git_unstage", { paths });

export const gitApplyHunks = (
  path: string,
  mode: DiffMode,
  hunks: number[],
  reverse: boolean,
  cached: boolean,
) => invoke<void>("git_apply_hunks", { path, mode, hunks, reverse, cached });

// ------------------------------------------------------------------ rules

export const rulesList = () => invoke<PolicyRule[]>("rules_list");
export const rulesPending = () => invoke<ApprovalRequest[]>("rules_pending");

export const rulesEvaluate = (command: string, cwd?: string) =>
  invoke<{ decision: Decision; expansion: Expansion | null }>("rules_evaluate", {
    command,
    cwd: cwd ?? null,
  });

export const rulesResolve = (requestId: string, outcome: Record<string, unknown>) =>
  invoke<{ result: string; command: string | null }>("rules_resolve", {
    requestId,
    outcome,
  });

export const rulesAdd = (rule: PolicyRule) => invoke<void>("rules_add", { rule });
export const rulesRemove = (id: string) => invoke<boolean>("rules_remove", { id });
export const auditRecent = (limit: number) =>
  invoke<AuditRecord[]>("audit_recent", { limit });

// ----------------------------------------------------------------- agents

export const agentsOverview = () => invoke<AgentsOverview>("agents_overview");

export const agentsSaveProfiles = (
  profiles: AgentProfile[],
  defaultProfile: string | null,
) =>
  invoke<string>("agents_save_profiles", { profiles, defaultProfile });

/**
 * Register an agent that speaks the Agent Client Protocol.
 *
 * Any such agent gets the full structured integration — plans, tool events, and a
 * real permission gate — without Tervin knowing anything about it in advance.
 */
/** A portable handoff built from a Thread's recorded work. */
export interface ContextBundle {
  task: string | null;
  origin: string;
  outcome: string;
  plan: string[];
  files_touched: string[];
  commands: { command: string; exit_code: number | null; excerpt: string | null }[];
  tests: string[];
  problems: string[];
  refusals: string[];
  last_message: string | null;
  omissions: string[];
}

export interface HandoffResponse {
  /** Ready to send as the first prompt of a new Thread. */
  prompt: string;
  summary: string;
  bundle: ContextBundle;
}

/**
 * Turn a Thread's work into a briefing another agent can read.
 *
 * The point of a provider-neutral event stream: what Claude Code did can be handed to
 * an ACP agent or a local model without either knowing the other exists.
 */
export const threadHandoff = (threadId: string) =>
  invoke<HandoffResponse>("thread_handoff", { threadId });

export const agentsAddAcp = (displayName: string, binary: string, args: string[]) =>
  invoke<Discovery>("agents_add_acp", { displayName, binary, args });

/**
 * Register an OpenAI-compatible model endpoint — LM Studio, Ollama, vLLM, or any
 * remote server speaking the same dialect.
 */
export const agentsAddLocalModel = (
  displayName: string,
  baseUrl: string,
  apiKey: string | null,
) => invoke<Discovery>("agents_add_local_model", { displayName, baseUrl, apiKey });

export interface ThreadStartRequest {
  profile_id?: string | null;
  cwd?: string | null;
  prompt: string;
  attachments?: Record<string, unknown>[];
  model?: string | null;
  permission_mode?: string | null;
  task_title?: string | null;
  resume_id?: string | null;
}

export interface ThreadStartResponse {
  thread_id: string;
  profile_id: string;
  runtime_id: string;
  capabilities: Capabilities;
  permissions: PermissionState;
}

export const threadStart = (request: ThreadStartRequest) =>
  invoke<ThreadStartResponse>("thread_start", { request });

export const threadSend = (
  threadId: string,
  content: string,
  attachments: Record<string, unknown>[] = [],
) => invoke<void>("thread_send", { threadId, content, attachments });

export const threadInterrupt = (threadId: string) =>
  invoke<void>("thread_interrupt", { threadId });

export const threadSetPermissionMode = (threadId: string, mode: string) =>
  invoke<void>("thread_set_permission_mode", { threadId, mode });

export const threadInfo = (threadId: string) =>
  invoke<ThreadInfo | null>("thread_info", { threadId });

export const threadEvents = (threadId: string, limit: number) =>
  invoke<TervinEvent[]>("thread_events", { threadId, limit });

export const threadsList = (limit: number) => invoke<unknown[]>("threads_list", { limit });

export const threadStop = (threadId: string) => invoke<void>("thread_stop", { threadId });

// ------------------------------------------------------ environment & settings

export const environment = () => invoke<ShellEnvironment>("environment");

export const shellIntegrationInstall = (shell: string) =>
  invoke<Record<string, unknown>>("shell_integration_install", { shell });

export const shellIntegrationUninstall = (shell: string) =>
  invoke<boolean>("shell_integration_uninstall", { shell });

export const aliasesReload = () => invoke<ShellAliases>("aliases_reload");

export const aliasExpand = (command: string) =>
  invoke<Expansion>("alias_expand", { command });

export const settingsGet = (key: string) =>
  invoke<string | null>("settings_get", { key });

export const settingsSet = (key: string, value: string) =>
  invoke<void>("settings_set", { key, value });

/** One hole in a saved command. */
export interface SavedParameter {
  name: string;
  /** Prefilled when present, so the common case is one keystroke. */
  default: string | null;
}

/**
 * A saved command with its holes already parsed.
 *
 * Parsed in Rust, not here: a second implementation of what counts as a hole would
 * eventually disagree about `${HOME}` or `awk '{print $1}'`, and the disagreement would
 * show up as a corrupted command.
 */
export interface SavedCommandView {
  id: string;
  name: string;
  template: string;
  description: string | null;
  uses: number;
  parameters: SavedParameter[];
}

export const savedCommands = () => invoke<SavedCommandView[]>("saved_commands");

export const savedCommandUpsert = (name: string, template: string, description: string) =>
  invoke<void>("saved_command_upsert", { name, template, description });

export const savedCommandDelete = (id: string) => invoke<void>("saved_command_delete", { id });

/** Fill the holes and note the use. Returns the line to type. */
export const savedCommandRender = (
  id: string,
  template: string,
  values: [string, string][],
) => invoke<string>("saved_command_render", { id, template, values });

export const workspaceSave = (id: string, name: string, json: string) =>
  invoke<void>("workspace_save", { id, name, json });

export const workspaceLoad = (id: string) =>
  invoke<string | null>("workspace_load", { id });

/**
 * Save a pane's visible history, keyed by the pane id the session records.
 *
 * `program` is stored alongside it so a load cannot hand a local shell's output to what
 * is now an SSH session.
 */
export const scrollbackSave = (
  paneKey: string,
  program: string | null,
  cwd: string | null,
  body: string,
) => invoke<void>("scrollback_save", { paneKey, program, cwd, body });

/** Load a pane's saved history. Null when none was saved, or the program differs. */
export const scrollbackLoad = (paneKey: string, program: string | null) =>
  invoke<string | null>("scrollback_load", { paneKey, program });

/**
 * Tell the backend whether the terminal background is dark.
 *
 * Programs that enabled DEC mode 2031 are sent a report when this changes, and any
 * program asking `CSI ? 996 n` is answered with it. Returns how many panes were told.
 */
export const colorSchemeSet = (dark: boolean) =>
  invoke<number>("color_scheme_set", { dark });

/** A directory offered for `cd`, ranked by match and frecency. */
export interface DirSuggestion {
  path: string;
  name: string;
  visits: number;
  age_hours: number;
  /** True when the directory no longer exists. */
  missing: boolean;
}

/**
 * Directories a pane has actually sat in, ranked for `query`.
 *
 * An empty query gives "where I usually am"; a typed one gives "the thing I mean".
 */
export const recentDirectories = (query: string, limit = 30) =>
  invoke<DirSuggestion[]>("recent_directories", { query, limit });

/** Forget a directory that no longer exists. */
export const forgetDirectory = (path: string) =>
  invoke<void>("forget_directory", { path });

/** Forget saved history for panes the session no longer contains. */
export const scrollbackRetain = (paneKeys: string[]) =>
  invoke<number>("scrollback_retain", { paneKeys });

export const setProjectRoot = (path: string) =>
  invoke<string>("set_project_root", { path });

// ------------------------------------------------------------ connections

export interface ShellProfile {
  id: string;
  name: string;
  program: string;
  args: string[];
  env: [string, string][];
  cwd: string | null;
  supports_integration: boolean;
}

export interface SshHostInfo {
  alias: string;
  hostname: string | null;
  user: string | null;
  port: number | null;
  identity_file: string | null;
  proxy_jump: string | null;
  proxy_command: string | null;
  forward_agent: boolean | null;
  request_tty: string | null;
  source_file: string | null;
  is_pattern: boolean;
}

export interface MultiplexerSession {
  program: "tmux" | "zellij";
  name: string;
  detail: string | null;
  attached: boolean;
}

export interface SerialDevice {
  path: string;
  label: string;
}

export interface WslDistribution {
  name: string;
  is_default: boolean;
}

/**
 * What Tervin learned about reaching an SSH host.
 *
 * `open.connect_ms` is the time to establish TCP — **not** SSH round-trip time. SSH
 * exposes no round-trip time, so a number labelled "latency" would be a measurement of
 * something else. The UI repeats the distinction rather than smoothing it over.
 */
export type Reachability =
  | { state: "multiplexed" }
  | { state: "open"; connect_ms: number }
  | { state: "refused" }
  | { state: "timeout" }
  | { state: "unresolved" }
  | { state: "skipped"; reason: string };

/**
 * Whether a host's key is loaded in the SSH agent.
 *
 * Tervin never stores a passphrase. The problem worth solving is narrower: knowing a
 * connection is about to ask for one, before it asks. Identity is established by
 * fingerprint, computed from the *public* key, so no private key is ever read.
 */
export type KeyStatus =
  | { status: "loaded"; comment: string }
  | { status: "not_loaded"; path: string }
  | { status: "no_identity_named" }
  | { status: "cannot_fingerprint"; path: string; reason: string }
  | { status: "unknown" };

/** One call for every host: `ssh-add -l` is a single question. */
export const sshKeyStatus = () => invoke<[string, KeyStatus][]>("ssh_key_status");

/** Probe one host, on demand. Never called for a whole config at once. */
export const sshProbe = (alias: string) => invoke<Reachability>("ssh_probe", { alias });

export interface Connections {
  shells: ShellProfile[];
  ssh_hosts: SshHostInfo[];
  ssh_warnings: string[];
  multiplexers: MultiplexerSession[];
  serial: SerialDevice[];
  wsl: WslDistribution[];
}

/** What a pane can be attached to. Mirrors `session_manager::SessionKind`. */
export type SessionKind =
  | { kind: "shell"; profile_id: string }
  | { kind: "ssh"; alias: string }
  | { kind: "multiplexer"; program: "tmux" | "zellij"; session: string }
  | { kind: "serial"; device: string; baud: number }
  | { kind: "wsl"; distribution: string }
  | { kind: "command"; program: string; args: string[] };

export interface LaunchSpec {
  program: string;
  args: string[];
  cwd: string | null;
  env: [string, string][];
  description: string;
}

export const connections = () => invoke<Connections>("connections");

export const connectionLaunchSpec = (kind: SessionKind, cwd?: string) =>
  invoke<LaunchSpec>("connection_launch_spec", { kind, cwd: cwd ?? null });

// ------------------------------------------------------------ path index

export type Want = "any" | "files" | "dirs";

export interface Completion {
  path: string;
  is_dir: boolean;
  score: number;
  /** Character indices in `path` that matched, for highlighting. */
  positions: number[];
}

export interface IndexStatus {
  root: string;
  files: number;
  directories: number;
  truncated: boolean;
  duration_ms: number;
}

export const pathComplete = (
  query: string,
  want: Want = "files",
  relativeTo?: string | null,
  limit = 12,
) =>
  invoke<Completion[]>("path_complete", {
    query,
    want,
    relativeTo: relativeTo ?? null,
    limit,
  });

export const pathIndexStatus = () => invoke<IndexStatus>("path_index_status");
export const pathIndexRebuild = () => invoke<IndexStatus>("path_index_rebuild");

/**
 * Find the `@` that starts a path reference before the cursor.
 *
 * Mirrors `file_index::at_path_query` so the composer can decide whether to open
 * the picker without a round trip on every keystroke. Only a `@` at the start or
 * after whitespace counts, so an email address does not open a file picker.
 */
export function atPathQuery(
  input: string,
  cursor: number,
): { at: number; query: string } | null {
  const before = input.slice(0, Math.min(cursor, input.length));
  const at = before.lastIndexOf("@");
  if (at < 0) return null;
  if (at > 0 && !/\s/.test(before[at - 1] ?? "")) return null;
  const query = before.slice(at + 1);
  if (/\s/.test(query)) return null;
  return { at, query };
}

// -------------------------------------------------------------- ui logging

/**
 * Send a UI message to Tervin's log.
 *
 * Fire-and-forget and never throwing: this is what gets called when something has
 * already gone wrong, so it must not be able to make things worse.
 */
export function uiLog(
  level: "error" | "warn" | "info" | "debug",
  message: string,
  detail?: string,
): void {
  try {
    void invoke("ui_log", { level, message, detail: detail ?? null }).catch(() => {});
  } catch {
    // No bridge (a plain browser). Nothing to do.
  }
}

// ----------------------------------------------------------------- events

/**
 * Subscribe to a backend event.
 *
 * Never rejects. A subscription can fail for exactly one interesting reason —
 * the window's capability file does not grant `core:event:allow-listen` — and
 * that failure used to surface as a wall of unhandled promise rejections that
 * buried the actual cause. Now it is logged once, in words, and the app keeps
 * running with that one feed dead.
 */
export function on<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
  return listen<T>(event, (e) => handler(e.payload)).catch((error) => {
    uiLog(
      "error",
      `Could not subscribe to "${event}"`,
      `${String(error)}\n\nIf this mentions an ACL, the window capability file is missing a permission.`,
    );
    // A no-op unsubscribe, so callers' cleanup paths stay uniform.
    return () => {};
  });
}

/** True when running inside the Tauri shell rather than a plain browser tab. */
export const isDesktop = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
