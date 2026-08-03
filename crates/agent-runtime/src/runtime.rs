//! The `AgentRuntime` interface.
//!
//! Every agent Tervin hosts arrives through this interface, and nothing above it
//! knows which agent is running. An adapter's job is to translate its runtime's
//! dialect into Tervin's event vocabulary and to report honestly what it cannot
//! do — an adapter that claims a capability it lacks breaks a promise the UI has
//! already made to the user.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tervin_core::{AgentIdentity, Capabilities, TervinEvent, ThreadId};
use tokio::sync::mpsc;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("`{0}` was not found on PATH")]
    NotInstalled(String),
    #[error("failed to start {runtime}: {source}")]
    Launch {
        runtime: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{runtime} does not support {feature}")]
    Unsupported { runtime: String, feature: String },
    #[error("the session has ended")]
    SessionEnded,
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

/// What `discover()` found about a runtime on this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discovery {
    pub runtime_id: String,
    pub display_name: String,
    pub available: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    /// Anything the user should know, such as a version too old for structured
    /// events. Shown verbatim in the Bridge panel.
    pub notes: Vec<String>,
    pub capabilities: Capabilities,
}

/// Something attached to a prompt as explicit context.
///
/// Attachments are always explicit. Tervin never quietly ships scrollback, files,
/// or environment values to a provider — that is the whole of the local-first
/// privacy promise, and it is enforced by there being no other path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    /// A Block's command and output.
    Block {
        block_id: tervin_core::BlockId,
        command: String,
        output: String,
    },
    File {
        path: String,
    },
    /// A unified diff.
    Diff {
        path: String,
        patch: String,
    },
    /// Free text the user selected in the terminal.
    Selection {
        text: String,
    },
    Image {
        media_type: String,
        /// Base64-encoded bytes.
        data: String,
    },
}

impl Attachment {
    /// How the attachment is described in the timeline.
    pub fn describe(&self) -> String {
        match self {
            Self::Block { command, .. } => format!("block: {command}"),
            Self::File { path } => format!("file: {path}"),
            Self::Diff { path, .. } => format!("diff: {path}"),
            Self::Selection { text } => {
                format!("selection ({} chars)", text.chars().count())
            }
            Self::Image { media_type, .. } => format!("image ({media_type})"),
        }
    }

    /// Rendered into prompt text. Images are handled by the adapter separately.
    pub fn to_prompt_text(&self) -> Option<String> {
        match self {
            Self::Block {
                command, output, ..
            } => Some(format!(
                "Command that was run in Tervin:\n```\n$ {command}\n{output}\n```"
            )),
            Self::File { path } => Some(format!("Relevant file: {path}")),
            Self::Diff { path, patch } => Some(format!("Diff for {path}:\n```diff\n{patch}\n```")),
            Self::Selection { text } => {
                Some(format!("Selected terminal output:\n```\n{text}\n```"))
            }
            Self::Image { .. } => None,
        }
    }
}

/// How to start a Thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchConfig {
    pub thread_id: ThreadId,
    pub cwd: String,
    /// Executable to run instead of the adapter's default.
    ///
    /// This is what makes a profile mean something: two profiles can drive the same
    /// adapter against different installs. Without it a profile could only change
    /// environment, and "point Tervin at this build" would be unexpressible.
    pub binary: Option<String>,
    /// Arguments placed before the adapter's own.
    pub extra_args: Vec<String>,
    /// Initial prompt, if the Thread starts with one.
    pub prompt: Option<String>,
    pub attachments: Vec<Attachment>,
    /// Runtime-specific model selector, when the runtime supports choosing.
    pub model: Option<String>,
    /// Runtime-specific reasoning effort, when the runtime supports choosing.
    pub effort: Option<String>,
    /// Runtime-specific permission mode.
    pub permission_mode: Option<String>,
    /// Tool patterns Tervin Rules pre-authorises, passed to the runtime so policy
    /// applies before anything runs rather than after.
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub task_title: Option<String>,
    /// Extra environment for the child process.
    ///
    /// An empty value means **remove this variable**, not "set it to empty". A
    /// profile clears account-selecting variables so an ambient value cannot decide
    /// which account runs, and `CLAUDE_CONFIG_DIR=""` is not the same as unset — one
    /// is an empty path, the other is absence. Use [`apply_env`] rather than
    /// `Command::envs`.
    pub env: Vec<(String, String)>,
}

/// Apply a launch environment to a command, honouring removals.
///
/// Exists so no adapter can reintroduce the empty-string-versus-unset bug: passing
/// these pairs straight to `Command::envs` sets variables to empty strings, which a
/// runtime may read as a real, empty value.
pub fn apply_env(command: &mut tokio::process::Command, env: &[(String, String)]) {
    for (key, value) in env {
        if value.is_empty() {
            command.env_remove(key);
        } else {
            command.env(key, value);
        }
    }
}

impl LaunchConfig {
    pub fn new(thread_id: ThreadId, cwd: impl Into<String>) -> Self {
        Self {
            thread_id,
            cwd: cwd.into(),
            binary: None,
            extra_args: Vec::new(),
            prompt: None,
            attachments: Vec::new(),
            model: None,
            effort: None,
            permission_mode: None,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            task_title: None,
            env: Vec::new(),
        }
    }

    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = Some(prompt.into());
        self
    }
}

/// Live facts about a running session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// The runtime's own session identifier, used for resuming.
    pub resume_id: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub runtime_version: Option<String>,
    /// Tool names the runtime reported having.
    pub tools: Vec<String>,
    /// MCP servers and their connection state, for the Bridge panel.
    pub mcp_servers: Vec<McpServerState>,
    /// Slash commands the runtime accepts, for composer autocomplete.
    pub slash_commands: Vec<String>,
    /// The user's own hooks, as they actually ran.
    ///
    /// Hooks are the most invisible part of a Claude Code setup: they run silently,
    /// and a broken one degrades the session with no message anywhere. Recording
    /// them makes that inspectable.
    #[serde(default)]
    pub hook_runs: Vec<HookRun>,
    /// Permission or operating modes this session will accept.
    ///
    /// Reported rather than hard-coded in the UI, because they differ per runtime
    /// and, under ACP, per agent. A control offering a mode the agent would reject
    /// is worse than no control.
    #[serde(default)]
    pub modes: Vec<SessionMode>,
    /// Project instruction files the runtime says it loaded.
    pub instruction_sources: Vec<String>,
}

/// One execution of one of the user's hooks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRun {
    /// As the runtime names it, e.g. `PreToolUse:Bash`.
    pub name: String,
    /// The lifecycle event it ran for, e.g. `SessionStart`.
    pub event: String,
    pub exit_code: i32,
    /// `success`, or whatever the runtime reported.
    pub outcome: String,
    /// Anything the hook wrote to stderr, which is where a failing hook explains
    /// itself and where a blocking one states its reason.
    pub message: Option<String>,
    /// True for Tervin's own gate, so the UI can tell the two apart rather than
    /// presenting Tervin's work as the user's configuration.
    pub is_tervin: bool,
}

/// One selectable mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMode {
    /// Passed back to `set_permission_mode` verbatim.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl SessionMode {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
        }
    }

    pub fn described(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerState {
    pub name: String,
    pub status: String,
}

/// How permissions actually work for a live session.
///
/// This is what the UI reads to decide whether to present Tervin's own approval
/// sheet or to explain that the runtime is deciding for itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionState {
    pub mode: String,
    /// True only when Tervin can block an action before it executes.
    pub tervin_can_intercept: bool,
    /// One sentence describing who actually decides, shown in the UI.
    pub explanation: String,
    /// Actions the runtime reported refusing.
    pub denials: Vec<String>,
}

/// A problem with the runtime itself, as opposed to with the user's code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeDiagnostic {
    pub severity: tervin_core::events::Severity,
    pub message: String,
    pub at: tervin_core::Timestamp,
}

/// Consulted when a runtime asks Tervin for permission before acting.
///
/// Only some runtimes can ask. Where one can, Tervin Rules become a real gate;
/// where none can, this is never called and the session reports its permissions
/// as provider-native. The distinction is never blurred.
#[async_trait]
pub trait PermissionArbiter: Send + Sync {
    async fn decide(
        &self,
        thread_id: &ThreadId,
        tool_name: &str,
        input: &serde_json::Value,
        cwd: &str,
    ) -> ArbiterDecision;
}

#[derive(Debug, Clone)]
pub enum ArbiterDecision {
    Allow,
    Deny { reason: String },
}

/// A discoverable, launchable agent runtime.
#[async_trait]
pub trait AgentRuntime: Send + Sync {
    fn runtime_id(&self) -> &str;
    fn identity(&self) -> AgentIdentity;

    /// Whether this runtime is installed and what it can do here.
    async fn discover(&self) -> Discovery;

    /// Static capability declaration, refined by `discover` and by a live session.
    fn capabilities(&self) -> Capabilities;

    /// The launch choices this runtime accepts, for controls shown *before* a
    /// session exists.
    ///
    /// Separate from the modes a live session reports, because the composer has to
    /// offer these when there is nothing running to ask. Declared by the adapter
    /// rather than listed in the UI for the same reason the mode picker is: an
    /// interface offering a choice the runtime would reject is worse than one
    /// offering none. A runtime that takes neither returns the default and the
    /// controls do not appear at all.
    fn launch_options(&self) -> LaunchOptions {
        LaunchOptions::default()
    }

    /// Start a new session.
    async fn launch(&self, config: LaunchConfig) -> Result<LaunchedSession>;

    /// Continue a previous session by its runtime-issued id.
    async fn resume(&self, resume_id: &str, config: LaunchConfig) -> Result<LaunchedSession>;
}

/// One option in a launch control, as the adapter defines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchChoice {
    /// What is passed to the runtime verbatim.
    pub value: String,
    /// What the picker shows.
    pub label: String,
    /// A caveat worth reading before choosing, such as what it costs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl LaunchChoice {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            note: None,
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// What a runtime accepts at launch. Empty means the control is not shown.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaunchOptions {
    pub models: Vec<LaunchChoice>,
    pub efforts: Vec<LaunchChoice>,
}

/// A session plus the event stream it produces.
///
/// The receiver is the `event_stream()` of the specified interface. It is handed
/// out once, at launch, because a Thread has exactly one consumer — the store,
/// which fans out to the UI.
pub struct LaunchedSession {
    pub session: Box<dyn AgentSession>,
    pub events: mpsc::UnboundedReceiver<TervinEvent>,
}

/// A running agent session.
#[async_trait]
pub trait AgentSession: Send + Sync {
    /// Send another turn of input.
    async fn send_input(&self, content: String, attachments: Vec<Attachment>) -> Result<()>;

    /// Ask the agent to stop what it is doing.
    async fn interrupt(&self) -> Result<()>;

    /// Change the permission mode mid-session, where the runtime allows it.
    async fn set_permission_mode(&self, mode: &str) -> Result<()>;

    fn session_metadata(&self) -> SessionMetadata;

    fn permissions(&self) -> PermissionState;

    fn diagnostics(&self) -> Vec<RuntimeDiagnostic>;

    /// Capabilities as now known, after any live probing.
    fn capabilities(&self) -> Capabilities;

    /// True while the underlying process is alive.
    fn is_running(&self) -> bool;

    /// End the session and reap the process.
    async fn shutdown(&self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_value_removes_a_variable_rather_than_emptying_it() {
        // The bug this guards against cost a real session: a profile clears
        // `CLAUDE_CONFIG_DIR` so an ambient value cannot decide which account runs,
        // but setting it to "" is an empty *path*, not absence — and the runtime
        // silently used the wrong account.
        let mut command = tokio::process::Command::new("/usr/bin/env");
        apply_env(
            &mut command,
            &[
                ("KEEP".to_string(), "value".to_string()),
                ("DROP".to_string(), String::new()),
            ],
        );

        let std_command = command.as_std();
        let envs: Vec<(String, Option<String>)> = std_command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_string(),
                    v.map(|v| v.to_string_lossy().to_string()),
                )
            })
            .collect();

        assert!(
            envs.contains(&("KEEP".to_string(), Some("value".to_string()))),
            "{envs:?}"
        );
        // `None` is a removal. `Some("")` would be the bug.
        assert!(
            envs.contains(&("DROP".to_string(), None)),
            "an empty value must remove the variable, got {envs:?}"
        );
    }

    #[test]
    fn a_launch_config_starts_with_no_binary_override_and_no_extra_args() {
        let config = LaunchConfig::new(tervin_core::ThreadId::new(), "/tmp");
        assert!(config.binary.is_none());
        assert!(config.extra_args.is_empty());
    }
}
