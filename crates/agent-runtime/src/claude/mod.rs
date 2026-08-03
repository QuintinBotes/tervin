//! The Claude Code adapter — a Tier 1, structured integration.
//!
//! Claude Code is driven over its `stream-json` protocol on stdin and stdout,
//! which gives Tervin a persistent, multi-turn session rather than a series of
//! one-shot invocations. Two channels are multiplexed on that stream:
//!
//! - **Messages** (`system`, `assistant`, `user`, `result`) describe the work, and
//!   are normalised by [`normalize::Normalizer`].
//! - **Control requests** carry out-of-band operations — the `initialize`
//!   handshake, `interrupt`, and `set_permission_mode` — correlated by request id.
//!
//! ## On the permission bridge
//!
//! Tervin has two independent routes to a real gate here, and neither is assumed to
//! work — both are *probed*:
//!
//! 1. **A `PreToolUse` hook** ([`hooks`]), installed with `--settings`. This is the
//!    dependable one: the runtime runs Tervin before each tool call and honours a
//!    refusal. Verified against the real CLI.
//! 2. **`canUseTool`**, declared during `initialize`. Whether the runtime calls back
//!    is a property of the installed version.
//!
//! [`ClaudeSession::permissions`] reports `tervin_can_intercept` as false until one
//! of them has genuinely fired. Configuration is not evidence: `claude --help` warns
//! that settings files failing validation are *silently ignored* in print mode, so a
//! gate that was installed but never consulted is indistinguishable from none, and
//! Tervin says so rather than claiming one.
//!
//! Until a gate is confirmed, Tervin classifies every proposed command, shows the
//! risk, and can interrupt — but it does not claim to have prevented anything.
//! Presenting an observation as a gate would be worse than showing no gate at all.

pub mod hooks;
pub mod normalize;
pub mod transcript;

use crate::runtime::{
    AgentRuntime, AgentSession, ArbiterDecision, Attachment, Discovery, LaunchConfig,
    LaunchedSession, PermissionArbiter, PermissionState, Result, RuntimeDiagnostic, RuntimeError,
    SessionMetadata,
};
use async_trait::async_trait;
use normalize::Normalizer;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tervin_core::{AgentIdentity, Capabilities, CapabilityLevel, TervinEvent, Tier};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, oneshot};

/// Upper bound on a single protocol line.
///
/// A malformed or hostile stream must not be able to grow the read buffer without
/// limit. Real lines are bounded by the runtime's own tool-output limits, which
/// sit far below this.
const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;

/// How long to wait for a control response before giving up on it.
const CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Permission modes Tervin offers.
///
/// `bypassPermissions` is deliberately absent: Tervin will not present a
/// one-click way to disable every check, because doing so cannot be reconciled
/// with telling the user their actions are reviewable.
pub const PERMISSION_MODES: [&str; 4] = ["plan", "auto", "acceptEdits", "manual"];

/// The offered modes, described for the UI.
///
/// Each description says who decides, because that is the only thing a user needs
/// from a mode name and the only thing a wrong guess makes dangerous.
pub fn permission_modes() -> Vec<crate::runtime::SessionMode> {
    use crate::runtime::SessionMode;
    vec![
        SessionMode::new("plan", "Plan").described("Proposes a plan and writes nothing."),
        SessionMode::new("auto", "Auto")
            .described("The runtime decides, using its own permission rules."),
        SessionMode::new("acceptEdits", "Accept edits")
            .described("File edits go through without asking; commands still do."),
        SessionMode::new("manual", "Manual").described("Asks before each action."),
    ]
}

/// The models offered, as aliases rather than pinned identifiers.
///
/// `claude --help` documents these as "an alias for the latest model", so the CLI
/// resolves each to whatever is current. Pinning `claude-opus-4-1-20250805` here
/// would mean shipping a list that silently rots: every new model would need a
/// Tervin release, and worse, a stale entry names a model that still exists and so
/// fails by quietly running the wrong one rather than by erroring.
///
/// The resolved name is reported back by the session and shown alongside, because
/// the alias is not what runs and the difference is what costs money.
pub fn model_choices() -> Vec<crate::runtime::LaunchChoice> {
    use crate::runtime::LaunchChoice;
    vec![
        LaunchChoice::new("", "Profile default")
            .with_note("Whatever the profile or the CLI's own configuration selects."),
        LaunchChoice::new("opus", "Opus").with_note("Most capable, and the most expensive."),
        LaunchChoice::new("sonnet", "Sonnet").with_note("The general-purpose balance."),
        LaunchChoice::new("fable", "Fable"),
        LaunchChoice::new("haiku", "Haiku").with_note("Fastest and cheapest."),
    ]
}

/// The reasoning-effort levels the CLI accepts.
///
/// Unlike the models, this list has to be exact. An unrecognised `--effort` value
/// is a *warning*, not an error: the CLI prints one line, falls back to the default
/// and runs anyway. A typo would therefore produce a session that looks like it is
/// running at the requested effort and is not, which is precisely the class of
/// silent mismatch Tervin exists to make visible. These five are the values the
/// shipped binary names when it rejects one.
pub fn effort_choices() -> Vec<crate::runtime::LaunchChoice> {
    use crate::runtime::LaunchChoice;
    vec![
        LaunchChoice::new("", "Default effort"),
        LaunchChoice::new("low", "Low").with_note("Least thinking, least cost."),
        LaunchChoice::new("medium", "Medium"),
        LaunchChoice::new("high", "High"),
        LaunchChoice::new("xhigh", "Extra high"),
        LaunchChoice::new("max", "Max").with_note("Most thinking, and the slowest."),
    ]
}

/// Shared state between the session handle and its reader task.
struct Shared {
    normalizer: Mutex<Normalizer>,
    metadata: Mutex<SessionMetadata>,
    diagnostics: Mutex<Vec<RuntimeDiagnostic>>,
    /// Set once an inbound `can_use_tool` request is genuinely observed.
    bridge_confirmed: AtomicBool,
    permission_mode: Mutex<String>,
    running: AtomicBool,
    /// Pending control requests, keyed by request id.
    control_waiters: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    /// The `PreToolUse` gate for this session, if one could be installed.
    ///
    /// Owned here so it lives exactly as long as the session: dropping it removes
    /// the socket, and a socket that outlived its Thread would keep answering
    /// permission questions for work that no longer exists.
    gate: Option<hooks::HookGate>,
    events: mpsc::UnboundedSender<TervinEvent>,
    /// Raw payloads drained alongside events, for the store to persist.
    raw_out: mpsc::UnboundedSender<(String, String)>,
}

impl Shared {
    /// Emit events and hand off any raw payloads they reference.
    fn emit(&self, events: Vec<TervinEvent>) {
        let raws: Vec<(String, String)> = {
            let mut n = self.normalizer.lock();
            std::mem::take(&mut n.raw_sink)
        };
        for raw in raws {
            let _ = self.raw_out.send(raw);
        }
        for event in events {
            let _ = self.events.send(event);
        }
    }
}

/// The Claude Code runtime.
pub struct ClaudeCodeRuntime {
    binary: String,
    arbiter: Option<Arc<dyn PermissionArbiter>>,
    /// Tervin's own executable, used as the `PreToolUse` hook command.
    ///
    /// Held rather than resolved at launch so a test can point it elsewhere, and so
    /// a failure to determine it disables the gate loudly instead of silently
    /// registering a hook that cannot run.
    executable: Option<std::path::PathBuf>,
}

impl Default for ClaudeCodeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeCodeRuntime {
    pub fn new() -> Self {
        Self {
            binary: std::env::var("TERVIN_CLAUDE_BINARY").unwrap_or_else(|_| "claude".to_string()),
            arbiter: None,
            executable: std::env::current_exe().ok(),
        }
    }

    /// Wire Tervin Rules in as the permission arbiter, used if and when the
    /// runtime asks.
    pub fn with_arbiter(mut self, arbiter: Arc<dyn PermissionArbiter>) -> Self {
        self.arbiter = Some(arbiter);
        self
    }

    /// Point the hook at a specific executable instead of this process.
    pub fn with_executable(mut self, executable: std::path::PathBuf) -> Self {
        self.executable = Some(executable);
        self
    }

    /// Capabilities as documented for this integration.
    ///
    /// `native_permission_bridge` starts as `Partial` rather than `Supported`
    /// because a gate is only real once it has fired: a settings file that failed
    /// validation is silently ignored, so being installed proves nothing.
    fn static_capabilities() -> Capabilities {
        Capabilities {
            tier: Tier::Structured,
            plan_mode: CapabilityLevel::Supported,
            resume: CapabilityLevel::Supported,
            tool_events: CapabilityLevel::Supported,
            file_edits: CapabilityLevel::Supported,
            native_permission_bridge: CapabilityLevel::partial(
                "Tervin installs a PreToolUse hook, so Tervin Rules can refuse an action \
                 before it runs — and only refuse: Tervin never approves on the runtime's \
                 behalf. The gate is confirmed per session, and it cannot block if Tervin \
                 becomes unreachable.",
            ),
            mcp: CapabilityLevel::Supported,
            hooks: CapabilityLevel::Supported,
            subagents: CapabilityLevel::Supported,
            image_input: CapabilityLevel::partial(
                "Images can be attached to a prompt; the runtime decides what to do with them.",
            ),
            cost_reporting: CapabilityLevel::Supported,
            model_selection: CapabilityLevel::Supported,
            remote_execution: CapabilityLevel::unsupported(
                "Runs as a local process. Remote work happens through the pane's own session.",
            ),
            multi_turn: CapabilityLevel::Supported,
            interrupt: CapabilityLevel::Supported,
        }
    }

    /// The executable for one launch: the profile's, or this adapter's default.
    fn binary_for(&self, config: &LaunchConfig) -> String {
        config
            .binary
            .clone()
            .filter(|b| !b.trim().is_empty())
            .unwrap_or_else(|| self.binary.clone())
    }

    /// Start the `PreToolUse` gate, if one can be started.
    ///
    /// Returns `None` when there is nothing to gate with — no arbiter wired, or no
    /// resolvable executable to run as the hook. Returning `None` rather than
    /// registering a hook that cannot answer matters: a hook that fails is a
    /// non-blocking error, so a broken gate is indistinguishable from no gate at
    /// all, and Tervin would be claiming one it does not have.
    async fn start_gate(
        &self,
        config: &LaunchConfig,
        events: mpsc::UnboundedSender<TervinEvent>,
    ) -> (Option<hooks::HookGate>, Vec<String>) {
        let mut notes = Vec::new();

        let Some(arbiter) = self.arbiter.clone() else {
            return (None, notes);
        };
        let Some(executable) = self.executable.clone() else {
            notes.push(
                "Tervin could not determine its own path, so Tervin Rules cannot gate this \
                 session's actions. They are still shown and classified."
                    .to_string(),
            );
            return (None, notes);
        };

        // Every decision becomes a timeline event. A refusal that only shows in a
        // status line is not an audit trail — the point of the gate is that what was
        // stopped stays inspectable afterwards.
        let identity = AgentIdentity::new("claude-code", "Claude Code", Tier::Structured);
        let thread_id = config.thread_id.clone();
        let cwd = config.cwd.clone();
        let project = std::path::Path::new(&config.cwd)
            .file_name()
            .and_then(|s| s.to_str())
            .map(String::from);

        let handler = Arc::new(
            hooks::ArbiterHandler::new(arbiter, config.thread_id.clone()).observed_by(Box::new(
                move |request, decision| {
                    for event in
                        gate_events(&identity, &thread_id, &project, &cwd, request, decision)
                    {
                        let _ = events.send(event);
                    }
                },
            )),
        );
        let dir = tervin_core::paths::runtime_dir();

        match hooks::start_gate(&dir, &config.thread_id, &executable, handler).await {
            Ok(gate) => (Some(gate), notes),
            Err(e) => {
                // Say so. The user would otherwise believe actions are being gated.
                notes.push(format!(
                    "Tervin Rules could not be installed as a gate for this session ({e}). \
                     Actions are shown and classified, but not blocked."
                ));
                (None, notes)
            }
        }
    }

    fn build_args(
        &self,
        config: &LaunchConfig,
        resume: Option<&str>,
        hook_settings: Option<&std::path::Path>,
    ) -> Vec<String> {
        // A profile's own arguments go first, so it can select a build or pass a
        // flag Tervin does not model, without displacing the protocol flags below.
        let mut args: Vec<String> = config.extra_args.clone();
        args.extend([
            "-p".into(),
            "--input-format".into(),
            "stream-json".into(),
            "--output-format".into(),
            "stream-json".into(),
            // Required for the full event stream rather than only a final result.
            "--verbose".into(),
            // Puts the user's own hooks into the stream Tervin already reads, so a
            // silently failing hook becomes visible instead of quietly degrading
            // every session.
            "--include-hook-events".into(),
        ]);

        if let Some(id) = resume {
            args.push("--resume".into());
            args.push(id.to_string());
        }
        if let Some(model) = &config.model {
            args.push("--model".into());
            args.push(model.clone());
        }
        if let Some(effort) = &config.effort {
            args.push("--effort".into());
            args.push(effort.clone());
        }
        let mode = config
            .permission_mode
            .clone()
            .unwrap_or_else(|| "auto".into());
        args.push("--permission-mode".into());
        args.push(mode);

        // Policy is pushed down to the runtime so it applies before anything runs,
        // rather than being evaluated after the fact.
        if !config.allowed_tools.is_empty() {
            args.push("--allowed-tools".into());
            args.push(config.allowed_tools.join(" "));
        }
        if !config.disallowed_tools.is_empty() {
            args.push("--disallowed-tools".into());
            args.push(config.disallowed_tools.join(" "));
        }

        // `--settings` loads *additional* settings, so this adds Tervin's gate
        // without reading or overriding anything the user configured. It goes last
        // so a profile argument cannot displace it.
        if let Some(path) = hook_settings {
            args.push("--settings".into());
            args.push(path.display().to_string());
        }

        args
    }

    async fn start(&self, config: LaunchConfig, resume: Option<&str>) -> Result<LaunchedSession> {
        // The event channel comes first, because the gate writes into it: a refusal
        // has to appear in the Thread's timeline, not only in a status line.
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (raw_tx, raw_rx) = mpsc::unbounded_channel();

        // Then the gate, because its settings path has to be in the arguments.
        let (gate, gate_notes) = self.start_gate(&config, events_tx.clone()).await;
        let args = self.build_args(&config, resume, gate.as_ref().map(|g| g.settings_path()));
        let binary = self.binary_for(&config);

        let mut command = tokio::process::Command::new(&binary);
        // Removals must remove. An empty `CLAUDE_CONFIG_DIR` is an empty path, not
        // an absent one, and it would silently select the wrong account.
        crate::runtime::apply_env(&mut command, &config.env);

        let mut child = command
            .args(&args)
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Without this, a killed Tervin can leave the agent running.
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    RuntimeError::NotInstalled(binary.clone())
                } else {
                    RuntimeError::Launch {
                        runtime: "claude-code".to_string(),
                        source: e,
                    }
                }
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RuntimeError::Protocol("child stdin was not piped".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::Protocol("child stdout was not piped".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RuntimeError::Protocol("child stderr was not piped".to_string()))?;

        let identity = AgentIdentity::new("claude-code", "Claude Code", Tier::Structured);
        let mut normalizer =
            Normalizer::new(config.thread_id.clone(), identity, config.cwd.clone());
        // So a sign-in message can name which account failed. Taken from the launch
        // environment, which is what actually decides, rather than from the profile
        // name, which is only a label.
        normalizer.set_account_hint(account_hint(&config));

        let shared = Arc::new(Shared {
            normalizer: Mutex::new(normalizer),
            metadata: Mutex::new(SessionMetadata::default()),
            diagnostics: Mutex::new(Vec::new()),
            bridge_confirmed: AtomicBool::new(false),
            permission_mode: Mutex::new(
                config
                    .permission_mode
                    .clone()
                    .unwrap_or_else(|| "auto".into()),
            ),
            running: AtomicBool::new(true),
            control_waiters: Mutex::new(HashMap::new()),
            gate,
            events: events_tx,
            raw_out: raw_tx,
        });

        for note in gate_notes {
            shared.diagnostics.lock().push(RuntimeDiagnostic {
                severity: tervin_core::events::Severity::Warning,
                message: note,
                at: tervin_core::now(),
            });
        }

        let stdin = Arc::new(tokio::sync::Mutex::new(stdin));

        // Reader task: the protocol's only consumer.
        {
            let shared = shared.clone();
            let stdin = stdin.clone();
            let arbiter = self.arbiter.clone();
            let cwd = config.cwd.clone();
            let thread_id = config.thread_id.clone();
            tokio::spawn(async move {
                read_stream(stdout, shared.clone(), stdin, arbiter, cwd, thread_id).await;
                shared.running.store(false, Ordering::SeqCst);
            });
        }

        // stderr is diagnostics about the runtime, not about the user's code.
        {
            let shared = shared.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() {
                        continue;
                    }
                    shared.diagnostics.lock().push(RuntimeDiagnostic {
                        severity: tervin_core::events::Severity::Warning,
                        message: line,
                        at: tervin_core::now(),
                    });
                }
            });
        }

        // Reap the child so a crash becomes a Disconnected Thread rather than a
        // Thread that appears to still be working.
        {
            let shared = shared.clone();
            tokio::spawn(async move {
                let status = child.wait().await;
                shared.running.store(false, Ordering::SeqCst);
                let detail = match status {
                    Ok(s) if s.success() => "agent process exited".to_string(),
                    Ok(s) => format!("agent process exited with status {s}"),
                    Err(e) => format!("could not reap agent process: {e}"),
                };
                let events = shared.normalizer.lock().disconnected(&detail);
                shared.emit(events);
            });
        }

        let session = ClaudeSession {
            shared: shared.clone(),
            stdin,
            raw_rx: Mutex::new(Some(raw_rx)),
        };

        // Declare the permission-arbitration capability. Success only means the
        // handshake was accepted, never that callbacks will arrive.
        let _ = session
            .control_request(
                "initialize",
                json!({ "hooks": {}, "capabilities": { "canUseTool": true } }),
            )
            .await;

        if let Some(prompt) = config.prompt.clone() {
            session
                .send_input(prompt, config.attachments.clone())
                .await?;
        }

        Ok(LaunchedSession {
            session: Box::new(session),
            events: events_rx,
        })
    }
}

#[async_trait]
impl AgentRuntime for ClaudeCodeRuntime {
    fn runtime_id(&self) -> &str {
        "claude-code"
    }

    fn identity(&self) -> AgentIdentity {
        AgentIdentity::new("claude-code", "Claude Code", Tier::Structured)
    }

    async fn discover(&self) -> Discovery {
        let mut notes = Vec::new();
        let output = tokio::process::Command::new(&self.binary)
            .arg("--version")
            .output()
            .await;

        let (available, version) = match output {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                // "2.1.220 (Claude Code)"
                let version = text.split_whitespace().next().map(String::from);
                (true, version)
            }
            Ok(_) => {
                notes.push("`claude --version` failed; the install may be broken.".to_string());
                (false, None)
            }
            Err(_) => (false, None),
        };

        if !available {
            notes.push("Install Claude Code to use it here. Tervin runs without it.".to_string());
        }

        let path = which(&self.binary);

        Discovery {
            runtime_id: "claude-code".to_string(),
            display_name: "Claude Code".to_string(),
            available,
            version,
            path,
            notes,
            capabilities: Self::static_capabilities(),
        }
    }

    fn capabilities(&self) -> Capabilities {
        Self::static_capabilities()
    }

    fn launch_options(&self) -> crate::runtime::LaunchOptions {
        crate::runtime::LaunchOptions {
            models: model_choices(),
            efforts: effort_choices(),
        }
    }

    async fn launch(&self, config: LaunchConfig) -> Result<LaunchedSession> {
        self.start(config, None).await
    }

    async fn resume(&self, resume_id: &str, config: LaunchConfig) -> Result<LaunchedSession> {
        self.start(config, Some(resume_id)).await
    }
}

/// A live Claude Code session.
pub struct ClaudeSession {
    shared: Arc<Shared>,
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    /// Raw payload stream, taken once by the host.
    raw_rx: Mutex<Option<mpsc::UnboundedReceiver<(String, String)>>>,
}

impl ClaudeSession {
    /// Take the raw-payload stream, so the store can persist what the runtime
    /// actually said behind each event.
    pub fn take_raw_stream(&self) -> Option<mpsc::UnboundedReceiver<(String, String)>> {
        self.raw_rx.lock().take()
    }

    async fn write_line(&self, value: &Value) -> Result<()> {
        if !self.shared.running.load(Ordering::SeqCst) {
            return Err(RuntimeError::SessionEnded);
        }
        let mut line =
            serde_json::to_string(value).map_err(|e| RuntimeError::Protocol(e.to_string()))?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Send a control request and await its response.
    async fn control_request(&self, subtype: &str, mut request: Value) -> Result<Value> {
        let request_id = format!("tervin-{}", uuid::Uuid::new_v4());
        if let Some(obj) = request.as_object_mut() {
            obj.insert("subtype".to_string(), json!(subtype));
        }

        let (tx, rx) = oneshot::channel();
        self.shared
            .control_waiters
            .lock()
            .insert(request_id.clone(), tx);

        self.write_line(&json!({
            "type": "control_request",
            "request_id": request_id,
            "request": request,
        }))
        .await?;

        match tokio::time::timeout(CONTROL_TIMEOUT, rx).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(RuntimeError::Protocol(
                "control response channel closed".to_string(),
            )),
            Err(_) => {
                // Do not leak the waiter if the runtime never answers.
                self.shared.control_waiters.lock().remove(&request_id);
                Err(RuntimeError::Protocol(format!(
                    "{subtype} timed out after {}s",
                    CONTROL_TIMEOUT.as_secs()
                )))
            }
        }
    }
}

#[async_trait]
impl AgentSession for ClaudeSession {
    async fn send_input(&self, content: String, attachments: Vec<Attachment>) -> Result<()> {
        // Attachments become explicit prompt text so that what was shared is
        // visible in the transcript, not implied.
        let mut blocks: Vec<Value> = Vec::new();
        for attachment in &attachments {
            match attachment {
                Attachment::Image { media_type, data } => blocks.push(json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": media_type, "data": data }
                })),
                other => {
                    if let Some(text) = other.to_prompt_text() {
                        blocks.push(json!({ "type": "text", "text": text }));
                    }
                }
            }
        }
        blocks.push(json!({ "type": "text", "text": content.clone() }));

        let events = self
            .shared
            .normalizer
            .lock()
            .user_prompt(&content, &attachments);
        self.shared.emit(events);

        self.write_line(&json!({
            "type": "user",
            "message": { "role": "user", "content": blocks }
        }))
        .await
    }

    async fn interrupt(&self) -> Result<()> {
        let result = self.control_request("interrupt", json!({})).await;
        let events = self.shared.normalizer.lock().interrupted();
        self.shared.emit(events);
        result.map(|_| ())
    }

    async fn set_permission_mode(&self, mode: &str) -> Result<()> {
        if !PERMISSION_MODES.contains(&mode) {
            return Err(RuntimeError::Unsupported {
                runtime: "claude-code".to_string(),
                feature: format!("permission mode `{mode}`"),
            });
        }
        self.control_request("set_permission_mode", json!({ "mode": mode }))
            .await?;
        *self.shared.permission_mode.lock() = mode.to_string();
        Ok(())
    }

    fn session_metadata(&self) -> SessionMetadata {
        let n = self.shared.normalizer.lock();
        let mut meta = self.shared.metadata.lock().clone();
        meta.resume_id = n.resume_id.clone();
        meta.model = n.model.clone();
        meta.runtime_version = n.runtime_version.clone();
        meta.tools = n.tools.clone();
        meta.mcp_servers = n.mcp_servers.clone();
        meta.slash_commands = n.slash_commands.clone();
        meta.hook_runs = n.hook_runs.clone();
        meta.permission_mode = Some(self.shared.permission_mode.lock().clone());
        meta.modes = permission_modes();
        meta
    }

    fn permissions(&self) -> PermissionState {
        let asked = self.shared.bridge_confirmed.load(Ordering::SeqCst);
        let gate = self.shared.gate.as_ref();
        let gate_live = gate.is_some_and(|g| g.confirmed());

        // Two independent routes to a real gate. Whichever is live decides what the
        // UI is allowed to claim, and the wording distinguishes them because they
        // fail differently.
        let (intercept, explanation) = if asked {
            (
                true,
                "This runtime asks Tervin before acting, so Tervin Rules decide.".to_string(),
            )
        } else if gate_live {
            (
                true,
                "Tervin Rules gate this session through a PreToolUse hook: a refusal \
                 stops the action before it runs. Tervin only ever adds a refusal — it \
                 never approves on the runtime\'s behalf, so Claude Code\'s own checks \
                 still apply. If Tervin becomes unreachable the hook cannot block, and \
                 actions would proceed unchecked."
                    .to_string(),
            )
        } else if gate.is_some() {
            (
                // Configured is not confirmed. Nothing has come through the hook yet,
                // so there is no evidence it works, and claiming a gate on the
                // strength of configuration is exactly the dishonesty to avoid.
                false,
                "Tervin Rules are installed as a PreToolUse gate for this session but \
                 have not been consulted yet. Until the first tool call arrives, treat \
                 approvals as the runtime\'s own."
                    .to_string(),
            )
        } else {
            (
                false,
                "Approvals are handled by Claude Code itself. Tervin shows what is \
                 proposed and can stop the session, but does not gate individual actions."
                    .to_string(),
            )
        };

        let mut denials = self.shared.normalizer.lock().denials.clone();
        if let Some(gate) = gate {
            denials.extend(gate.denials());
        }

        PermissionState {
            mode: self.shared.permission_mode.lock().clone(),
            tervin_can_intercept: intercept,
            explanation,
            denials,
        }
    }

    fn diagnostics(&self) -> Vec<RuntimeDiagnostic> {
        self.shared.diagnostics.lock().clone()
    }

    fn capabilities(&self) -> Capabilities {
        let mut caps = ClaudeCodeRuntime::static_capabilities();
        // Upgraded only by evidence: an inbound `can_use_tool`, or a hook that has
        // actually called in. Configuration alone is not proof.
        let gate_live = self.shared.gate.as_ref().is_some_and(|g| g.confirmed());
        if self.shared.bridge_confirmed.load(Ordering::SeqCst) || gate_live {
            caps.native_permission_bridge = CapabilityLevel::Supported;
        } else if self.shared.gate.is_some() {
            caps.native_permission_bridge = CapabilityLevel::partial(
                "Tervin Rules are installed as a PreToolUse gate but have not been \
                 consulted yet, so the gate is unproven for this session.",
            );
        }
        caps
    }

    fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    async fn shutdown(&self) -> Result<()> {
        self.shared.running.store(false, Ordering::SeqCst);
        // Closing stdin asks the runtime to finish; `kill_on_drop` is the backstop.
        let mut stdin = self.stdin.lock().await;
        let _ = stdin.shutdown().await;
        Ok(())
    }
}

/// Read and dispatch the protocol stream until it ends.
async fn read_stream(
    stdout: tokio::process::ChildStdout,
    shared: Arc<Shared>,
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    arbiter: Option<Arc<dyn PermissionArbiter>>,
    cwd: String,
    thread_id: tervin_core::ThreadId,
) {
    let mut reader = BufReader::new(stdout);
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);

    loop {
        buf.clear();
        // `read_until` rather than `lines()` so an absurd line can be rejected
        // instead of being buffered without limit.
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        if buf.len() > MAX_LINE_BYTES {
            shared.diagnostics.lock().push(RuntimeDiagnostic {
                severity: tervin_core::events::Severity::Error,
                message: format!(
                    "Discarded a {} MB protocol line, which exceeds the {} MB limit.",
                    buf.len() / (1024 * 1024),
                    MAX_LINE_BYTES / (1024 * 1024)
                ),
                at: tervin_core::now(),
            });
            continue;
        }

        let text = String::from_utf8_lossy(&buf);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            // Not JSON. Some builds print banners or warnings on stdout; record
            // it rather than treating the stream as broken.
            shared.diagnostics.lock().push(RuntimeDiagnostic {
                severity: tervin_core::events::Severity::Info,
                message: trimmed.chars().take(500).collect(),
                at: tervin_core::now(),
            });
            continue;
        };

        match value.get("type").and_then(Value::as_str) {
            Some("control_response") => {
                let response = value.get("response").cloned().unwrap_or(Value::Null);
                if let Some(id) = response.get("request_id").and_then(Value::as_str) {
                    if let Some(tx) = shared.control_waiters.lock().remove(id) {
                        let _ = tx.send(response);
                    }
                }
            }

            Some("control_request") => {
                handle_inbound_control(&value, &shared, &stdin, arbiter.as_ref(), &cwd, &thread_id)
                    .await;
            }

            Some("control_cancel_request") => {
                if let Some(id) = value.get("request_id").and_then(Value::as_str) {
                    shared.control_waiters.lock().remove(id);
                }
            }

            _ => {
                let events = shared.normalizer.lock().ingest(&value);
                shared.emit(events);
            }
        }
    }
}

/// Handle a control request the runtime sent us.
///
/// The only one Tervin can answer meaningfully is `can_use_tool`. Receiving it is
/// also the moment the permission bridge stops being theoretical, so it flips
/// `bridge_confirmed` and the capability panel updates.
async fn handle_inbound_control(
    value: &Value,
    shared: &Arc<Shared>,
    stdin: &Arc<tokio::sync::Mutex<ChildStdin>>,
    arbiter: Option<&Arc<dyn PermissionArbiter>>,
    cwd: &str,
    thread_id: &tervin_core::ThreadId,
) {
    let request_id = value
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let request = value.get("request").cloned().unwrap_or(Value::Null);
    let subtype = request.get("subtype").and_then(Value::as_str).unwrap_or("");

    let response = if subtype == "can_use_tool" {
        shared.bridge_confirmed.store(true, Ordering::SeqCst);

        let tool_name = request
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let input = request.get("input").cloned().unwrap_or(Value::Null);

        let decision = match arbiter {
            Some(arbiter) => arbiter.decide(thread_id, &tool_name, &input, cwd).await,
            // No arbiter wired: deny rather than allow. An unanswered gate must
            // fail closed.
            None => ArbiterDecision::Deny {
                reason: "No Tervin Rules arbiter is configured for this session.".to_string(),
            },
        };

        let (behavior, message, allowed) = match &decision {
            ArbiterDecision::Allow => ("allow", String::new(), true),
            ArbiterDecision::Deny { reason } => ("deny", reason.clone(), false),
        };

        let events = shared.normalizer.lock().tervin_decision(
            &tool_name,
            allowed,
            if allowed {
                "Allowed by Tervin Rules"
            } else {
                &message
            },
        );
        shared.emit(events);

        if allowed {
            json!({ "subtype": "success", "request_id": request_id, "response": { "behavior": "allow", "updatedInput": input } })
        } else {
            json!({ "subtype": "success", "request_id": request_id, "response": { "behavior": behavior, "message": message } })
        }
    } else {
        // Unknown control request: say so rather than pretending to have handled it.
        json!({
            "subtype": "error",
            "request_id": request_id,
            "error": format!("Tervin does not implement control request `{subtype}`.")
        })
    };

    let line = json!({ "type": "control_response", "response": response });
    if let Ok(mut text) = serde_json::to_string(&line) {
        text.push('\n');
        let mut guard = stdin.lock().await;
        let _ = guard.write_all(text.as_bytes()).await;
        let _ = guard.flush().await;
    }
}

/// Turn one gate decision into timeline events.
///
/// A refusal produces two rows, not one: what was asked, and what Tervin answered.
/// A denial with no visible request would read as Tervin blocking something out of
/// nowhere, and the pair is what makes it reviewable.
///
/// A `defer` produces nothing. Tervin had no opinion, and a row per allowed action
/// would bury the ones that matter.
fn gate_events(
    identity: &AgentIdentity,
    thread_id: &tervin_core::ThreadId,
    project: &Option<String>,
    cwd: &str,
    request: &hooks::HookRequest,
    decision: &hooks::HookDecision,
) -> Vec<TervinEvent> {
    let hooks::HookDecision::Deny { reason } = decision else {
        return Vec::new();
    };

    let action = hooks::describe_action(request);
    let event = |summary: String, payload: tervin_core::EventPayload| {
        TervinEvent::new(identity.clone(), summary, payload)
            .with_thread(thread_id.clone())
            .with_location(project.clone(), Some(cwd.to_string()))
    };

    let mut risk = match request.tool_input.get("command").and_then(Value::as_str) {
        Some(command) => rules_engine::classify(command, cwd),
        None => tervin_core::RiskAssessment::benign(),
    };
    // The hook is a real gate: the action had not run when this was decided.
    risk.enforceable = true;

    vec![
        event(
            format!("Permission requested: {action}"),
            tervin_core::EventPayload::PermissionRequested {
                request_id: tervin_core::RequestId::new(),
                action: action.clone(),
                risk,
                interceptable: true,
            },
        ),
        event(
            format!("{action} blocked"),
            tervin_core::EventPayload::PermissionDenied {
                request_id: None,
                action,
                authority: tervin_core::events::DecisionAuthority::Tervin,
                reason: Some(reason.clone()),
            },
        ),
    ]
}

/// Which account a launch will use, described without reading any secret.
///
/// Only the *presence* of a credential variable is reported, and only the config
/// directory's path — never a token's value. An error message is exactly the wrong
/// place for a secret to appear.
fn account_hint(config: &LaunchConfig) -> Option<String> {
    let get = |key: &str| {
        config
            .env
            .iter()
            .find(|(k, v)| k == key && !v.is_empty())
            .map(|(_, v)| v.clone())
    };

    if let Some(dir) = get("CLAUDE_CONFIG_DIR") {
        return Some(format!("the account in {dir}"));
    }
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        if get(key).is_some() {
            return Some(format!("the credential this profile sets in {key}"));
        }
    }

    // The trap this message exists for: a profile that sets no config directory
    // runs the *default* account, which is often not the one the user signs into
    // from their shell. Naming the directory turns "re-authenticate" into
    // something they can act on, and hints at the real fix — pick a profile.
    let default_dir = dirs::home_dir()
        .map(|home| tervin_core::paths::abbreviate(&home.join(".claude")))
        .unwrap_or_else(|| "~/.claude".to_string());
    Some(format!(
        "the default account in {default_dir} — this profile sets no CLAUDE_CONFIG_DIR, \
         so it does not use whichever account your shell aliases select"
    ))
}

/// Resolve a binary on `PATH`, for the Bridge panel's "where is this from".
fn which(binary: &str) -> Option<String> {
    if binary.contains('/') {
        return std::path::Path::new(binary)
            .exists()
            .then(|| binary.to_string());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
        .map(|p| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::LaunchConfig;
    use tervin_core::ThreadId;

    fn config() -> LaunchConfig {
        LaunchConfig::new(ThreadId::new(), "/tmp")
    }

    #[test]
    fn launch_args_request_a_persistent_structured_session() {
        let rt = ClaudeCodeRuntime::new();
        let args = rt.build_args(&config(), None, None);
        let joined = args.join(" ");
        // Both directions must be stream-json, or the session is one-shot.
        assert!(joined.contains("--input-format stream-json"));
        assert!(joined.contains("--output-format stream-json"));
        // Without --verbose the stream collapses to a single result message.
        assert!(args.iter().any(|a| a == "--verbose"));
    }

    #[test]
    fn resume_passes_the_runtime_session_id() {
        let rt = ClaudeCodeRuntime::new();
        let args = rt.build_args(&config(), Some("abc-123"), None);
        let i = args
            .iter()
            .position(|a| a == "--resume")
            .expect("no --resume");
        assert_eq!(args[i + 1], "abc-123");
    }

    #[test]
    fn model_and_effort_are_passed_only_when_chosen() {
        let rt = ClaudeCodeRuntime::new();

        // Nothing chosen: neither flag appears, so the CLI's own configuration and
        // the user's defaults decide. Passing an empty value would override them.
        let bare = rt.build_args(&config(), None, None);
        assert!(!bare.iter().any(|a| a == "--model"));
        assert!(!bare.iter().any(|a| a == "--effort"));

        let mut cfg = config();
        cfg.model = Some("opus".into());
        cfg.effort = Some("high".into());
        let args = rt.build_args(&cfg, None, None);
        let m = args
            .iter()
            .position(|a| a == "--model")
            .expect("no --model");
        assert_eq!(args[m + 1], "opus");
        let e = args
            .iter()
            .position(|a| a == "--effort")
            .expect("no --effort");
        assert_eq!(args[e + 1], "high");
    }

    #[test]
    fn the_offered_efforts_are_exactly_the_ones_the_cli_accepts() {
        // This list has to be exact in a way the model list does not. An
        // unrecognised `--effort` value is a warning, not an error: the CLI falls
        // back to the default and runs anyway, so a wrong entry here produces a
        // session that reports one effort and spends another. These five are what
        // the binary names when it rejects a value.
        let offered: Vec<String> = effort_choices()
            .into_iter()
            .map(|c| c.value)
            .filter(|v| !v.is_empty())
            .collect();
        assert_eq!(offered, ["low", "medium", "high", "xhigh", "max"]);
    }

    #[test]
    fn the_offered_models_are_aliases_rather_than_pinned_identifiers() {
        // A pinned id fails by quietly running last year's model, since the old name
        // still resolves. An alias is documented to track whatever is current, so
        // the list cannot rot into silently wrong.
        for choice in model_choices() {
            assert!(
                !choice.value.starts_with("claude-"),
                "{} is a pinned identifier, not an alias",
                choice.value
            );
        }
        let values: Vec<String> = model_choices().into_iter().map(|c| c.value).collect();
        assert!(values.contains(&String::new()), "no way to express 'unset'");
    }

    #[test]
    fn policy_is_pushed_down_to_the_runtime() {
        // Rules must apply before an action runs, not only after Tervin sees it.
        let rt = ClaudeCodeRuntime::new();
        let mut cfg = config();
        cfg.allowed_tools = vec!["Bash(git *)".into(), "Read".into()];
        cfg.disallowed_tools = vec!["WebFetch".into()];
        let args = rt.build_args(&cfg, None, None);

        let i = args.iter().position(|a| a == "--allowed-tools").unwrap();
        assert_eq!(args[i + 1], "Bash(git *) Read");
        let j = args.iter().position(|a| a == "--disallowed-tools").unwrap();
        assert_eq!(args[j + 1], "WebFetch");
    }

    #[test]
    fn a_profile_chooses_the_executable_and_its_own_flags_come_first() {
        // Without this a profile could only change environment, and "run this
        // build of the agent" would be unexpressible.
        let rt = ClaudeCodeRuntime::new();
        let mut cfg = config();
        cfg.binary = Some("/opt/builds/claude-next".into());
        cfg.extra_args = vec!["--settings".into(), "/tmp/work.json".into()];

        assert_eq!(rt.binary_for(&cfg), "/opt/builds/claude-next");

        let args = rt.build_args(&cfg, None, None);
        assert_eq!(args[0], "--settings");
        assert_eq!(args[1], "/tmp/work.json");
        // The protocol flags must still be there, after the profile's.
        assert!(args.iter().any(|a| a == "--verbose"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--output-format" && w[1] == "stream-json"));
    }

    #[test]
    fn an_empty_profile_binary_falls_back_to_the_default() {
        let rt = ClaudeCodeRuntime::new();
        let mut cfg = config();
        cfg.binary = Some("   ".into());
        assert_eq!(rt.binary_for(&cfg), rt.binary);
    }

    #[test]
    fn the_hook_settings_are_passed_with_the_merging_flag() {
        // `--settings` loads additional settings. Anything that replaced the user's
        // configuration instead would be a serious regression, so the flag matters
        // as much as the file.
        let rt = ClaudeCodeRuntime::new();
        let settings = std::path::Path::new("/run/tervin/hooks-thr_x.json");
        let args = rt.build_args(&config(), None, Some(settings));

        let i = args
            .iter()
            .position(|a| a == "--settings")
            .expect("no --settings");
        assert_eq!(args[i + 1], "/run/tervin/hooks-thr_x.json");
    }

    #[test]
    fn a_profile_argument_cannot_displace_the_gate() {
        // The settings flag goes last on purpose: a profile that also passes
        // --settings must not end up shadowing Tervin's gate.
        let rt = ClaudeCodeRuntime::new();
        let mut cfg = config();
        cfg.extra_args = vec!["--settings".into(), "/tmp/mine.json".into()];
        let args = rt.build_args(&cfg, None, Some(std::path::Path::new("/run/gate.json")));

        let last = args
            .iter()
            .rposition(|a| a == "--settings")
            .expect("no --settings");
        assert_eq!(
            args[last + 1],
            "/run/gate.json",
            "Tervin's gate must be the last settings source"
        );
    }

    #[test]
    fn no_settings_flag_appears_when_there_is_no_gate() {
        let rt = ClaudeCodeRuntime::new();
        let args = rt.build_args(&config(), None, None);
        assert!(!args.iter().any(|a| a == "--settings"));
    }

    #[tokio::test]
    async fn no_arbiter_means_no_gate_rather_than_a_broken_one() {
        // A hook that cannot answer is a non-blocking error, so a broken gate looks
        // exactly like no gate. Registering one anyway would have Tervin claim a
        // gate it does not have.
        let rt = ClaudeCodeRuntime::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let (gate, notes) = rt.start_gate(&config(), tx).await;
        assert!(gate.is_none());
        assert!(
            notes.is_empty(),
            "nothing to warn about: none was asked for"
        );
    }

    #[tokio::test]
    async fn an_unresolvable_executable_disables_the_gate_loudly() {
        struct Yes;
        #[async_trait]
        impl PermissionArbiter for Yes {
            async fn decide(
                &self,
                _: &tervin_core::ThreadId,
                _: &str,
                _: &Value,
                _: &str,
            ) -> ArbiterDecision {
                ArbiterDecision::Allow
            }
        }

        let rt = ClaudeCodeRuntime {
            binary: "claude".to_string(),
            arbiter: Some(Arc::new(Yes)),
            executable: None,
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let (gate, notes) = rt.start_gate(&config(), tx).await;
        assert!(gate.is_none());
        assert!(
            notes.iter().any(|n| n.contains("cannot gate")),
            "the user must be told the gate is absent: {notes:?}"
        );
    }

    #[test]
    fn a_blocked_action_becomes_a_reviewable_pair_in_the_timeline() {
        // What was asked *and* what Tervin answered. A denial with no visible request
        // would read as Tervin blocking something out of nowhere.
        let identity = AgentIdentity::new("claude-code", "Claude Code", Tier::Structured);
        let thread_id = tervin_core::ThreadId::new();
        let request = hooks::HookRequest {
            tool_name: "Bash".into(),
            tool_input: json!({ "command": "rm -rf /" }),
            ..Default::default()
        };
        let decision = hooks::HookDecision::Deny {
            reason: "Denied by Tervin Rules: irreversible".into(),
        };

        let events = gate_events(
            &identity,
            &thread_id,
            &Some("proj".into()),
            "/Users/dev/proj",
            &request,
            &decision,
        );
        let kinds: Vec<&str> = events.iter().map(|e| e.kind()).collect();
        assert_eq!(kinds, vec!["permission.requested", "permission.denied"]);

        match &events[0].payload {
            tervin_core::EventPayload::PermissionRequested {
                risk,
                interceptable,
                ..
            } => {
                assert_eq!(risk.level, tervin_core::RiskLevel::Critical);
                // The action had not run when this was decided.
                assert!(risk.enforceable);
                assert!(*interceptable);
            }
            other => panic!("got {other:?}"),
        }
        match &events[1].payload {
            tervin_core::EventPayload::PermissionDenied {
                action,
                authority,
                reason,
                ..
            } => {
                assert_eq!(action, "rm -rf /");
                assert_eq!(*authority, tervin_core::events::DecisionAuthority::Tervin);
                assert!(reason
                    .as_deref()
                    .is_some_and(|r| r.contains("irreversible")));
            }
            other => panic!("got {other:?}"),
        }

        for event in &events {
            assert_eq!(event.thread_id.as_ref(), Some(&thread_id));
            assert_eq!(event.cwd.as_deref(), Some("/Users/dev/proj"));
        }
    }

    #[test]
    fn an_allowed_action_adds_no_timeline_noise() {
        // Tervin had no opinion. A row per allowed action would bury the ones that
        // matter.
        let events = gate_events(
            &AgentIdentity::new("claude-code", "Claude Code", Tier::Structured),
            &tervin_core::ThreadId::new(),
            &None,
            "/tmp",
            &hooks::HookRequest {
                tool_name: "Read".into(),
                tool_input: json!({ "file_path": "/tmp/a.rs" }),
                ..Default::default()
            },
            &hooks::HookDecision::Defer {
                reason: "no objection".into(),
            },
        );
        assert!(events.is_empty());
    }

    #[test]
    fn the_offered_modes_match_what_the_runtime_accepts() {
        // A mode control listing something the runtime rejects is worse than none.
        let modes = permission_modes();
        assert_eq!(modes.len(), PERMISSION_MODES.len());
        for mode in &modes {
            assert!(
                PERMISSION_MODES.contains(&mode.id.as_str()),
                "{} is offered but not accepted",
                mode.id
            );
            assert!(
                mode.description.as_deref().is_some_and(|d| !d.is_empty()),
                "{} needs to say who decides",
                mode.id
            );
        }
    }

    #[test]
    fn bypass_permissions_is_not_an_offered_mode() {
        // Tervin will not present a one-click way to disable every check.
        assert!(!PERMISSION_MODES.contains(&"bypassPermissions"));
        assert!(PERMISSION_MODES.contains(&"plan"));
        assert!(PERMISSION_MODES.contains(&"manual"));
    }

    #[test]
    fn the_permission_bridge_is_advertised_as_partial_until_proven() {
        // The capability model must not promise a gate that may not exist.
        let caps = ClaudeCodeRuntime::static_capabilities();
        assert!(
            matches!(
                caps.native_permission_bridge,
                CapabilityLevel::Partial { .. }
            ),
            "bridge should start Partial, was {:?}",
            caps.native_permission_bridge
        );
        assert!(caps.native_permission_bridge.note().is_some());
    }

    #[test]
    fn remote_execution_is_declared_unsupported_with_a_reason() {
        let caps = ClaudeCodeRuntime::static_capabilities();
        match caps.remote_execution {
            CapabilityLevel::Unsupported { reason } => assert!(!reason.is_empty()),
            other => panic!("expected an explained refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn discovery_reports_a_missing_binary_without_failing() {
        let rt = ClaudeCodeRuntime {
            binary: "definitely-not-a-real-binary-xyz".to_string(),
            arbiter: None,
            executable: None,
        };
        let d = rt.discover().await;
        assert!(!d.available);
        assert!(d.version.is_none());
        assert!(
            !d.notes.is_empty(),
            "an unavailable runtime should explain itself"
        );
    }

    #[tokio::test]
    async fn discovery_finds_the_real_cli_when_installed() {
        let rt = ClaudeCodeRuntime::new();
        let d = rt.discover().await;
        // Skip rather than fail where Claude Code is not installed.
        if !d.available {
            return;
        }
        assert!(d.version.is_some(), "version should parse from --version");
        assert!(d.path.is_some(), "path should resolve from PATH");
    }

    #[test]
    fn which_resolves_a_known_binary() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }
}
