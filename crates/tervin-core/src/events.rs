//! The unified, append-only event stream.
//!
//! Every runtime — a Tier 1 structured agent, a Tier 2 CLI whose output we
//! parse, or a Tier 3 command in a managed pane — normalises into exactly these
//! events. The UI reads only this vocabulary, which is why adding an adapter
//! never requires touching a view.
//!
//! Events are append-only. Nothing in Tervin rewrites history: a superseded
//! plan is followed by a new `plan.proposed`, never an edit of the old one.

use crate::{
    capability::Tier,
    ids::*,
    risk::RiskAssessment,
    thread::{AgentIdentity, ThreadState},
    Timestamp,
};
use serde::{Deserialize, Serialize};

/// A single normalised event with its full provenance envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TervinEvent {
    pub id: EventId,
    pub thread_id: Option<ThreadId>,
    pub ts: Timestamp,
    pub agent: AgentIdentity,
    pub project: Option<String>,
    pub cwd: Option<String>,
    /// One concise line, written for a human scanning a timeline. Never a dump
    /// of the payload — the payload is available separately and on demand.
    pub summary: String,
    /// Pointer to the untouched runtime payload. Held by reference rather than
    /// inline so a timeline stays cheap to render and so redaction has a single
    /// place to happen.
    pub raw: Option<RawRef>,
    pub links: Vec<Link>,
    pub payload: EventPayload,
}

impl TervinEvent {
    pub fn new(agent: AgentIdentity, summary: impl Into<String>, payload: EventPayload) -> Self {
        Self {
            id: EventId::new(),
            thread_id: None,
            ts: crate::now(),
            agent,
            project: None,
            cwd: None,
            summary: summary.into(),
            raw: None,
            links: Vec::new(),
            payload,
        }
    }

    pub fn with_thread(mut self, thread_id: ThreadId) -> Self {
        self.thread_id = Some(thread_id);
        self
    }

    pub fn with_location(mut self, project: Option<String>, cwd: Option<String>) -> Self {
        self.project = project;
        self.cwd = cwd;
        self
    }

    pub fn with_raw(mut self, raw: RawRef) -> Self {
        self.raw = Some(raw);
        self
    }

    pub fn with_links(mut self, links: Vec<Link>) -> Self {
        self.links = links;
        self
    }

    /// The stable wire name, matching the specified event vocabulary exactly.
    pub fn kind(&self) -> &'static str {
        self.payload.kind()
    }
}

/// A safe handle to a raw runtime payload.
///
/// `redacted` records whether secret-shaped values were stripped before the
/// payload was stored, so an export can state what it did rather than implying
/// the original was clean.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawRef {
    /// Source dialect, e.g. `claude-code/stream-json`.
    pub kind: String,
    /// Opaque lookup key into the event store.
    pub pointer: String,
    pub byte_len: usize,
    pub redacted: bool,
}

/// A typed cross-reference from an event to something inspectable.
///
/// These are what make principle 4 (every action is inspectable) mechanical
/// rather than aspirational: a timeline row can always be followed to the exact
/// block, hunk, or diagnostic it describes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum Link {
    Block {
        block_id: BlockId,
    },
    Pane {
        pane_id: PaneId,
    },
    File {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
    },
    /// A specific hunk within a specific file's diff.
    DiffHunk {
        path: String,
        hunk_index: usize,
    },
    Diagnostic {
        diagnostic_id: DiagnosticId,
    },
    Test {
        suite: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        case: Option<String>,
    },
    Artifact {
        artifact_id: ArtifactId,
    },
    Commit {
        sha: String,
    },
    Url {
        url: String,
    },
}

/// A file change attributed to an agent, as reported or observed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub kind: FileChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_lines: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_lines: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcome {
    Passed,
    Failed,
    Skipped,
    Errored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// Who actually decided a permission outcome.
///
/// This distinction is required rather than cosmetic: Tervin must never present
/// a provider's own approval as though Tervin gated it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAuthority {
    /// Tervin Rules gated the action before it could run.
    Tervin,
    /// The agent runtime's own permission system decided; Tervin observed it.
    ProviderNative,
    /// A standing Tervin policy rule matched, with no interactive prompt.
    TervinPolicy,
}

/// Token and cost accounting, all fields optional because most runtimes report
/// only some of it and Tervin must not invent the rest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_used: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// One step of a proposed plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touches: Vec<String>,
}

/// The event payloads, tagged with the exact specified wire names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventPayload {
    #[serde(rename = "thread.started")]
    ThreadStarted {
        tier: Tier,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resume_id: Option<String>,
    },

    #[serde(rename = "user.prompted")]
    UserPrompted { text: String },

    #[serde(rename = "context.attached")]
    ContextAttached {
        /// What was attached, described plainly: "3 blocks", "diff of src/x.rs".
        description: String,
        kinds: Vec<String>,
    },

    #[serde(rename = "agent.message")]
    AgentMessage {
        text: String,
        /// Reasoning traces are marked so the UI can keep them collapsed by
        /// default without discarding them.
        #[serde(default)]
        is_reasoning: bool,
        /// Set when the message came from a subagent rather than the root task.
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },

    #[serde(rename = "plan.proposed")]
    PlanProposed {
        steps: Vec<PlanStep>,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_text: Option<String>,
    },

    #[serde(rename = "plan.approved")]
    PlanApproved { authority: DecisionAuthority },

    #[serde(rename = "tool.requested")]
    ToolRequested {
        tool_use_id: String,
        tool_name: String,
        /// Compact, human-readable rendering of the arguments.
        input_summary: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_use_id: Option<String>,
    },

    #[serde(rename = "tool.completed")]
    ToolCompleted {
        tool_use_id: String,
        tool_name: String,
        is_error: bool,
        output_summary: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },

    #[serde(rename = "command.proposed")]
    CommandProposed {
        command: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        risk: RiskAssessment,
    },

    #[serde(rename = "command.started")]
    CommandStarted {
        command: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        block_id: Option<BlockId>,
    },

    #[serde(rename = "command.output")]
    CommandOutput {
        stream: OutputStream,
        /// Bounded excerpt. The authoritative full output lives on the Block.
        excerpt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        block_id: Option<BlockId>,
    },

    #[serde(rename = "command.completed")]
    CommandCompleted {
        command: String,
        exit_code: i32,
        duration_ms: u64,
        /// Whether `exit_code` is what the runtime reported, or a value Tervin derived
        /// from a success flag.
        ///
        /// The distinction matters once these become Blocks. An ACP terminal reports a
        /// real status; Claude Code reports only success or failure, and the 0/1/130 in
        /// `exit_code` is then Tervin's inference. A Block showing "exit 1" that no
        /// runtime ever said is worse than one showing no exit code at all, so a Block
        /// built from a derived value carries none.
        ///
        /// Defaults to false, which is the safe direction: an event from an older build
        /// reads as "not reported" rather than as authoritative.
        #[serde(default)]
        exit_code_reported: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        block_id: Option<BlockId>,
    },

    #[serde(rename = "file.read")]
    FileRead {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        lines: Option<u32>,
    },

    #[serde(rename = "file.changed")]
    FileChanged { change: FileChange },

    #[serde(rename = "patch.proposed")]
    PatchProposed {
        files: Vec<FileChange>,
        #[serde(skip_serializing_if = "Option::is_none")]
        unified_diff: Option<String>,
    },

    #[serde(rename = "patch.applied")]
    PatchApplied {
        files: Vec<FileChange>,
        authority: DecisionAuthority,
    },

    #[serde(rename = "git.changed")]
    GitChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        dirty: bool,
        changed_files: u32,
        /// Commits Tervin did not initiate are surfaced explicitly rather than
        /// folded silently into working-tree state.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        external_commits: Vec<String>,
    },

    #[serde(rename = "test.started")]
    TestStarted {
        suite: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        block_id: Option<BlockId>,
    },

    #[serde(rename = "test.completed")]
    TestCompleted {
        suite: String,
        outcome: TestOutcome,
        passed: u32,
        failed: u32,
        skipped: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        block_id: Option<BlockId>,
    },

    #[serde(rename = "diagnostic.detected")]
    DiagnosticDetected {
        diagnostic_id: DiagnosticId,
        severity: Severity,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },

    #[serde(rename = "permission.requested")]
    PermissionRequested {
        request_id: RequestId,
        action: String,
        risk: RiskAssessment,
        /// False when Tervin can observe the request but cannot block it. The UI
        /// must say so rather than implying a gate exists.
        interceptable: bool,
    },

    #[serde(rename = "permission.granted")]
    PermissionGranted {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        action: String,
        authority: DecisionAuthority,
        scope: String,
    },

    #[serde(rename = "permission.denied")]
    PermissionDenied {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<RequestId>,
        action: String,
        authority: DecisionAuthority,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    #[serde(rename = "artifact.created")]
    ArtifactCreated {
        artifact_id: ArtifactId,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        byte_len: Option<u64>,
    },

    #[serde(rename = "cost.updated")]
    CostUpdated { snapshot: CostSnapshot },

    #[serde(rename = "thread.completed")]
    ThreadCompleted {
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cost: Option<CostSnapshot>,
    },

    #[serde(rename = "thread.failed")]
    ThreadFailed {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        recoverable: Option<bool>,
    },

    /// A normalised state transition. Not in the specified list, but required so
    /// the UI can render Thread state without re-deriving it from every event.
    #[serde(rename = "thread.state")]
    ThreadState { state: ThreadState },

    /// An event the adapter received but could not confidently classify. Kept
    /// rather than dropped, and shown as unclassified rather than guessed at.
    #[serde(rename = "runtime.unclassified")]
    RuntimeUnclassified { source_type: String },
}

impl EventPayload {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ThreadStarted { .. } => "thread.started",
            Self::UserPrompted { .. } => "user.prompted",
            Self::ContextAttached { .. } => "context.attached",
            Self::AgentMessage { .. } => "agent.message",
            Self::PlanProposed { .. } => "plan.proposed",
            Self::PlanApproved { .. } => "plan.approved",
            Self::ToolRequested { .. } => "tool.requested",
            Self::ToolCompleted { .. } => "tool.completed",
            Self::CommandProposed { .. } => "command.proposed",
            Self::CommandStarted { .. } => "command.started",
            Self::CommandOutput { .. } => "command.output",
            Self::CommandCompleted { .. } => "command.completed",
            Self::FileRead { .. } => "file.read",
            Self::FileChanged { .. } => "file.changed",
            Self::PatchProposed { .. } => "patch.proposed",
            Self::PatchApplied { .. } => "patch.applied",
            Self::GitChanged { .. } => "git.changed",
            Self::TestStarted { .. } => "test.started",
            Self::TestCompleted { .. } => "test.completed",
            Self::DiagnosticDetected { .. } => "diagnostic.detected",
            Self::PermissionRequested { .. } => "permission.requested",
            Self::PermissionGranted { .. } => "permission.granted",
            Self::PermissionDenied { .. } => "permission.denied",
            Self::ArtifactCreated { .. } => "artifact.created",
            Self::CostUpdated { .. } => "cost.updated",
            Self::ThreadCompleted { .. } => "thread.completed",
            Self::ThreadFailed { .. } => "thread.failed",
            Self::ThreadState { .. } => "thread.state",
            Self::RuntimeUnclassified { .. } => "runtime.unclassified",
        }
    }

    /// Whether this event should pull a user's attention. Used to keep the Deck
    /// and notifications quiet: state churn and reasoning are not notable.
    pub fn is_notable(&self) -> bool {
        matches!(
            self,
            Self::PermissionRequested { .. }
                | Self::ThreadFailed { .. }
                | Self::ThreadCompleted { .. }
                | Self::PlanProposed { .. }
                | Self::PatchProposed { .. }
                | Self::TestCompleted {
                    outcome: TestOutcome::Failed | TestOutcome::Errored,
                    ..
                }
        )
    }
}
