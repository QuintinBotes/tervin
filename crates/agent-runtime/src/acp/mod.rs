//! The Agent Client Protocol adapter — one adapter for every agent that speaks ACP.
//!
//! ACP standardises the editor↔agent relationship the way LSP standardised
//! editor↔language-server. That makes this the most valuable adapter in Tervin:
//! it is not an integration with one vendor, it is an integration with a protocol,
//! so an agent released next month works here without a code change.
//!
//! ## Why this adapter can do what the Claude Code adapter cannot
//!
//! ACP has [`client_method::REQUEST_PERMISSION`] — an **agent → client** request.
//! The agent describes what it is about to do and *waits* for an answer. Tervin
//! answers from Tervin Rules, and the agent honours it. That is a genuine
//! pre-execution gate, so here — and only here among the adapters written so far —
//! `tervin_can_intercept` is true, risk assessments are marked `enforceable`, and
//! decisions are attributed to [`DecisionAuthority::Tervin`] rather than to the
//! provider.
//!
//! The nuance the UI must still carry: the *agent* decides which actions are worth
//! asking about. Tervin's answer is binding for everything it is asked about, and
//! [`AcpSession::permissions`] says exactly that rather than implying Tervin sees
//! every action.
//!
//! ## What Tervin offers the agent in return
//!
//! Tervin declares three client capabilities and implements all three, because an
//! agent that is told a method exists and then gets an error has no way to recover:
//!
//! - `fs/read_text_file` and `fs/write_text_file`, confined to the session's own
//!   project root and refusing credential-shaped files. Confinement is enforced
//!   after symlink resolution, so `proj/link → ~/.ssh` does not escape it.
//! - `terminal/*`, which means the agent asks *Tervin* to run commands. Every one
//!   goes through Tervin Rules first, so the gate covers execution too, and the
//!   command and its output land in the timeline.

pub mod normalize;
pub mod protocol;

use crate::runtime::{
    AgentRuntime, AgentSession, ArbiterDecision, Attachment, Discovery, LaunchConfig,
    LaunchedSession, McpServerState, PermissionArbiter, PermissionState, Result, RuntimeDiagnostic,
    RuntimeError, SessionMetadata,
};
use async_trait::async_trait;
use normalize::Normalizer;
use parking_lot::Mutex;
use protocol::{
    agent_method, classify, client_method, parse_permission_request, parse_session_update,
    ClientCapabilities, Incoming, InitializeResult, Outgoing, StopReason, PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tervin_core::events::Severity;
use tervin_core::{AgentIdentity, Capabilities, CapabilityLevel, TervinEvent, ThreadId, Tier};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, oneshot};

/// Upper bound on a single JSON-RPC line, so a malformed stream cannot grow the
/// read buffer without limit.
const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;

/// How long to wait for a handshake-class request.
///
/// Deliberately not applied to `session/prompt`, which legitimately runs for many
/// minutes.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Default cap on how much output Tervin retains for one agent-hosted command.
const DEFAULT_TERMINAL_OUTPUT_LIMIT: usize = 1024 * 1024;

/// How long an agent gets to exit on its own after its stdin is closed, before
/// Tervin kills it.
///
/// A well-behaved agent leaves immediately at EOF. The kill exists because a badly
/// behaved one would otherwise outlive the workspace, and an orphaned agent holding
/// a model connection is worse than an abrupt one.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Largest file Tervin will read on an agent's behalf.
///
/// A bigger file is refused with its size rather than streamed, because an agent
/// cannot use a megabyte of source in one block and the refusal is more useful
/// than a truncation it was not told about.
const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;

/// JSON-RPC error codes Tervin returns.
const ERR_INVALID_PARAMS: i64 = -32602;
const ERR_INTERNAL: i64 = -32603;
/// Application-level refusal: the request was well-formed but Tervin declined.
const ERR_REFUSED: i64 = -32000;

// ---------------------------------------------------------------- agent specs

/// A known ACP-speaking agent and how to start it.
#[derive(Debug, Clone)]
pub struct AcpAgentSpec {
    pub runtime_id: String,
    pub display_name: String,
    pub binary: String,
    /// Arguments that put the binary into ACP mode.
    pub args: Vec<String>,
    /// Shown in the Bridge panel.
    pub note: String,
    /// How to get it, shown when the binary is missing.
    pub install_hint: String,
}

/// Agents that speak ACP through a first-party or vendor-published entry point.
///
/// This list is a convenience, not a limit: [`AcpRuntime::custom`] accepts any
/// command, which is the point of adopting a protocol rather than an API.
pub fn known_acp_agents() -> Vec<AcpAgentSpec> {
    vec![
        AcpAgentSpec {
            runtime_id: "gemini-acp".into(),
            display_name: "Gemini CLI".into(),
            binary: "gemini".into(),
            // Gemini CLI speaks ACP behind an experimental flag.
            args: vec!["--experimental-acp".into()],
            note: "Structured integration over the Agent Client Protocol. Tervin Rules \
                   gate every action the agent asks about."
                .into(),
            install_hint: "Install Gemini CLI (`npm i -g @google/gemini-cli`).".into(),
        },
        AcpAgentSpec {
            runtime_id: "copilot-acp".into(),
            display_name: "GitHub Copilot CLI".into(),
            binary: "copilot".into(),
            args: vec!["--acp".into()],
            note: "Structured integration over the Agent Client Protocol. Tervin Rules \
                   gate every action the agent asks about."
                .into(),
            install_hint: "Install GitHub Copilot CLI and sign in with `copilot`.".into(),
        },
        AcpAgentSpec {
            runtime_id: "claude-code-acp".into(),
            display_name: "Claude Code (ACP)".into(),
            binary: "claude-code-acp".into(),
            args: Vec::new(),
            note: "Claude Code through its ACP bridge. Unlike the direct adapter, this \
                   route gives Tervin a real pre-execution gate, at the cost of going \
                   through a third-party bridge."
                .into(),
            install_hint: "Install the bridge (`npm i -g @zed-industries/claude-code-acp`).".into(),
        },
    ]
}

// -------------------------------------------------------------------- runtime

/// An ACP agent, driven over JSON-RPC on stdio.
pub struct AcpRuntime {
    spec: AcpAgentSpec,
    arbiter: Option<Arc<dyn PermissionArbiter>>,
}

impl AcpRuntime {
    pub fn new(spec: AcpAgentSpec) -> Self {
        Self {
            spec,
            arbiter: None,
        }
    }

    /// Any ACP agent the user configures by command line.
    pub fn custom(
        runtime_id: impl Into<String>,
        display_name: impl Into<String>,
        binary: impl Into<String>,
        args: Vec<String>,
    ) -> Self {
        Self::new(AcpAgentSpec {
            runtime_id: runtime_id.into(),
            display_name: display_name.into(),
            binary: binary.into(),
            args,
            note: "A user-configured agent speaking the Agent Client Protocol.".into(),
            install_hint: String::new(),
        })
    }

    pub fn with_arbiter(mut self, arbiter: Arc<dyn PermissionArbiter>) -> Self {
        self.arbiter = Some(arbiter);
        self
    }

    pub fn spec(&self) -> &AcpAgentSpec {
        &self.spec
    }

    /// Capabilities the protocol itself guarantees.
    ///
    /// `native_permission_bridge` is `Supported` rather than `Partial` because the
    /// bridge is part of the protocol, not a property of one build: if the agent
    /// asks, Tervin's answer is binding. Which actions it asks about is the
    /// agent's choice, and that is stated in [`AcpSession::permissions`] rather
    /// than hidden behind the capability.
    fn static_capabilities() -> Capabilities {
        Capabilities {
            tier: Tier::Structured,
            // Plans arrive as a `plan` session update.
            plan_mode: CapabilityLevel::Supported,
            // Refined per session: only an agent that declares `loadSession` can
            // actually be resumed.
            resume: CapabilityLevel::partial(
                "Resuming works only with agents that declare `loadSession`; \
                 Tervin checks during the handshake.",
            ),
            tool_events: CapabilityLevel::Supported,
            file_edits: CapabilityLevel::Supported,
            native_permission_bridge: CapabilityLevel::Supported,
            mcp: CapabilityLevel::partial(
                "MCP servers can be passed to the agent when a session starts. \
                 The protocol does not report their connection state back.",
            ),
            hooks: CapabilityLevel::unsupported(
                "Hooks are a Claude Code feature. ACP has no equivalent, so Tervin \
                 does not offer them here.",
            ),
            // Subagents, if the agent has them, are invisible over this protocol:
            // there is no field that distinguishes a nested task from a tool call.
            subagents: CapabilityLevel::Unknown,
            image_input: CapabilityLevel::partial(
                "Images are sent when the agent declares image prompt support.",
            ),
            cost_reporting: CapabilityLevel::unsupported(
                "ACP does not carry token or cost accounting, so Tervin has nothing \
                 to report rather than an estimate.",
            ),
            model_selection: CapabilityLevel::unsupported(
                "The model is chosen by the agent's own configuration. ACP has no \
                 model selector.",
            ),
            remote_execution: CapabilityLevel::unsupported(
                "Runs as a local process. Remote work happens through the pane's own \
                 session.",
            ),
            multi_turn: CapabilityLevel::Supported,
            interrupt: CapabilityLevel::Supported,
        }
    }

    async fn start(&self, config: LaunchConfig, load: Option<&str>) -> Result<LaunchedSession> {
        // The root every filesystem request is confined to. Resolved once, so a
        // symlink created later cannot widen it.
        let project_root =
            std::fs::canonicalize(&config.cwd).unwrap_or_else(|_| PathBuf::from(&config.cwd));

        // A profile can point at a different install of the same agent. Its own
        // arguments go first; the spec's ACP flag has to survive, or the process
        // starts its interactive UI and never speaks the protocol.
        let binary = config
            .binary
            .clone()
            .filter(|b| !b.trim().is_empty())
            .unwrap_or_else(|| self.spec.binary.clone());
        let mut args = config.extra_args.clone();
        args.extend(self.spec.args.iter().cloned());

        let mut command = tokio::process::Command::new(&binary);
        // Removals must remove, not set an empty value: see `apply_env`.
        crate::runtime::apply_env(&mut command, &config.env);

        let mut child = command
            .args(&args)
            .current_dir(&config.cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Without this, a killed Tervin can leave the agent running.
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    RuntimeError::NotInstalled(binary.clone())
                } else {
                    RuntimeError::Launch {
                        runtime: self.spec.runtime_id.clone(),
                        source: e,
                    }
                }
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RuntimeError::Protocol("child stdin was not piped".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::Protocol("child stdout was not piped".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RuntimeError::Protocol("child stderr was not piped".into()))?;

        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let identity = AgentIdentity::new(
            self.spec.runtime_id.clone(),
            self.spec.display_name.clone(),
            Tier::Structured,
        );

        let shared = Arc::new(Shared {
            normalizer: Mutex::new(Normalizer::new(
                config.thread_id.clone(),
                identity,
                config.cwd.clone(),
            )),
            metadata: Mutex::new(SessionMetadata::default()),
            diagnostics: Mutex::new(Vec::new()),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            session_id: Mutex::new(None),
            available_modes: Mutex::new(Vec::new()),
            load_session: AtomicBool::new(false),
            image_prompts: AtomicBool::new(false),
            turn_active: AtomicBool::new(false),
            running: AtomicBool::new(true),
            permission_asks: AtomicU64::new(0),
            terminals: Mutex::new(HashMap::new()),
            next_terminal: AtomicU64::new(1),
            events: events_tx,
            project_root,
            cwd: config.cwd.clone(),
            thread_id: config.thread_id.clone(),
            runtime_id: self.spec.runtime_id.clone(),
        });

        let conn = Arc::new(Conn {
            shared: shared.clone(),
            stdin: tokio::sync::Mutex::new(Some(stdin)),
            arbiter: self.arbiter.clone(),
        });

        // Reader task: the protocol's only consumer.
        {
            let conn = conn.clone();
            tokio::spawn(async move {
                read_stream(stdout, conn.clone()).await;
                conn.shared.running.store(false, Ordering::SeqCst);
                // Nothing will answer outstanding requests now; releasing the
                // waiters turns a hang into an error.
                conn.shared.pending.lock().clear();
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
                    shared.note(Severity::Warning, line);
                }
            });
        }

        // Reap the child, so a crash becomes a Disconnected Thread rather than one
        // that appears to still be working. The channel lets `shutdown` escalate
        // from "stdin is closed" to an actual kill.
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        {
            let shared = shared.clone();
            tokio::spawn(async move {
                // `child.wait()` borrows mutably for the whole select, so the kill
                // has to happen after the select expression rather than in an arm.
                let finished = tokio::select! {
                    status = child.wait() => Some(status),
                    _ = stop_rx => None,
                };
                let status = match finished {
                    Some(status) => status,
                    None => match tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await {
                        Ok(status) => status,
                        Err(_) => {
                            let _ = child.kill().await;
                            child.wait().await
                        }
                    },
                };
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

        // --- handshake ---------------------------------------------------------

        let init = conn
            .request_with_timeout(
                agent_method::INITIALIZE,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientCapabilities": ClientCapabilities::default(),
                }),
                HANDSHAKE_TIMEOUT,
            )
            .await?;

        let init: InitializeResult = serde_json::from_value(init.clone()).unwrap_or_default();
        shared
            .load_session
            .store(init.agent_capabilities.load_session, Ordering::SeqCst);
        shared
            .image_prompts
            .store(init.agent_capabilities.prompt.image, Ordering::SeqCst);

        if init.protocol_version != PROTOCOL_VERSION {
            // Not fatal: the agent named the version it will use, and Tervin's
            // parsing is deliberately tolerant. Recorded so a later oddity has an
            // explanation.
            shared.note(
                Severity::Info,
                format!(
                    "The agent negotiated protocol version {} while Tervin implements {}.",
                    init.protocol_version, PROTOCOL_VERSION
                ),
            );
        }

        if !init.auth_methods.is_empty() {
            let names: Vec<&str> = init.auth_methods.iter().map(|m| m.name.as_str()).collect();
            // Tervin does not attempt authentication: these flows open browsers and
            // write credentials, which is the user's business, not an adapter's.
            shared.note(
                Severity::Info,
                format!(
                    "This agent offers authentication ({}). If a session fails to start, \
                     sign in with the agent's own CLI first.",
                    names.join(", ")
                ),
            );
        }

        // --- session -----------------------------------------------------------

        // Under ACP the client supplies MCP servers — the agent has no config of its
        // own to read — so without this an ACP agent would have no MCP at all.
        let (mcp, mcp_error) = crate::mcp::McpConfig::load();
        if let Some(error) = mcp_error {
            shared.note(Severity::Warning, error);
        }
        shared.metadata.lock().mcp_servers = mcp.declared_states();

        let session_params = json!({
            "cwd": config.cwd,
            "mcpServers": mcp.to_acp(),
        });

        let result = match load {
            Some(session_id) => {
                if !init.agent_capabilities.load_session {
                    return Err(RuntimeError::Unsupported {
                        runtime: self.spec.runtime_id.clone(),
                        feature: "resuming a session (the agent does not declare `loadSession`)"
                            .into(),
                    });
                }
                let mut params = session_params.clone();
                params["sessionId"] = json!(session_id);
                conn.request_with_timeout(agent_method::SESSION_LOAD, params, HANDSHAKE_TIMEOUT)
                    .await?;
                // `session/load` replays into the same session id.
                json!({ "sessionId": session_id })
            }
            None => {
                conn.request_with_timeout(
                    agent_method::SESSION_NEW,
                    session_params,
                    HANDSHAKE_TIMEOUT,
                )
                .await?
            }
        };

        let session_id = result
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| RuntimeError::Protocol("the agent did not return a session id".into()))?
            .to_string();

        *shared.session_id.lock() = Some(session_id.clone());
        record_modes(&shared, &result);

        let events = shared
            .normalizer
            .lock()
            .started(session_id, init.agent_capabilities.load_session);
        shared.emit(events);

        let session = AcpSession {
            conn: conn.clone(),
            stop: Mutex::new(Some(stop_tx)),
        };

        // An initial mode, where the agent offers modes at all.
        if let Some(mode) = config.permission_mode.clone() {
            if let Err(e) = session.set_permission_mode(&mode).await {
                shared.note(
                    Severity::Info,
                    format!("Could not select mode `{mode}`: {e}"),
                );
            }
        }

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
impl AgentRuntime for AcpRuntime {
    fn runtime_id(&self) -> &str {
        &self.spec.runtime_id
    }

    fn identity(&self) -> AgentIdentity {
        AgentIdentity::new(
            self.spec.runtime_id.clone(),
            self.spec.display_name.clone(),
            Tier::Structured,
        )
    }

    async fn discover(&self) -> Discovery {
        let path = crate::which(&self.spec.binary);
        let available = path.is_some();

        let mut notes = vec![self.spec.note.clone()];
        if !available {
            notes.insert(0, format!("`{}` was not found on PATH.", self.spec.binary));
            if !self.spec.install_hint.is_empty() {
                notes.push(self.spec.install_hint.clone());
            }
        }

        // A version is reported only when the binary answers `--version` plainly.
        // Guessing at the format of every agent's version output would print a
        // misleading number.
        let version = if available {
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                tokio::process::Command::new(&self.spec.binary)
                    .arg("--version")
                    .output(),
            )
            .await
            {
                Ok(Ok(out)) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty() && line.len() < 80),
                _ => None,
            }
        } else {
            None
        };

        Discovery {
            runtime_id: self.spec.runtime_id.clone(),
            display_name: self.spec.display_name.clone(),
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

    async fn launch(&self, config: LaunchConfig) -> Result<LaunchedSession> {
        self.start(config, None).await
    }

    async fn resume(&self, resume_id: &str, config: LaunchConfig) -> Result<LaunchedSession> {
        self.start(config, Some(resume_id)).await
    }
}

// --------------------------------------------------------------------- shared

/// A mode the agent offers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentMode {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

/// One agent-hosted command.
struct Terminal {
    /// Named in the diagnostic when a session ends while it is still running, so
    /// "Tervin killed something on your behalf" is never silent.
    command: String,
    output: Arc<Mutex<TerminalOutput>>,
    /// Signals the waiter task to kill the child.
    kill: Mutex<Option<oneshot::Sender<()>>>,
    /// Resolves when the process ends.
    exit: tokio::sync::watch::Receiver<Option<ExitInfo>>,
}

#[derive(Default)]
struct TerminalOutput {
    text: String,
    /// True once output was dropped, so the agent is told rather than silently
    /// given a partial answer.
    truncated: bool,
    limit: usize,
}

impl TerminalOutput {
    fn push(&mut self, chunk: &str) {
        if self.text.len() >= self.limit {
            self.truncated = true;
            return;
        }
        let room = self.limit - self.text.len();
        if chunk.len() <= room {
            self.text.push_str(chunk);
        } else {
            // Split on a character boundary so the buffer stays valid UTF-8.
            let mut cut = room;
            while cut > 0 && !chunk.is_char_boundary(cut) {
                cut -= 1;
            }
            self.text.push_str(&chunk[..cut]);
            self.truncated = true;
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ExitInfo {
    code: Option<i32>,
    signal: Option<i32>,
}

/// State shared between the session handle and every task.
struct Shared {
    normalizer: Mutex<Normalizer>,
    metadata: Mutex<SessionMetadata>,
    diagnostics: Mutex<Vec<RuntimeDiagnostic>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>>,
    next_id: AtomicU64,
    session_id: Mutex<Option<String>>,
    available_modes: Mutex<Vec<AgentMode>>,
    load_session: AtomicBool,
    image_prompts: AtomicBool,
    /// True while a `session/prompt` is outstanding. ACP allows one turn at a time.
    turn_active: AtomicBool,
    running: AtomicBool,
    /// How many times the gate actually fired, shown so a user can tell an agent
    /// that asks from one that never does.
    permission_asks: AtomicU64,
    terminals: Mutex<HashMap<String, Arc<Terminal>>>,
    next_terminal: AtomicU64,
    events: mpsc::UnboundedSender<TervinEvent>,
    /// Resolved boundary for every filesystem request.
    project_root: PathBuf,
    cwd: String,
    thread_id: ThreadId,
    runtime_id: String,
}

impl Shared {
    fn emit(&self, events: Vec<TervinEvent>) {
        for event in events {
            let _ = self.events.send(event);
        }
    }

    fn note(&self, severity: Severity, message: impl Into<String>) {
        self.diagnostics.lock().push(RuntimeDiagnostic {
            severity,
            message: message.into(),
            at: tervin_core::now(),
        });
    }

    fn session_id(&self) -> Result<String> {
        self.session_id
            .lock()
            .clone()
            .ok_or_else(|| RuntimeError::Protocol("no ACP session has been established".into()))
    }
}

/// Record the modes an agent reported, so the UI offers real options.
fn record_modes(shared: &Arc<Shared>, result: &Value) {
    let Some(modes) = result.get("modes") else {
        return;
    };
    let available: Vec<AgentMode> = modes
        .get("availableModes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|m| {
                    Some(AgentMode {
                        id: m.get("id")?.as_str()?.to_string(),
                        name: m
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        description: m
                            .get("description")
                            .and_then(Value::as_str)
                            .map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if !available.is_empty() {
        *shared.available_modes.lock() = available;
    }
    if let Some(current) = modes.get("currentModeId").and_then(Value::as_str) {
        shared.normalizer.lock().mode = Some(current.to_string());
    }
}

// ----------------------------------------------------------------- connection

/// The stdio connection: request correlation, writing, and answering.
struct Conn {
    shared: Arc<Shared>,
    /// `None` once shutdown has closed it.
    ///
    /// Held as an `Option` rather than a plain handle because *dropping* it is what
    /// closes the pipe. `AsyncWrite::shutdown` on a child's stdin does not close the
    /// descriptor, so an agent blocked on a read would never see EOF and would
    /// outlive the session.
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    arbiter: Option<Arc<dyn PermissionArbiter>>,
}

impl Conn {
    async fn write(&self, value: &Value) -> Result<()> {
        if !self.shared.running.load(Ordering::SeqCst) {
            return Err(RuntimeError::SessionEnded);
        }
        let mut line =
            serde_json::to_string(value).map_err(|e| RuntimeError::Protocol(e.to_string()))?;
        line.push('\n');
        let mut guard = self.stdin.lock().await;
        let Some(stdin) = guard.as_mut() else {
            return Err(RuntimeError::SessionEnded);
        };
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    // `Outgoing` is four fields — a `&'static str`, an `Option<u64>`, a `String`, and
    // an `Option<Value>` — and none of those can fail to serialise. `to_value` reports
    // an error only when a `Serialize` impl returns one or a map key is not a string,
    // and a derived impl over those types does neither. The `params` a caller passes in
    // is already a `Value`, so it has been through serde once before it arrives here.
    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        #[allow(
            clippy::unwrap_used,
            reason = "`Outgoing`'s fields cannot fail to serialise"
        )]
        let envelope = serde_json::to_value(Outgoing::notification(method, params)).unwrap();
        self.write(&envelope).await
    }

    /// Send a request and await its response. No timeout: `session/prompt` can
    /// legitimately run for many minutes.
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.shared.pending.lock().insert(id, tx);

        // The stream may have ended between the check inside `write` and this
        // insert. Without re-checking, the waiter would sit in a map nobody drains
        // and the caller would wait forever.
        if !self.shared.running.load(Ordering::SeqCst) {
            self.shared.pending.lock().remove(&id);
            return Err(RuntimeError::SessionEnded);
        }

        // Same envelope, same argument as `notify` above.
        #[allow(
            clippy::unwrap_used,
            reason = "`Outgoing`'s fields cannot fail to serialise"
        )]
        let envelope = serde_json::to_value(Outgoing::request(id, method, params)).unwrap();
        if let Err(e) = self.write(&envelope).await {
            self.shared.pending.lock().remove(&id);
            return Err(e);
        }

        match rx.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(message)) => Err(RuntimeError::Protocol(format!("{method}: {message}"))),
            // The waiter map is cleared when the stream ends.
            Err(_) => Err(RuntimeError::SessionEnded),
        }
    }

    async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: std::time::Duration,
    ) -> Result<Value> {
        match tokio::time::timeout(timeout, self.request(method, params)).await {
            Ok(result) => result,
            Err(_) => Err(RuntimeError::Protocol(format!(
                "{method} timed out after {}s",
                timeout.as_secs()
            ))),
        }
    }

    /// Answer a request the agent made.
    async fn respond(&self, id: u64, result: Value) {
        let _ = self
            .write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            .await;
    }

    /// Refuse a request the agent made, with a reason it can act on.
    async fn respond_error(&self, id: u64, code: i64, message: impl Into<String>) {
        let message = message.into();
        let _ = self
            .write(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message },
            }))
            .await;
    }

    /// Consult Tervin Rules. With no arbiter wired, an unanswerable gate fails
    /// closed — the alternative is a gate that silently allows everything.
    async fn decide(&self, tool: &str, input: &Value) -> ArbiterDecision {
        match &self.arbiter {
            Some(arbiter) => {
                arbiter
                    .decide(&self.shared.thread_id, tool, input, &self.shared.cwd)
                    .await
            }
            None => ArbiterDecision::Deny {
                reason: "No Tervin Rules arbiter is configured for this session.".into(),
            },
        }
    }
}

// -------------------------------------------------------------------- session

/// A live ACP session.
pub struct AcpSession {
    conn: Arc<Conn>,
    /// Tells the reaper task to stop waiting patiently and end the process.
    stop: Mutex<Option<oneshot::Sender<()>>>,
}

impl AcpSession {
    /// Modes the agent offers, for the UI's mode control.
    pub fn available_modes(&self) -> Vec<AgentMode> {
        self.conn.shared.available_modes.lock().clone()
    }

    /// Render a prompt as ACP content blocks.
    fn prompt_blocks(&self, content: &str, attachments: &[Attachment]) -> Vec<Value> {
        let images = self.conn.shared.image_prompts.load(Ordering::SeqCst);
        let mut blocks = Vec::new();

        for attachment in attachments {
            match attachment {
                Attachment::Image { media_type, data } if images => blocks.push(json!({
                    "type": "image",
                    "mimeType": media_type,
                    "data": data,
                })),
                // An agent that did not declare image support is told in words
                // rather than sent a block it will reject.
                Attachment::Image { media_type, .. } => blocks.push(json!({
                    "type": "text",
                    "text": format!(
                        "[An image ({media_type}) was attached, but this agent does not \
                         accept images, so it was not sent.]"
                    ),
                })),
                other => {
                    if let Some(text) = other.to_prompt_text() {
                        blocks.push(json!({ "type": "text", "text": text }));
                    }
                }
            }
        }

        blocks.push(json!({ "type": "text", "text": content }));
        blocks
    }
}

#[async_trait]
impl AgentSession for AcpSession {
    async fn send_input(&self, content: String, attachments: Vec<Attachment>) -> Result<()> {
        let shared = self.conn.shared.clone();
        let session_id = shared.session_id()?;

        // ACP runs one turn at a time. Queuing silently would reorder the
        // conversation; saying so lets the user interrupt first.
        if shared
            .turn_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(RuntimeError::Protocol(
                "the agent is still working on the previous turn — interrupt it first".into(),
            ));
        }

        let blocks = self.prompt_blocks(&content, &attachments);
        let events = shared.normalizer.lock().user_prompt(&content, &attachments);
        shared.emit(events);

        // The prompt is awaited off-thread: the call returns as soon as the turn
        // has started, and the stop reason arrives as an event when it ends.
        let conn = self.conn.clone();
        tokio::spawn(async move {
            let outcome = conn
                .request(
                    agent_method::SESSION_PROMPT,
                    json!({ "sessionId": session_id, "prompt": blocks }),
                )
                .await;
            conn.shared.turn_active.store(false, Ordering::SeqCst);

            match outcome {
                Ok(result) => {
                    let reason = StopReason::parse(&result);
                    let events = conn.shared.normalizer.lock().turn_ended(reason);
                    conn.shared.emit(events);
                }
                Err(RuntimeError::SessionEnded) => {
                    // The reaper task already reports the disconnect.
                }
                Err(e) => {
                    conn.shared.note(Severity::Error, e.to_string());
                    let events = conn
                        .shared
                        .normalizer
                        .lock()
                        .turn_ended(StopReason::Unknown);
                    conn.shared.emit(events);
                }
            }
        });

        Ok(())
    }

    async fn interrupt(&self) -> Result<()> {
        let session_id = self.conn.shared.session_id()?;
        // Cancellation is a notification: the agent acknowledges it by ending the
        // outstanding prompt with `stopReason: cancelled`, which is what moves the
        // Thread to Interrupted. Tervin does not pre-empt that.
        self.conn
            .notify(
                agent_method::SESSION_CANCEL,
                json!({ "sessionId": session_id }),
            )
            .await?;
        self.conn.shared.note(
            Severity::Info,
            "Cancellation sent. Waiting for the agent to stop.",
        );
        Ok(())
    }

    async fn set_permission_mode(&self, mode: &str) -> Result<()> {
        let session_id = self.conn.shared.session_id()?;
        let known = self.conn.shared.available_modes.lock().clone();

        // Modes are agent-specific. Rejecting an unknown one here beats sending it
        // and reporting a success the agent never granted.
        if !known.is_empty() && !known.iter().any(|m| m.id == mode) {
            return Err(RuntimeError::Unsupported {
                runtime: self.conn.shared.runtime_id.clone(),
                feature: format!(
                    "mode `{mode}` (this agent offers: {})",
                    known
                        .iter()
                        .map(|m| m.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }

        self.conn
            .request_with_timeout(
                agent_method::SESSION_SET_MODE,
                json!({ "sessionId": session_id, "modeId": mode }),
                HANDSHAKE_TIMEOUT,
            )
            .await?;
        self.conn.shared.normalizer.lock().mode = Some(mode.to_string());
        Ok(())
    }

    fn session_metadata(&self) -> SessionMetadata {
        let shared = &self.conn.shared;
        let mut meta = shared.metadata.lock().clone();
        let n = shared.normalizer.lock();
        // Resume is offered only where the agent can actually load a session.
        meta.resume_id = shared
            .load_session
            .load(Ordering::SeqCst)
            .then(|| n.session_id.clone())
            .flatten();
        meta.permission_mode = n.mode.clone();
        // The agent's own modes, so the UI never offers one it would reject.
        meta.modes = shared
            .available_modes
            .lock()
            .iter()
            .map(|m| crate::runtime::SessionMode {
                id: m.id.clone(),
                name: if m.name.is_empty() {
                    m.id.clone()
                } else {
                    m.name.clone()
                },
                description: m.description.clone(),
            })
            .collect();
        meta
    }

    fn permissions(&self) -> PermissionState {
        let shared = &self.conn.shared;
        let asks = shared.permission_asks.load(Ordering::SeqCst);
        // Read everything the normalizer holds under one guard. Two `lock()` calls
        // inside one struct literal would deadlock: the temporaries live to the end
        // of the statement, and the lock is not reentrant.
        let (mode, denials) = {
            let n = shared.normalizer.lock();
            (n.mode.clone(), n.denials.clone())
        };
        PermissionState {
            mode: mode.unwrap_or_else(|| "agent default".to_string()),
            // The distinguishing property of ACP: the agent waits for the answer.
            tervin_can_intercept: true,
            explanation: if asks == 0 {
                "This agent asks Tervin before acting, and Tervin Rules decide. It has \
                 not asked yet — the agent chooses which actions need permission."
                    .to_string()
            } else {
                format!(
                    "Tervin Rules decide: this agent has asked {asks} time(s) and honoured \
                     the answer. The agent chooses which actions need permission."
                )
            },
            denials,
        }
    }

    fn diagnostics(&self) -> Vec<RuntimeDiagnostic> {
        self.conn.shared.diagnostics.lock().clone()
    }

    fn capabilities(&self) -> Capabilities {
        let mut caps = AcpRuntime::static_capabilities();
        // Refined by the handshake rather than assumed.
        if self.conn.shared.load_session.load(Ordering::SeqCst) {
            caps.resume = CapabilityLevel::Supported;
        } else {
            caps.resume = CapabilityLevel::unsupported(
                "This agent does not declare `loadSession`, so its sessions cannot be \
                 resumed.",
            );
        }
        if self.conn.shared.image_prompts.load(Ordering::SeqCst) {
            caps.image_input = CapabilityLevel::Supported;
        } else {
            caps.image_input =
                CapabilityLevel::unsupported("This agent did not declare image prompt support.");
        }
        caps
    }

    fn is_running(&self) -> bool {
        self.conn.shared.running.load(Ordering::SeqCst)
    }

    async fn shutdown(&self) -> Result<()> {
        // Kill anything Tervin started on the agent's behalf: a session ending must
        // not leave orphaned commands running.
        let terminals: Vec<Arc<Terminal>> = self
            .conn
            .shared
            .terminals
            .lock()
            .values()
            .cloned()
            .collect();
        for terminal in terminals {
            if let Some(kill) = terminal.kill.lock().take() {
                let _ = kill.send(());
                // Killing something on the user's behalf is never done silently.
                self.conn.shared.note(
                    Severity::Info,
                    format!(
                        "Terminated `{}` because the session ended.",
                        terminal.command
                    ),
                );
            }
        }
        self.conn.shared.terminals.lock().clear();

        self.conn.shared.running.store(false, Ordering::SeqCst);

        // Dropping stdin closes the pipe, which is what asks the agent to finish.
        // `AsyncWrite::shutdown` alone would not: it leaves the descriptor open, so
        // an agent blocked on a read would never see EOF.
        {
            let mut guard = self.conn.stdin.lock().await;
            if let Some(stdin) = guard.take() {
                drop(stdin);
            }
        }

        // Then escalate: a well-behaved agent exits at EOF, and one that does not
        // gets killed rather than left holding a model connection.
        if let Some(stop) = self.stop.lock().take() {
            let _ = stop.send(());
        }
        Ok(())
    }
}

// ------------------------------------------------------------------- the loop

/// Read and dispatch the JSON-RPC stream until it ends.
async fn read_stream(stdout: tokio::process::ChildStdout, conn: Arc<Conn>) {
    let mut reader = BufReader::new(stdout);
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);

    loop {
        buf.clear();
        // `read_until` rather than `lines()` so an absurd line can be rejected
        // instead of buffered without limit.
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buf.len() > MAX_LINE_BYTES {
            conn.shared.note(
                Severity::Error,
                format!(
                    "Discarded a {} MB protocol line, which exceeds the {} MB limit.",
                    buf.len() / (1024 * 1024),
                    MAX_LINE_BYTES / (1024 * 1024)
                ),
            );
            continue;
        }

        let text = String::from_utf8_lossy(&buf);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            // Some agents print banners on stdout. Record it rather than treating
            // the stream as broken.
            conn.shared.note(
                Severity::Info,
                trimmed.chars().take(500).collect::<String>(),
            );
            continue;
        };

        let Some(message) = classify(&value) else {
            conn.shared.note(
                Severity::Info,
                format!(
                    "Ignored a message that is not a JSON-RPC envelope: {}",
                    trimmed.chars().take(200).collect::<String>()
                ),
            );
            continue;
        };

        match message {
            Incoming::Response { id, result } => {
                if let Some(tx) = conn.shared.pending.lock().remove(&id) {
                    let _ = tx.send(Ok(result));
                }
            }
            Incoming::Error { id, code, message } => {
                if let Some(tx) = conn.shared.pending.lock().remove(&id) {
                    let _ = tx.send(Err(format!("{message} (code {code})")));
                }
            }
            Incoming::Notification { method, params } => {
                handle_notification(&conn, &method, &params);
            }
            Incoming::Request { id, method, params } => {
                // Handled off the reader: a permission decision or a
                // `terminal/wait_for_exit` can take arbitrarily long, and blocking
                // here would stall the whole session.
                let conn = conn.clone();
                tokio::spawn(async move {
                    handle_request(conn, id, method, params).await;
                });
            }
        }
    }
}

/// Handle a one-way message from the agent.
fn handle_notification(conn: &Arc<Conn>, method: &str, params: &Value) {
    if method != client_method::SESSION_UPDATE {
        conn.shared.note(
            Severity::Info,
            format!("Unhandled notification `{method}`."),
        );
        return;
    }

    // Commands the agent offers are session metadata for composer autocomplete,
    // not something to put in the timeline.
    let body = params.get("update").unwrap_or(params);
    if body.get("sessionUpdate").and_then(Value::as_str) == Some("available_commands_update") {
        let commands: Vec<String> = body
            .get("availableCommands")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|c| c.get("name").and_then(Value::as_str).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if !commands.is_empty() {
            conn.shared.metadata.lock().slash_commands = commands;
        }
        return;
    }

    let Some((_, update)) = parse_session_update(params) else {
        conn.shared.note(
            Severity::Info,
            "Ignored a session update with no `sessionUpdate` field.",
        );
        return;
    };

    let events = conn.shared.normalizer.lock().ingest(update);
    conn.shared.emit(events);
}

/// Answer a request the agent made of Tervin.
async fn handle_request(conn: Arc<Conn>, id: u64, method: String, params: Value) {
    match method.as_str() {
        client_method::REQUEST_PERMISSION => handle_permission(conn, id, params).await,
        client_method::FS_READ_TEXT_FILE => handle_read(conn, id, params).await,
        client_method::FS_WRITE_TEXT_FILE => handle_write(conn, id, params).await,
        client_method::TERMINAL_CREATE => handle_terminal_create(conn, id, params).await,
        client_method::TERMINAL_OUTPUT => handle_terminal_output(conn, id, params).await,
        client_method::TERMINAL_WAIT_FOR_EXIT => handle_terminal_wait(conn, id, params).await,
        client_method::TERMINAL_KILL => handle_terminal_kill(conn, id, params).await,
        client_method::TERMINAL_RELEASE => handle_terminal_release(conn, id, params).await,
        other => {
            // Saying so beats a silent non-answer, which would hang the agent.
            conn.respond_error(id, -32601, format!("Tervin does not implement `{other}`."))
                .await;
        }
    }
}

// ---------------------------------------------------------------- permissions

/// The gate. This is the method that makes ACP worth adopting.
async fn handle_permission(conn: Arc<Conn>, id: u64, params: Value) {
    conn.shared.permission_asks.fetch_add(1, Ordering::SeqCst);

    let Some(request) = parse_permission_request(&params) else {
        conn.respond_error(id, ERR_INVALID_PARAMS, "Unreadable permission request.")
            .await;
        return;
    };

    // The action as the user will read it, and as the grant is keyed on.
    let action = request
        .raw_input
        .get("command")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| request.title.clone());

    let mut risk = match request.raw_input.get("command").and_then(Value::as_str) {
        Some(command) => rules_engine::classify(command, &conn.shared.cwd),
        None => tervin_core::RiskAssessment::benign(),
    };
    // Tervin's answer is binding here, so the assessment says so.
    risk.enforceable = true;

    let events = conn
        .shared
        .normalizer
        .lock()
        .awaiting_permission(&action, risk);
    conn.shared.emit(events);

    let decision = conn.decide(&request.kind, &request.raw_input).await;

    let (allow, reason) = match &decision {
        ArbiterDecision::Allow => (true, "Allowed by Tervin Rules".to_string()),
        ArbiterDecision::Deny { reason } => (false, reason.clone()),
    };

    // Never `allow_always` or `reject_always`: a standing grant is the user's to
    // give, not an adapter's to infer from one decision.
    let option = request
        .options
        .iter()
        .find(|o| o.is_allow() == allow && !o.is_always())
        .or_else(|| request.options.iter().find(|o| o.is_allow() == allow));

    match option {
        Some(option) => {
            let events = conn
                .shared
                .normalizer
                .lock()
                .decided(&action, allow, &reason);
            conn.shared.emit(events);
            conn.respond(
                id,
                json!({ "outcome": { "outcome": "selected", "optionId": option.id } }),
            )
            .await;
        }
        None => {
            // The agent offered no option matching the decision. Cancelling is the
            // only honest answer: picking the opposite would invert the decision.
            let events = conn.shared.normalizer.lock().decided(
                &action,
                false,
                &format!(
                    "{reason}. The agent offered no matching option, so the request was \
                     cancelled."
                ),
            );
            conn.shared.emit(events);
            conn.shared.note(
                Severity::Warning,
                format!(
                    "The agent asked about `{action}` but offered no {} option; the request \
                     was cancelled.",
                    if allow { "allow" } else { "reject" }
                ),
            );
            conn.respond(id, json!({ "outcome": { "outcome": "cancelled" } }))
                .await;
        }
    }
}

// ----------------------------------------------------------------- filesystem

/// Why a path was refused.
#[derive(Debug, PartialEq, Eq)]
enum PathRefusal {
    NotAbsolute,
    OutsideProject,
    Secret,
}

impl PathRefusal {
    fn message(&self, root: &Path) -> String {
        match self {
            Self::NotAbsolute => {
                "Tervin only accepts absolute paths, so there is no ambiguity about \
                 which file is meant."
                    .to_string()
            }
            Self::OutsideProject => format!(
                "Tervin only reads and writes inside the session's project ({}). Ask the \
                 user to open the other directory, or to attach the file to a prompt.",
                root.display()
            ),
            Self::Secret => "Tervin does not read or write files that hold credentials. If \
                             this file is genuinely needed, ask the user to attach it \
                             explicitly."
                .to_string(),
        }
    }
}

/// Names that hold credentials or key material.
///
/// Refusing these is a deliberate hole in the filesystem capability: an agent can
/// still be *given* such a file by the user attaching it, which keeps the decision
/// where it belongs.
fn looks_like_secret(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    const EXACT: [&str; 9] = [
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".htpasswd",
        "credentials",
        "credentials.json",
        ".git-credentials",
        "id_rsa",
        ".dockercfg",
    ];
    const PREFIXES: [&str; 4] = [".env", "id_ed25519", "id_ecdsa", "id_dsa"];
    const SUFFIXES: [&str; 6] = [".pem", ".key", ".p12", ".pfx", ".keystore", ".jks"];

    if EXACT.contains(&name.as_str()) {
        return true;
    }
    if PREFIXES.iter().any(|p| name.starts_with(p)) {
        return true;
    }
    if SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return true;
    }
    // `secrets.yaml`, `secret.json`, and friends.
    name.starts_with("secret")
}

/// Resolve a path the agent asked for, or say why not.
///
/// Confinement is checked *after* symlink resolution on the deepest existing
/// ancestor, so a symlink inside the project cannot be used to reach outside it.
fn resolve_in_project(root: &Path, raw: &str) -> std::result::Result<PathBuf, PathRefusal> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(PathRefusal::NotAbsolute);
    }

    // Walk up to the deepest ancestor that exists, canonicalise that, then rejoin
    // the remainder. This works for a file that is about to be created.
    let mut existing = path;
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let resolved_base = loop {
        match std::fs::canonicalize(existing) {
            Ok(base) => break base,
            Err(_) => match (existing.parent(), existing.file_name()) {
                (Some(parent), Some(name)) => {
                    tail.push(name.to_os_string());
                    existing = parent;
                }
                // Ran out of ancestors without finding anything real.
                _ => return Err(PathRefusal::OutsideProject),
            },
        }
    };

    let mut resolved = resolved_base;
    for name in tail.iter().rev() {
        // `..` in the unresolved tail could climb out; reject rather than guess.
        if name.as_os_str() == ".." {
            return Err(PathRefusal::OutsideProject);
        }
        resolved.push(name);
    }

    if !resolved.starts_with(root) {
        return Err(PathRefusal::OutsideProject);
    }
    if looks_like_secret(&resolved) {
        return Err(PathRefusal::Secret);
    }
    Ok(resolved)
}

async fn handle_read(conn: Arc<Conn>, id: u64, params: Value) {
    let Some(raw) = params.get("path").and_then(Value::as_str) else {
        conn.respond_error(id, ERR_INVALID_PARAMS, "`path` is required.")
            .await;
        return;
    };

    let path = match resolve_in_project(&conn.shared.project_root, raw) {
        Ok(path) => path,
        Err(refusal) => {
            conn.respond_error(id, ERR_REFUSED, refusal.message(&conn.shared.project_root))
                .await;
            return;
        }
    };

    match std::fs::metadata(&path) {
        Ok(meta) if meta.len() > MAX_READ_BYTES => {
            conn.respond_error(
                id,
                ERR_REFUSED,
                format!(
                    "{} is {} MB, above Tervin's {} MB limit for a single read. Read a \
                     range, or search it instead.",
                    path.display(),
                    meta.len() / (1024 * 1024),
                    MAX_READ_BYTES / (1024 * 1024)
                ),
            )
            .await;
            return;
        }
        Ok(_) => {}
        Err(e) => {
            conn.respond_error(id, ERR_INVALID_PARAMS, format!("{}: {e}", path.display()))
                .await;
            return;
        }
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            conn.respond_error(id, ERR_INVALID_PARAMS, format!("{}: {e}", path.display()))
                .await;
            return;
        }
    };

    // ACP allows a 1-based line offset and a limit.
    let line = params.get("line").and_then(Value::as_u64);
    let limit = params.get("limit").and_then(Value::as_u64);
    let content = if line.is_some() || limit.is_some() {
        let start = line.unwrap_or(1).saturating_sub(1) as usize;
        let lines: Vec<&str> = content.lines().skip(start).collect();
        let taken = match limit {
            Some(n) => lines.into_iter().take(n as usize).collect::<Vec<_>>(),
            None => lines,
        };
        taken.join("\n")
    } else {
        content
    };

    let events = {
        let mut n = conn.shared.normalizer.lock();
        n.ingest(protocol::SessionUpdate::ToolCall {
            id: format!("tervin-fs-read-{id}"),
            title: format!("Read {}", path.display()),
            kind: "read".into(),
            status: "completed".into(),
            raw_input: json!({ "path": path.display().to_string() }),
        })
    };
    conn.shared.emit(events);

    conn.respond(id, json!({ "content": content })).await;
}

async fn handle_write(conn: Arc<Conn>, id: u64, params: Value) {
    let Some(raw) = params.get("path").and_then(Value::as_str) else {
        conn.respond_error(id, ERR_INVALID_PARAMS, "`path` is required.")
            .await;
        return;
    };
    let content = params
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let path = match resolve_in_project(&conn.shared.project_root, raw) {
        Ok(path) => path,
        Err(refusal) => {
            conn.respond_error(id, ERR_REFUSED, refusal.message(&conn.shared.project_root))
                .await;
            return;
        }
    };

    // A write is a mutation, so it goes through Tervin Rules like any other.
    let display = path.display().to_string();
    let decision = conn
        .decide(
            "fs/write_text_file",
            &json!({ "path": display, "bytes": content.len() }),
        )
        .await;

    if let ArbiterDecision::Deny { reason } = &decision {
        let events =
            conn.shared
                .normalizer
                .lock()
                .decided(&format!("write {display}"), false, reason);
        conn.shared.emit(events);
        conn.respond_error(id, ERR_REFUSED, format!("Tervin Rules declined: {reason}"))
            .await;
        return;
    }

    let existed = path.exists();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            conn.respond_error(id, ERR_INTERNAL, format!("{}: {e}", parent.display()))
                .await;
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, &content) {
        conn.respond_error(id, ERR_INTERNAL, format!("{display}: {e}"))
            .await;
        return;
    }

    let events = {
        let mut n = conn.shared.normalizer.lock();
        let mut events = n.decided(&format!("write {display}"), true, "Allowed by Tervin Rules");
        events.extend(n.file_written(&display, existed));
        events
    };
    conn.shared.emit(events);

    conn.respond(id, Value::Null).await;
}

// ------------------------------------------------------------------ terminals

async fn handle_terminal_create(conn: Arc<Conn>, id: u64, params: Value) {
    let Some(command) = params.get("command").and_then(Value::as_str) else {
        conn.respond_error(id, ERR_INVALID_PARAMS, "`command` is required.")
            .await;
        return;
    };
    let args: Vec<String> = params
        .get("args")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|a| a.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Displayed and classified as the user would type it. Quoting matters: an
    // argument with a space must not read as two arguments.
    let display = shell_words::join(
        std::iter::once(command.to_string())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .iter()
            .map(String::as_str),
    );

    let cwd = match params.get("cwd").and_then(Value::as_str) {
        Some(raw) => match resolve_in_project(&conn.shared.project_root, raw) {
            Ok(path) => path,
            Err(refusal) => {
                conn.respond_error(id, ERR_REFUSED, refusal.message(&conn.shared.project_root))
                    .await;
                return;
            }
        },
        None => conn.shared.project_root.clone(),
    };

    // The gate covers execution too: the agent is asking Tervin to run this.
    let mut risk = rules_engine::classify(&display, &conn.shared.cwd);
    risk.enforceable = true;
    let events = conn
        .shared
        .normalizer
        .lock()
        .awaiting_permission(&display, risk);
    conn.shared.emit(events);

    let decision = conn.decide("execute", &json!({ "command": display })).await;
    if let ArbiterDecision::Deny { reason } = &decision {
        let events = conn
            .shared
            .normalizer
            .lock()
            .decided(&display, false, reason);
        conn.shared.emit(events);
        conn.respond_error(id, ERR_REFUSED, format!("Tervin Rules declined: {reason}"))
            .await;
        return;
    }
    let events = conn
        .shared
        .normalizer
        .lock()
        .decided(&display, true, "Allowed by Tervin Rules");
    conn.shared.emit(events);

    let env: Vec<(String, String)> = params
        .get("env")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|e| {
                    Some((
                        e.get("name")?.as_str()?.to_string(),
                        e.get("value")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let limit = params
        .get("outputByteLimit")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).min(DEFAULT_TERMINAL_OUTPUT_LIMIT))
        .unwrap_or(DEFAULT_TERMINAL_OUTPUT_LIMIT);

    // Spawned directly rather than through a shell: the agent gave a command and
    // arguments, and running them through `sh -c` would reintroduce word splitting
    // and globbing that the classifier just reasoned about.
    let mut child = match tokio::process::Command::new(command)
        .args(&args)
        .current_dir(&cwd)
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            conn.respond_error(id, ERR_INTERNAL, format!("could not run `{display}`: {e}"))
                .await;
            return;
        }
    };

    let output = Arc::new(Mutex::new(TerminalOutput {
        text: String::new(),
        truncated: false,
        limit,
    }));

    // stdout and stderr are interleaved into one buffer, which is what a terminal
    // does and what the agent expects to read back.
    for stream in [
        child.stdout.take().map(StdStream::Out),
        child.stderr.take().map(StdStream::Err),
    ]
    .into_iter()
    .flatten()
    {
        let output = output.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8 * 1024];
            let mut reader = stream.into_reader();
            loop {
                match tokio::io::AsyncReadExt::read(&mut reader, &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]);
                        output.lock().push(&chunk);
                    }
                }
            }
        });
    }

    let (kill_tx, kill_rx) = oneshot::channel::<()>();
    let (exit_tx, exit_rx) = tokio::sync::watch::channel(None);

    // The waiter reports the end of the command itself rather than leaving it to
    // `terminal/wait_for_exit`. An agent is not obliged to wait for something it
    // started, and a timeline that recorded an ending only when the agent happened
    // to ask would be missing exactly the commands nobody was watching.
    {
        let shared = conn.shared.clone();
        let output = output.clone();
        let display = display.clone();
        let started = tervin_core::now();
        tokio::spawn(async move {
            // `child.wait()` borrows mutably for the select, so the kill has to
            // happen after the select expression rather than inside an arm.
            let finished = tokio::select! {
                status = child.wait() => Some(status),
                _ = kill_rx => None,
            };
            let killed = finished.is_none();
            let status = match finished {
                Some(status) => status,
                None => {
                    let _ = child.kill().await;
                    child.wait().await
                }
            };
            let info = match status {
                Ok(status) => ExitInfo {
                    code: status.code(),
                    signal: signal_of(&status),
                },
                Err(_) => ExitInfo {
                    code: None,
                    signal: None,
                },
            };
            let _ = exit_tx.send(Some(info));

            // Let the output readers drain the closed pipes, so the excerpt is not
            // systematically short by its last chunk.
            tokio::task::yield_now().await;
            let text = output.lock().text.clone();
            let duration = (tervin_core::now() - started).num_milliseconds().max(0) as u64;
            // A killed process has no exit status of its own. -1 stands in, and the
            // summary says it was terminated rather than presenting that as a real
            // exit code.
            let events = shared.normalizer.lock().command_finished(
                &display,
                info.code.unwrap_or(-1),
                duration,
                &text,
                killed,
            );
            shared.emit(events);
        });
    }

    let terminal_id = format!(
        "tervin-term-{}",
        conn.shared.next_terminal.fetch_add(1, Ordering::SeqCst)
    );
    conn.shared.terminals.lock().insert(
        terminal_id.clone(),
        Arc::new(Terminal {
            command: display.clone(),
            output,
            kill: Mutex::new(Some(kill_tx)),
            exit: exit_rx,
        }),
    );

    let events = conn.shared.normalizer.lock().command_started(&display);
    conn.shared.emit(events);

    conn.respond(id, json!({ "terminalId": terminal_id })).await;
}

/// One of a child's output streams.
enum StdStream {
    Out(tokio::process::ChildStdout),
    Err(tokio::process::ChildStderr),
}

impl StdStream {
    fn into_reader(self) -> Box<dyn tokio::io::AsyncRead + Unpin + Send> {
        match self {
            Self::Out(s) => Box::new(s),
            Self::Err(s) => Box::new(s),
        }
    }
}

#[cfg(unix)]
fn signal_of(status: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(status)
}

#[cfg(not(unix))]
fn signal_of(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

fn terminal_of(conn: &Arc<Conn>, params: &Value) -> Option<(String, Arc<Terminal>)> {
    let id = params
        .get("terminalId")
        .and_then(Value::as_str)?
        .to_string();
    let terminal = conn.shared.terminals.lock().get(&id).cloned()?;
    Some((id, terminal))
}

/// The exit status as ACP spells it, or null while the process is still running.
fn exit_status_json(info: Option<ExitInfo>) -> Value {
    match info {
        Some(info) => json!({ "exitCode": info.code, "signal": info.signal }),
        None => Value::Null,
    }
}

async fn handle_terminal_output(conn: Arc<Conn>, id: u64, params: Value) {
    let Some((_, terminal)) = terminal_of(&conn, &params) else {
        conn.respond_error(id, ERR_INVALID_PARAMS, "No such terminal.")
            .await;
        return;
    };

    let (text, truncated) = {
        let out = terminal.output.lock();
        (out.text.clone(), out.truncated)
    };
    let exit = *terminal.exit.borrow();

    conn.respond(
        id,
        json!({
            "output": text,
            // Reported rather than hidden: an agent acting on a silently truncated
            // output would draw a wrong conclusion.
            "truncated": truncated,
            "exitStatus": exit_status_json(exit),
        }),
    )
    .await;
}

async fn handle_terminal_wait(conn: Arc<Conn>, id: u64, params: Value) {
    let Some((_, terminal)) = terminal_of(&conn, &params) else {
        conn.respond_error(id, ERR_INVALID_PARAMS, "No such terminal.")
            .await;
        return;
    };

    let mut exit = terminal.exit.clone();
    // Already finished, or wait for the change. The command's ending is reported by
    // the waiter task, so this only answers the agent.
    let info = loop {
        if let Some(info) = *exit.borrow_and_update() {
            break Some(info);
        }
        if exit.changed().await.is_err() {
            break None;
        }
    };

    conn.respond(id, exit_status_json(info)).await;
}

async fn handle_terminal_kill(conn: Arc<Conn>, id: u64, params: Value) {
    let Some((_, terminal)) = terminal_of(&conn, &params) else {
        conn.respond_error(id, ERR_INVALID_PARAMS, "No such terminal.")
            .await;
        return;
    };
    if let Some(kill) = terminal.kill.lock().take() {
        let _ = kill.send(());
    }
    conn.respond(id, Value::Null).await;
}

async fn handle_terminal_release(conn: Arc<Conn>, id: u64, params: Value) {
    let Some((terminal_id, terminal)) = terminal_of(&conn, &params) else {
        conn.respond_error(id, ERR_INVALID_PARAMS, "No such terminal.")
            .await;
        return;
    };
    // Releasing also kills: leaving a process running after the agent has stopped
    // reading it would orphan it.
    if let Some(kill) = terminal.kill.lock().take() {
        let _ = kill.send(());
    }
    conn.shared.terminals.lock().remove(&terminal_id);
    conn.respond(id, Value::Null).await;
}

/// MCP server state, for parity with the Bridge panel. ACP does not report
/// connection status, so this is what Tervin passed rather than what connected.
pub fn declared_mcp_servers(names: &[String]) -> Vec<McpServerState> {
    names
        .iter()
        .map(|name| McpServerState {
            name: name.clone(),
            status: "declared (ACP does not report connection state)".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> AcpAgentSpec {
        AcpAgentSpec {
            runtime_id: "test-acp".into(),
            display_name: "Test ACP agent".into(),
            binary: "definitely-not-a-real-binary-xyz".into(),
            args: vec!["--acp".into()],
            note: "n".into(),
            install_hint: "install it".into(),
        }
    }

    #[test]
    fn the_permission_bridge_is_declared_supported_because_the_protocol_guarantees_it() {
        // Unlike the Claude Code adapter, this is not version-dependent: if the
        // agent asks, Tervin's answer is binding.
        let caps = AcpRuntime::static_capabilities();
        assert!(matches!(
            caps.native_permission_bridge,
            CapabilityLevel::Supported
        ));
    }

    #[test]
    fn capabilities_acp_does_not_carry_are_refused_with_a_reason() {
        let caps = AcpRuntime::static_capabilities();
        for (name, level) in [
            ("cost_reporting", &caps.cost_reporting),
            ("model_selection", &caps.model_selection),
            ("hooks", &caps.hooks),
        ] {
            match level {
                CapabilityLevel::Unsupported { reason } => {
                    assert!(!reason.is_empty(), "{name} refused without a reason")
                }
                other => panic!("{name} should be refused, was {other:?}"),
            }
        }
    }

    #[test]
    fn resume_starts_partial_because_it_depends_on_the_agent() {
        let caps = AcpRuntime::static_capabilities();
        assert!(matches!(caps.resume, CapabilityLevel::Partial { .. }));
    }

    #[tokio::test]
    async fn a_missing_binary_is_reported_with_how_to_get_it() {
        let rt = AcpRuntime::new(spec());
        let d = rt.discover().await;
        assert!(!d.available);
        assert!(d.notes.iter().any(|n| n.contains("not found on PATH")));
        assert!(d.notes.iter().any(|n| n.contains("install it")));
    }

    #[tokio::test]
    async fn launching_a_missing_binary_says_it_is_not_installed() {
        let rt = AcpRuntime::new(spec());
        match rt.launch(LaunchConfig::new(ThreadId::new(), "/tmp")).await {
            Err(RuntimeError::NotInstalled(binary)) => {
                assert_eq!(binary, "definitely-not-a-real-binary-xyz")
            }
            Err(other) => panic!("expected NotInstalled, got {other:?}"),
            Ok(_) => panic!("a missing binary must not launch"),
        }
    }

    #[test]
    fn the_known_agents_all_declare_how_to_start_them() {
        let agents = known_acp_agents();
        assert!(!agents.is_empty());
        for agent in agents {
            assert!(!agent.binary.is_empty());
            assert!(!agent.note.is_empty());
            assert!(!agent.install_hint.is_empty());
            assert!(!agent.runtime_id.is_empty());
        }
    }

    #[test]
    fn gemini_is_started_in_acp_mode() {
        // Without the flag Gemini CLI runs its own interactive UI and never speaks
        // the protocol.
        let gemini = known_acp_agents()
            .into_iter()
            .find(|a| a.binary == "gemini")
            .expect("no gemini entry");
        assert!(gemini.args.iter().any(|a| a == "--experimental-acp"));
    }

    // --- the filesystem boundary ------------------------------------------------

    #[test]
    fn a_relative_path_is_refused() {
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        assert_eq!(
            resolve_in_project(&root, "src/main.rs"),
            Err(PathRefusal::NotAbsolute)
        );
    }

    #[test]
    fn a_path_inside_the_project_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("main.rs"), "fn main() {}").unwrap();

        let resolved = resolve_in_project(&root, root.join("main.rs").to_str().unwrap()).unwrap();
        assert!(resolved.starts_with(&root));
    }

    #[test]
    fn a_file_that_does_not_exist_yet_still_resolves_for_writing() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let target = root.join("new").join("deep").join("file.rs");
        let resolved = resolve_in_project(&root, target.to_str().unwrap()).unwrap();
        assert!(resolved.starts_with(&root));
        assert!(resolved.ends_with("file.rs"));
    }

    #[test]
    fn a_path_outside_the_project_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            resolve_in_project(&root, "/etc/hosts"),
            Err(PathRefusal::OutsideProject)
        );
    }

    #[test]
    fn dot_dot_cannot_climb_out_of_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let escape = format!("{}/../../etc/hosts", root.display());
        assert!(
            resolve_in_project(&root, &escape).is_err(),
            "`..` must not escape the project root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_project_is_refused() {
        // The check that matters: confinement is meaningless if a link inside the
        // project can point anywhere.
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let secret = std::fs::canonicalize(outside.path()).unwrap().join("t.txt");
        std::fs::write(&secret, "elsewhere").unwrap();

        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();

        assert_eq!(
            resolve_in_project(&root, link.to_str().unwrap()),
            Err(PathRefusal::OutsideProject),
            "a symlink must not widen the boundary"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_out_of_the_project_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let elsewhere = std::fs::canonicalize(outside.path()).unwrap();
        std::fs::write(elsewhere.join("t.txt"), "x").unwrap();

        std::os::unix::fs::symlink(&elsewhere, root.join("out")).unwrap();
        let through = root.join("out").join("t.txt");
        assert_eq!(
            resolve_in_project(&root, through.to_str().unwrap()),
            Err(PathRefusal::OutsideProject)
        );
    }

    #[test]
    fn credential_shaped_files_are_refused_even_inside_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();

        for name in [
            ".env",
            ".env.local",
            "id_rsa",
            "id_ed25519",
            "server.pem",
            "private.key",
            ".netrc",
            ".npmrc",
            "credentials",
            "secrets.yaml",
            "keystore.jks",
        ] {
            std::fs::write(root.join(name), "x").unwrap();
            assert_eq!(
                resolve_in_project(&root, root.join(name).to_str().unwrap()),
                Err(PathRefusal::Secret),
                "{name} should be refused"
            );
        }
    }

    #[test]
    fn ordinary_files_are_not_mistaken_for_secrets() {
        // Over-refusing would make the capability useless.
        for name in [
            "main.rs",
            "environment.ts",
            "keyboard.rs",
            "secure_random.rs",
            "README.md",
            "package.json",
        ] {
            assert!(
                !looks_like_secret(Path::new(name)),
                "{name} should not be treated as a secret"
            );
        }
    }

    #[test]
    fn every_refusal_explains_itself_and_names_the_root() {
        let root = Path::new("/Users/dev/proj");
        assert!(PathRefusal::NotAbsolute.message(root).contains("absolute"));
        assert!(PathRefusal::OutsideProject
            .message(root)
            .contains("/Users/dev/proj"));
        assert!(PathRefusal::Secret.message(root).contains("credentials"));
    }

    // --- bounded output ---------------------------------------------------------

    #[test]
    fn terminal_output_is_capped_and_says_when_it_truncated() {
        let mut out = TerminalOutput {
            text: String::new(),
            truncated: false,
            limit: 10,
        };
        out.push("12345");
        assert!(!out.truncated);
        out.push("67890abcdef");
        assert!(out.truncated, "exceeding the limit must be reported");
        assert_eq!(out.text.len(), 10);
        // Further writes do not grow it.
        out.push("more");
        assert_eq!(out.text.len(), 10);
    }

    #[test]
    fn truncation_never_splits_a_character() {
        // A cut inside a multi-byte character would make the buffer invalid UTF-8.
        let mut out = TerminalOutput {
            text: String::new(),
            truncated: false,
            limit: 4,
        };
        out.push("ab");
        // "é" is two bytes, so only one of the two remaining bytes is usable.
        out.push("éé");
        assert!(out.truncated);
        assert!(out.text.is_char_boundary(out.text.len()));
        assert!(out.text.starts_with("ab"));
    }

    #[test]
    fn an_exit_status_is_null_while_the_process_runs() {
        assert_eq!(exit_status_json(None), Value::Null);
        let done = exit_status_json(Some(ExitInfo {
            code: Some(3),
            signal: None,
        }));
        assert_eq!(done["exitCode"], json!(3));
    }

    #[test]
    fn declared_mcp_servers_do_not_claim_to_be_connected() {
        let servers = declared_mcp_servers(&["fs".to_string()]);
        assert_eq!(servers.len(), 1);
        assert!(
            servers[0].status.contains("does not report"),
            "status must not imply a connection Tervin cannot see"
        );
    }
}
