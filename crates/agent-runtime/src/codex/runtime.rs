//! Launching Codex, and being honest about the shape of it.
//!
//! `codex exec` is a **one-shot invocation**, not a long-lived session. That single fact
//! decides almost everything here:
//!
//! - A turn is a process. It starts, streams JSONL, and exits.
//! - A follow-up is a *new* process — `codex exec resume <thread_id> <prompt>` — which is
//!   why multi-turn works at all, and why it is Partial rather than Supported: the model
//!   keeps its context, but nothing is held open in between.
//! - There is no approval channel. `codex exec` has no `--ask-for-approval`, only
//!   `--dangerously-bypass-approvals-and-sandbox`. So Tervin cannot gate Codex, and it
//!   says so rather than showing a control that would never fire.
//!
//! ## What Tervin deliberately does not pass
//!
//! Not `--dangerously-bypass-approvals-and-sandbox`, and not `--dangerously-bypass-hook-trust`.
//! Codex's own sandbox is the only thing standing between an agent and the filesystem here,
//! since Tervin has no gate of its own to substitute. Turning it off to make Tervin's
//! integration look smoother would be trading the user's safety for our convenience.
//!
//! ## stdin is closed on purpose
//!
//! Observed, not assumed: with a prompt given as an argument *and* stdin left as a pipe,
//! `codex exec` prints "Reading additional input from stdin..." and appends whatever
//! arrives as a `<stdin>` block. An adapter that left stdin open would either hang or
//! silently extend the user's prompt with nothing.

use super::normalize::CodexNormalizer;
use crate::runtime::{
    AgentRuntime, AgentSession, Attachment, Discovery, LaunchConfig, LaunchedSession,
    PermissionState, Result, RuntimeDiagnostic, RuntimeError, SessionMetadata,
};
use async_trait::async_trait;
use parking_lot::Mutex;
use std::process::Stdio;
use std::sync::Arc;
use tervin_core::capability::{Capabilities, CapabilityLevel};
use tervin_core::{AgentIdentity, EventPayload, TervinEvent, ThreadState, Tier};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// The default executable.
pub const DEFAULT_BINARY: &str = "codex";

/// Lines of stderr kept as diagnostics.
///
/// Codex logs a line per reconnect attempt and can produce a great many; the newest are
/// the ones that explain a failure.
const MAX_DIAGNOSTICS: usize = 40;

pub struct CodexRuntime;

impl CodexRuntime {
    pub fn new() -> Self {
        Self
    }

    fn identity_for(version: Option<String>) -> AgentIdentity {
        let mut identity = AgentIdentity::new("codex", "Codex", Tier::Structured);
        identity.version = version;
        identity
    }
}

impl Default for CodexRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// What Codex can and cannot do, each stated with its reason.
///
/// Nothing here is aspirational. Where a capability is absent the note says what would
/// have to change, because "unsupported" with no reason is indistinguishable from "not
/// implemented yet" and users deserve to know which.
pub fn codex_capabilities() -> Capabilities {
    Capabilities {
        tier: Tier::Structured,
        // Codex emits a `plan` item, so Tervin can *show* a plan. It cannot ask for one:
        // there is no plan-mode flag on `exec`.
        plan_mode: CapabilityLevel::Partial {
            note: "Codex reports a plan when it makes one, but `codex exec` has no way to ask for plan mode.".to_string(),
        },
        resume: CapabilityLevel::Supported,
        tool_events: CapabilityLevel::Supported,
        file_edits: CapabilityLevel::Supported,
        // The one that matters most, and the one an integration is most tempted to
        // overstate.
        native_permission_bridge: CapabilityLevel::Unsupported {
            reason: "`codex exec` is non-interactive: it has no approval request to answer, so Tervin Rules cannot gate it. Codex's own sandbox decides, and Tervin does not disable it.".to_string(),
        },
        mcp: CapabilityLevel::Partial {
            note: "Codex loads MCP servers from its own config; Tervin does not forward its list to it.".to_string(),
        },
        hooks: CapabilityLevel::Unsupported {
            reason: "Codex has its own hook system with its own trust model, which Tervin does not drive.".to_string(),
        },
        subagents: CapabilityLevel::Unknown,
        image_input: CapabilityLevel::Partial {
            note: "Images can be attached to the first prompt with `-i`, but not to a resumed turn.".to_string(),
        },
        cost_reporting: CapabilityLevel::Partial {
            note: "Codex reports token counts, never money. Tervin shows the tokens rather than deriving a price from a list that changes.".to_string(),
        },
        model_selection: CapabilityLevel::Supported,
        remote_execution: CapabilityLevel::Unknown,
        // Real, but worth qualifying: each turn is a fresh process resuming the thread.
        multi_turn: CapabilityLevel::Partial {
            note: "Each turn is a separate `codex exec resume`. The conversation continues; nothing is held open between turns.".to_string(),
        },
        interrupt: CapabilityLevel::Partial {
            note: "Tervin kills the running turn. Codex is not asked to wind down first, because a one-shot invocation has nothing to ask.".to_string(),
        },
    }
}

#[async_trait]
impl AgentRuntime for CodexRuntime {
    fn runtime_id(&self) -> &str {
        "codex"
    }

    fn identity(&self) -> AgentIdentity {
        Self::identity_for(None)
    }

    async fn discover(&self) -> Discovery {
        let mut notes = Vec::new();
        let output = Command::new(DEFAULT_BINARY)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .await;

        let (available, version) = match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                // `codex-cli 0.146.0` — take the version, not the whole line.
                let version = text.split_whitespace().last().map(str::to_string);
                (true, version)
            }
            Ok(_) => {
                notes.push(
                    "`codex --version` failed, so Tervin cannot tell which version is installed."
                        .to_string(),
                );
                (true, None)
            }
            Err(_) => (false, None),
        };

        if !available {
            notes.push(
                "Codex is not on PATH. Install it with `npm i -g @openai/codex`.".to_string(),
            );
        } else {
            notes.push(
                "Tervin reads Codex but cannot gate it: `codex exec` has no approval request to answer. Codex's own sandbox decides what runs."
                    .to_string(),
            );
        }

        Discovery {
            runtime_id: "codex".to_string(),
            display_name: "Codex".to_string(),
            available,
            version: version.clone(),
            path: which(DEFAULT_BINARY).await,
            notes,
            capabilities: codex_capabilities(),
        }
    }

    fn capabilities(&self) -> Capabilities {
        codex_capabilities()
    }

    async fn launch(&self, config: LaunchConfig) -> Result<LaunchedSession> {
        start(config, None).await
    }

    async fn resume(&self, resume_id: &str, config: LaunchConfig) -> Result<LaunchedSession> {
        start(config, Some(resume_id.to_string())).await
    }
}

/// Spawn one turn, and hand back the session that owns the rest of them.
async fn start(config: LaunchConfig, resume: Option<String>) -> Result<LaunchedSession> {
    let (tx, rx) = mpsc::unbounded_channel();
    let binary = config
        .binary
        .clone()
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BINARY.to_string());

    let session = Arc::new(CodexSession {
        binary,
        cwd: config.cwd.clone(),
        model: config.model.clone(),
        extra_args: config.extra_args.clone(),
        env: config.env.clone(),
        events: tx,
        state: Mutex::new(Shared {
            normalizer: CodexNormalizer::new(
                CodexRuntime::identity_for(None),
                config.cwd.clone(),
                // The project name is derived from the directory: `LaunchConfig` does not
                // carry one, and the app layer stamps it on the Thread instead.
                std::path::Path::new(&config.cwd)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(str::to_string),
            ),
            thread_id: resume.clone(),
            diagnostics: Vec::new(),
            running: false,
            last_exit: None,
        }),
        child: tokio::sync::Mutex::new(None),
    });

    // A Thread may start without a prompt — the composer is where the first one is typed.
    if let Some(prompt) = config.prompt.clone().filter(|p| !p.trim().is_empty()) {
        session.clone().run_turn(prompt, resume).await?;
    }

    Ok(LaunchedSession {
        session: Box::new(CodexHandle {
            inner: session.clone(),
        }),
        events: rx,
    })
}

struct Shared {
    normalizer: CodexNormalizer,
    /// Codex's thread id, learned from the first `thread.started` and used to resume.
    thread_id: Option<String>,
    diagnostics: Vec<RuntimeDiagnostic>,
    running: bool,
    last_exit: Option<i32>,
}

struct CodexSession {
    binary: String,
    cwd: String,
    model: Option<String>,
    extra_args: Vec<String>,
    env: Vec<(String, String)>,
    events: mpsc::UnboundedSender<TervinEvent>,
    state: Mutex<Shared>,
    /// The turn currently running, so it can be killed.
    child: tokio::sync::Mutex<Option<Child>>,
}

impl CodexSession {
    /// Run one turn to completion, streaming its events.
    async fn run_turn(self: Arc<Self>, prompt: String, resume: Option<String>) -> Result<()> {
        let mut command = Command::new(&self.binary);
        for arg in &self.extra_args {
            command.arg(arg);
        }
        command.arg("exec");

        // Resume before the flags, because `resume` is a subcommand of `exec`.
        if let Some(id) = resume.clone() {
            command.arg("resume").arg(id);
        }

        command
            .arg("--json")
            // Tervin opens panes in directories that are not always repositories.
            .arg("--skip-git-repo-check")
            .arg("-C")
            .arg(&self.cwd);

        if let Some(model) = &self.model {
            command.arg("--model").arg(model);
        }
        command.arg(prompt);

        crate::runtime::apply_env(&mut command, &self.env);
        command
            // Closed deliberately: with a prompt argument and a piped stdin, `codex exec`
            // waits on stdin and appends it to the prompt.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|source| RuntimeError::Launch {
            runtime: self.binary.clone(),
            source,
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            RuntimeError::Protocol("codex produced no stdout to read".to_string())
        })?;
        let stderr = child.stderr.take();

        self.state.lock().running = true;
        *self.child.lock().await = Some(child);

        // stdout: the protocol.
        {
            let session = self.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let events = {
                        let mut state = session.state.lock();
                        let events = state.normalizer.line(&line);
                        // Learned once, then used for every following turn.
                        if state.thread_id.is_none() {
                            state.thread_id = state.normalizer.session_id().map(str::to_string);
                        }
                        events
                    };
                    for event in events {
                        if session.events.send(event).is_err() {
                            // Nobody is listening any more; the Thread is gone.
                            return;
                        }
                    }
                }
            });
        }

        // stderr: logs, never the protocol. Kept as diagnostics so a failure has an
        // explanation, and deliberately not parsed — a tracing line is not an event.
        if let Some(stderr) = stderr {
            let session = self.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    let mut state = session.state.lock();
                    if state.diagnostics.len() >= MAX_DIAGNOSTICS {
                        state.diagnostics.remove(0);
                    }
                    state.diagnostics.push(RuntimeDiagnostic {
                        severity: tervin_core::events::Severity::Info,
                        message: line,
                        at: tervin_core::now(),
                    });
                }
            });
        }

        // Reap it, so a Thread whose turn died reports that rather than sitting idle.
        {
            let session = self.clone();
            tokio::spawn(async move {
                let status = {
                    let mut guard = session.child.lock().await;
                    match guard.as_mut() {
                        Some(child) => child.wait().await.ok(),
                        None => None,
                    }
                };
                *session.child.lock().await = None;

                let code = status.and_then(|s| s.code());
                let mut state = session.state.lock();
                state.running = false;
                state.last_exit = code;
                drop(state);

                // A non-zero exit with no `turn.failed` on the wire means the process
                // died before it could say why — reported rather than swallowed.
                if code.unwrap_or(0) != 0 {
                    let event = TervinEvent::new(
                        CodexRuntime::identity_for(None),
                        match code {
                            Some(code) => format!("codex exited {code}"),
                            None => "codex was terminated".to_string(),
                        },
                        EventPayload::ThreadState {
                            state: ThreadState::Failed,
                        },
                    );
                    let _ = session.events.send(event);
                }
            });
        }

        Ok(())
    }
}

/// The handle the app layer holds.
struct CodexHandle {
    inner: Arc<CodexSession>,
}

#[async_trait]
impl AgentSession for CodexHandle {
    async fn send_input(&self, content: String, attachments: Vec<Attachment>) -> Result<()> {
        if !attachments.is_empty() {
            // Said rather than silently dropped: `-i` applies to a first prompt, and a
            // resumed turn has no equivalent.
            return Err(RuntimeError::Unsupported {
                runtime: "codex".to_string(),
                feature: "attachments on a resumed turn (`-i` applies to the first prompt only)"
                    .to_string(),
            });
        }
        if self.inner.state.lock().running {
            return Err(RuntimeError::Unsupported {
                runtime: "codex".to_string(),
                feature: "a second turn while one is running — `codex exec` runs one at a time"
                    .to_string(),
            });
        }

        // Without a thread id there is nothing to resume, which happens when the very
        // first turn failed before Codex announced one.
        let resume = self.inner.state.lock().thread_id.clone();
        let resume = resume.ok_or_else(|| RuntimeError::Unsupported {
            runtime: "codex".to_string(),
            feature: "continuing a session Codex never gave an id for — start a new Thread"
                .to_string(),
        })?;

        self.inner.clone().run_turn(content, Some(resume)).await
    }

    async fn interrupt(&self) -> Result<()> {
        // Killed rather than asked to stop: a one-shot invocation has no protocol for
        // winding down, and pretending to ask would just delay the kill.
        if let Some(child) = self.inner.child.lock().await.as_mut() {
            let _ = child.start_kill();
        }
        Ok(())
    }

    async fn set_permission_mode(&self, _mode: &str) -> Result<()> {
        Err(RuntimeError::Unsupported {
            runtime: "codex".to_string(),
            feature: "changing the permission mode mid-session — the sandbox and approval policy are fixed when the turn starts and cannot be changed"
                .to_string(),
        })
    }

    fn session_metadata(&self) -> SessionMetadata {
        let state = self.inner.state.lock();
        SessionMetadata {
            resume_id: state.thread_id.clone(),
            model: self.inner.model.clone(),
            // Fixed at launch and not changeable, which `set_permission_mode` refuses.
            permission_mode: Some("codex sandbox".to_string()),
            runtime_version: None,
            tools: Vec::new(),
            mcp_servers: Vec::new(),
            slash_commands: Vec::new(),
            hook_runs: Vec::new(),
            // No modes: `codex exec` has no plan mode to switch into, so offering a
            // picker would be offering something that cannot work.
            modes: Vec::new(),
            // Codex reads its own AGENTS.md and config, and does not report which files it
            // loaded. Listing a guess here would put unverified paths in the Bridge panel.
            instruction_sources: Vec::new(),
            cwd: Some(self.inner.cwd.clone()),
        }
    }

    fn permissions(&self) -> PermissionState {
        PermissionState {
            mode: "codex sandbox".to_string(),
            // False, and this is the field the UI keys off to decide whether to show an
            // approval control at all.
            tervin_can_intercept: false,
            // The honest statement, and the same one the capability carries. A user
            // reading this needs to know Tervin is not the thing deciding.
            explanation:
                "Codex's own sandbox decides what runs. `codex exec` has no approval request to answer, so Tervin Rules cannot gate this session — and Tervin does not pass the flags that would disable Codex's sandbox."
                    .to_string(),
            denials: Vec::new(),
        }
    }

    fn diagnostics(&self) -> Vec<RuntimeDiagnostic> {
        let state = self.inner.state.lock();
        let mut out = state.diagnostics.clone();
        // Surfaced rather than left in a log: a protocol change shows up here first.
        let unrecognised = state.normalizer.unrecognised();
        if unrecognised > 0 {
            out.push(RuntimeDiagnostic {
                severity: tervin_core::events::Severity::Warning,
                message: format!(
                    "{unrecognised} line(s) from codex were not recognised. They are on the timeline as unclassified rather than dropped; a jump here usually means Codex changed its output format."
                ),
                at: tervin_core::now(),
            });
        }
        out
    }

    fn capabilities(&self) -> Capabilities {
        codex_capabilities()
    }

    fn is_running(&self) -> bool {
        self.inner.state.lock().running
    }

    async fn shutdown(&self) -> Result<()> {
        if let Some(child) = self.inner.child.lock().await.as_mut() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        *self.inner.child.lock().await = None;
        self.inner.state.lock().running = false;
        Ok(())
    }
}

/// Where a binary is, for the Bridge panel. `None` when it is not on PATH.
async fn which(binary: &str) -> Option<String> {
    let out = Command::new("/usr/bin/env")
        .arg("which")
        .arg(binary)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(path).filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_permission_capability_says_tervin_cannot_gate_codex() {
        // The single most important thing this adapter must not overstate. `codex exec`
        // has no approval request, so a Supported here would be a lie that shows the user
        // a gate that never fires.
        let caps = codex_capabilities();
        match caps.native_permission_bridge {
            CapabilityLevel::Unsupported { ref reason } => {
                assert!(reason.contains("non-interactive"));
                assert!(
                    reason.contains("sandbox"),
                    "the reason should name what does decide"
                );
            }
            ref other => panic!("Tervin must not claim it can gate Codex: {other:?}"),
        }
    }

    #[test]
    fn multi_turn_is_qualified_rather_than_claimed_outright() {
        // It genuinely works, via `codex exec resume`, but each turn is a new process.
        // Saying Supported would imply a live session that stays open.
        match codex_capabilities().multi_turn {
            CapabilityLevel::Partial { ref note } => assert!(note.contains("resume")),
            ref other => panic!("expected a qualified capability, got {other:?}"),
        }
    }

    #[test]
    fn cost_reporting_promises_tokens_and_not_money() {
        match codex_capabilities().cost_reporting {
            CapabilityLevel::Partial { ref note } => {
                assert!(note.contains("token"));
                assert!(note.contains("money") || note.contains("price"));
            }
            ref other => panic!("expected a qualified capability, got {other:?}"),
        }
    }

    #[test]
    fn every_absent_capability_explains_itself() {
        // "Unsupported" with no reason is indistinguishable from "not built yet", and the
        // user cannot tell which without being told.
        let caps = codex_capabilities();
        for (name, level) in [
            ("native_permission_bridge", &caps.native_permission_bridge),
            ("hooks", &caps.hooks),
            ("plan_mode", &caps.plan_mode),
            ("mcp", &caps.mcp),
            ("image_input", &caps.image_input),
            ("multi_turn", &caps.multi_turn),
            ("interrupt", &caps.interrupt),
            ("cost_reporting", &caps.cost_reporting),
        ] {
            match level {
                CapabilityLevel::Unsupported { reason } => {
                    assert!(reason.len() > 20, "{name} has a token reason: {reason:?}")
                }
                CapabilityLevel::Partial { note } => {
                    assert!(note.len() > 20, "{name} has a token note: {note:?}")
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn discovery_reports_a_missing_binary_with_how_to_install_it() {
        // A user whose Codex is not installed should be told what to do, not shown an
        // agent that silently fails on first use.
        let runtime = CodexRuntime::new();
        let found = runtime.discover().await;
        assert_eq!(found.runtime_id, "codex");
        if !found.available {
            assert!(
                found
                    .notes
                    .iter()
                    .any(|n| n.contains("npm i -g @openai/codex")),
                "a missing binary should say how to install it: {:?}",
                found.notes
            );
        } else {
            // When it is installed, the note that matters is the one about gating.
            assert!(
                found.notes.iter().any(|n| n.contains("cannot gate")),
                "an available runtime should still state its limit: {:?}",
                found.notes
            );
        }
    }

    #[tokio::test]
    async fn a_missing_binary_fails_to_launch_with_the_name_in_the_message() {
        let runtime = CodexRuntime::new();
        let mut config = LaunchConfig::new(
            tervin_core::ThreadId::new(),
            std::env::temp_dir().display().to_string(),
        );
        config.binary = Some("codex-that-does-not-exist".to_string());
        config.prompt = Some("hello".to_string());

        match runtime.launch(config).await {
            Err(error @ RuntimeError::Launch { .. }) => {
                let message = error.to_string();
                assert!(
                    message.contains("codex-that-does-not-exist"),
                    "the message should name the binary: {message}"
                );
            }
            Err(other) => panic!("expected a launch error, got {other:?}"),
            Ok(_) => panic!("launching a nonexistent binary should not succeed"),
        }
    }

    #[tokio::test]
    async fn a_thread_with_no_prompt_launches_without_running_anything() {
        // The composer is where the first prompt is often typed, so a Thread can exist
        // before there is anything to run.
        let runtime = CodexRuntime::new();
        let config = LaunchConfig::new(
            tervin_core::ThreadId::new(),
            std::env::temp_dir().display().to_string(),
        );
        let launched = runtime.launch(config).await.expect("should launch");
        assert!(!launched.session.is_running());
        // Nothing has run, so Codex has issued no id to resume from yet.
        assert_eq!(launched.session.session_metadata().resume_id, None);
    }

    #[tokio::test]
    async fn a_resumed_turn_refuses_attachments_and_says_why() {
        let runtime = CodexRuntime::new();
        let config = LaunchConfig::new(
            tervin_core::ThreadId::new(),
            std::env::temp_dir().display().to_string(),
        );
        let launched = runtime.launch(config).await.unwrap();

        let error = launched
            .session
            .send_input(
                "next".to_string(),
                vec![Attachment::File {
                    path: "/tmp/x.png".to_string(),
                }],
            )
            .await
            .expect_err("attachments on a resumed turn should be refused");
        assert!(format!("{error}").contains("attachments"), "{error}");
    }

    #[tokio::test]
    async fn continuing_without_a_thread_id_says_to_start_a_new_thread() {
        // Reached when the first turn died before Codex announced an id. The message has
        // to say what to do, because there is no way to recover this session.
        let runtime = CodexRuntime::new();
        let config = LaunchConfig::new(
            tervin_core::ThreadId::new(),
            std::env::temp_dir().display().to_string(),
        );
        let launched = runtime.launch(config).await.unwrap();

        let error = launched
            .session
            .send_input("next".to_string(), Vec::new())
            .await
            .expect_err("there is no session to continue");
        assert!(format!("{error}").contains("new Thread"), "{error}");
    }

    #[tokio::test]
    async fn changing_the_permission_mode_is_refused_with_a_reason() {
        let runtime = CodexRuntime::new();
        let config = LaunchConfig::new(
            tervin_core::ThreadId::new(),
            std::env::temp_dir().display().to_string(),
        );
        let launched = runtime.launch(config).await.unwrap();

        let error = launched
            .session
            .set_permission_mode("plan")
            .await
            .expect_err("codex cannot change mode mid-session");
        assert!(format!("{error}").contains("cannot be changed"), "{error}");
    }

    #[tokio::test]
    async fn the_permission_state_names_what_actually_decides() {
        let runtime = CodexRuntime::new();
        let config = LaunchConfig::new(
            tervin_core::ThreadId::new(),
            std::env::temp_dir().display().to_string(),
        );
        let launched = runtime.launch(config).await.unwrap();

        let permissions = launched.session.permissions();
        assert!(
            !permissions.tervin_can_intercept,
            "Tervin cannot intercept anything here"
        );
        assert!(permissions.explanation.contains("Codex's own sandbox"));
        // And that Tervin is not quietly disabling it.
        assert!(permissions.explanation.contains("does not pass the flags"));
    }
}
