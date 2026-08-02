//! Tervin Threads: provider-independent coding-agent tasks.

use crate::{capability::Tier, events::CostSnapshot, ids::*, Timestamp};
use serde::{Deserialize, Serialize};

/// Observable lifecycle state of a Thread.
///
/// `Unknown` is a first-class, valid state. For a Tier 3 generic agent command
/// Tervin genuinely cannot infer what the process is doing, and saying so is
/// correct where guessing would be a quiet lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadState {
    Idle,
    Starting,
    AwaitingInput,
    Understanding,
    Planning,
    Reading,
    Editing,
    Executing,
    Testing,
    WaitingForPermission,
    WaitingForExternalTool,
    ReviewRequired,
    Completed,
    Failed,
    Interrupted,
    Disconnected,
    Unknown,
}

impl ThreadState {
    /// A short label for the status rail and Deck. Concrete, not decorative.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Starting => "Starting",
            Self::AwaitingInput => "Awaiting input",
            Self::Understanding => "Understanding",
            Self::Planning => "Planning",
            Self::Reading => "Reading",
            Self::Editing => "Editing",
            Self::Executing => "Executing",
            Self::Testing => "Testing",
            Self::WaitingForPermission => "Waiting for permission",
            Self::WaitingForExternalTool => "Waiting for external tool",
            Self::ReviewRequired => "Review required",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Interrupted => "Interrupted",
            Self::Disconnected => "Disconnected",
            Self::Unknown => "Unknown",
        }
    }

    /// True when the Thread will not progress without the user.
    pub fn needs_user(&self) -> bool {
        matches!(
            self,
            Self::AwaitingInput | Self::WaitingForPermission | Self::ReviewRequired
        )
    }

    /// True once the Thread has stopped for good.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Interrupted | Self::Disconnected
        )
    }

    /// True while work is actively happening, which is what the Deck counts as
    /// an "active agent".
    pub fn is_working(&self) -> bool {
        matches!(
            self,
            Self::Starting
                | Self::Understanding
                | Self::Planning
                | Self::Reading
                | Self::Editing
                | Self::Executing
                | Self::Testing
                | Self::WaitingForExternalTool
        )
    }

    /// Semantic colour role, resolved to a palette token by the UI. Threads map
    /// to state colours only — never to decoration.
    pub fn tone(&self) -> &'static str {
        if self.needs_user() {
            return "amber";
        }
        match self {
            Self::Completed => "green",
            Self::Failed => "red",
            Self::Interrupted | Self::Disconnected => "red",
            Self::Unknown => "muted",
            s if s.is_working() => "teal",
            _ => "muted",
        }
    }
}

/// Who is doing the work, and how much Tervin can see of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentIdentity {
    /// Stable runtime key, e.g. `claude-code`, `codex`, `aider`, `generic`.
    pub runtime_id: String,
    /// Customer-facing name, e.g. `Claude Code`.
    pub display_name: String,
    pub tier: Tier,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl AgentIdentity {
    pub fn new(runtime_id: impl Into<String>, display_name: impl Into<String>, tier: Tier) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            display_name: display_name.into(),
            tier,
            model: None,
            version: None,
        }
    }

    /// Tervin itself, as the actor for shell work the user drove directly.
    pub fn tervin() -> Self {
        Self::new("tervin", "Tervin", Tier::Structured)
    }
}

/// The relationship of a Thread to a parent task, where the runtime reports one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadLineage {
    pub parent: Option<ThreadId>,
    #[serde(default)]
    pub children: Vec<ThreadId>,
}

/// A Thread's durable record. Field-for-field what a Thread must store, kept
/// free of any provider-specific shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: ThreadId,
    pub agent: AgentIdentity,
    pub state: ThreadState,

    pub project: Option<String>,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub worktree: Option<String>,
    pub host: String,

    pub task_title: String,
    pub lineage: ThreadLineage,
    /// The pane hosting this Thread's terminal, when it has one.
    pub pane_id: Option<PaneId>,

    /// Runtime-issued handle for resuming, when the runtime supports resuming.
    pub resume_id: Option<String>,
    pub permission_mode: String,

    pub created_at: Timestamp,
    pub updated_at: Timestamp,

    pub cost: CostSnapshot,
    /// Count of blocks, files, diffs, diagnostics and tests attributed to this
    /// Thread. The records themselves live in their own stores and are joined
    /// on demand, so a Thread row stays small.
    pub counts: ThreadCounts,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadCounts {
    pub events: u32,
    pub blocks: u32,
    pub files_changed: u32,
    pub diagnostics: u32,
    pub tests: u32,
    pub pending_approvals: u32,
}

impl Thread {
    pub fn new(
        agent: AgentIdentity,
        cwd: impl Into<String>,
        task_title: impl Into<String>,
    ) -> Self {
        let now = crate::now();
        Self {
            id: ThreadId::new(),
            agent,
            state: ThreadState::Idle,
            project: None,
            cwd: cwd.into(),
            git_branch: None,
            worktree: None,
            host: "local".to_string(),
            task_title: task_title.into(),
            lineage: ThreadLineage {
                parent: None,
                children: Vec::new(),
            },
            pane_id: None,
            resume_id: None,
            permission_mode: "default".to_string(),
            created_at: now,
            updated_at: now,
            cost: CostSnapshot::default(),
            counts: ThreadCounts::default(),
        }
    }
}
