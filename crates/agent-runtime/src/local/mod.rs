//! Local and OpenAI-compatible model endpoints.
//!
//! LM Studio, Ollama, vLLM, and llama.cpp all expose the same shape: `GET /v1/models`
//! and a streaming `POST /v1/chat/completions`. So this is one adapter for all of
//! them and for any remote endpoint that speaks the same dialect — the same argument
//! as the ACP adapter, applied to models rather than agents.
//!
//! ## What this is not
//!
//! A chat endpoint is not an agent. It has no tools, writes no files, runs no
//! commands, and produces no plan — so there is nothing to approve, and
//! [`Tier::Conversational`] says so rather than dressing it up as a structured
//! runtime with most of its capabilities switched off.
//!
//! What it is good for is the thing that has no other answer in Tervin: reasoning
//! about the workspace's own context cheaply and privately. A local model can read a
//! Block, a diff, or a failing test and say something useful without any of it
//! leaving the machine — which is the whole point of running one.
//!
//! ## Nothing is sent that was not attached
//!
//! Like every runtime here, this one only sees explicit [`Attachment`]s. A local
//! endpoint feels safe enough that quietly including scrollback would be tempting;
//! it is exactly as forbidden here as anywhere else.

use crate::runtime::{
    AgentRuntime, AgentSession, Attachment, Discovery, LaunchConfig, LaunchedSession,
    PermissionState, Result, RuntimeDiagnostic, RuntimeError, SessionMetadata, SessionMode,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tervin_core::events::CostSnapshot;
use tervin_core::{
    AgentIdentity, Capabilities, CapabilityLevel, EventPayload, TervinEvent, ThreadId, ThreadState,
    Tier,
};
use tokio::sync::mpsc;

/// How long to wait for an endpoint to say it exists.
///
/// Short: discovery runs at startup for every configured endpoint, and a machine
/// with none of them running must not pay for that.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// Largest response Tervin will accumulate from one turn.
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

/// A local runtime Tervin knows how to look for.
#[derive(Debug, Clone)]
pub struct LocalEndpoint {
    pub runtime_id: String,
    pub display_name: String,
    /// Base URL including the `/v1` prefix.
    pub base_url: String,
    /// Shown when nothing answers.
    pub install_hint: String,
}

/// The endpoints Tervin probes by default.
///
/// Ports are the documented defaults for each. This list is a convenience, not a
/// limit: [`LocalModelRuntime::custom`] accepts any base URL.
pub fn known_local_endpoints() -> Vec<LocalEndpoint> {
    vec![
        LocalEndpoint {
            runtime_id: "lmstudio".into(),
            display_name: "LM Studio".into(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            install_hint: "Start LM Studio and enable its local server.".into(),
        },
        LocalEndpoint {
            runtime_id: "ollama".into(),
            display_name: "Ollama".into(),
            // Ollama serves an OpenAI-compatible surface alongside its own API.
            base_url: "http://127.0.0.1:11434/v1".into(),
            install_hint: "Install Ollama and run `ollama serve`.".into(),
        },
        LocalEndpoint {
            runtime_id: "vllm".into(),
            display_name: "vLLM".into(),
            base_url: "http://127.0.0.1:8000/v1".into(),
            install_hint: "Start vLLM with `vllm serve <model>`.".into(),
        },
        LocalEndpoint {
            runtime_id: "llamacpp".into(),
            display_name: "llama.cpp server".into(),
            base_url: "http://127.0.0.1:8080/v1".into(),
            install_hint: "Start `llama-server` with a model loaded.".into(),
        },
    ]
}

/// An OpenAI-compatible model endpoint.
pub struct LocalModelRuntime {
    endpoint: LocalEndpoint,
    /// Sent as a bearer token. Local servers ignore it; remote ones need it.
    api_key: Option<String>,
    client: reqwest::Client,
}

impl LocalModelRuntime {
    pub fn new(endpoint: LocalEndpoint) -> Self {
        Self {
            endpoint,
            api_key: None,
            client: reqwest::Client::new(),
        }
    }

    /// Any endpoint speaking the same dialect, local or remote.
    pub fn custom(
        runtime_id: impl Into<String>,
        display_name: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self::new(LocalEndpoint {
            runtime_id: runtime_id.into(),
            display_name: display_name.into(),
            base_url: normalise_base_url(&base_url.into()),
            install_hint: String::new(),
        })
    }

    pub fn with_api_key(mut self, key: Option<String>) -> Self {
        self.api_key = key.filter(|k| !k.trim().is_empty());
        self
    }

    pub fn endpoint(&self) -> &LocalEndpoint {
        &self.endpoint
    }

    /// What a model endpoint can and cannot do, stated plainly.
    fn static_capabilities() -> Capabilities {
        // One reason, used for everything that needs the ability to act.
        let cannot_act = || {
            CapabilityLevel::unsupported(
                "This is a model endpoint, not an agent: it answers, and cannot run \
                 commands, edit files, or use tools.",
            )
        };
        Capabilities {
            tier: Tier::Conversational,
            plan_mode: cannot_act(),
            resume: CapabilityLevel::unsupported(
                "The conversation lives in Tervin, not on the server, so there is no \
                 session to resume — reopening the Thread keeps its history.",
            ),
            tool_events: cannot_act(),
            file_edits: cannot_act(),
            native_permission_bridge: CapabilityLevel::unsupported(
                "Nothing to gate: this runtime cannot take an action.",
            ),
            mcp: cannot_act(),
            hooks: cannot_act(),
            subagents: cannot_act(),
            image_input: CapabilityLevel::partial(
                "Images are sent when the endpoint accepts them; most local models do not.",
            ),
            cost_reporting: CapabilityLevel::partial(
                "Token counts are reported when the endpoint returns them. There is no \
                 price for a model running on your own machine.",
            ),
            // The point of the adapter: pick from what the server actually has.
            model_selection: CapabilityLevel::Supported,
            remote_execution: CapabilityLevel::unsupported(
                "Runs wherever the endpoint is. Tervin does not execute anything for it.",
            ),
            multi_turn: CapabilityLevel::Supported,
            interrupt: CapabilityLevel::Supported,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.endpoint.base_url.trim_end_matches('/'), path)
    }

    fn authorized(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => builder.bearer_auth(key),
            None => builder,
        }
    }

    /// The models the server actually has.
    pub async fn models(&self) -> Result<Vec<String>> {
        let response = self
            .authorized(self.client.get(self.url("models")))
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .map_err(|e| RuntimeError::Protocol(e.to_string()))?;

        if !response.status().is_success() {
            return Err(RuntimeError::Protocol(format!(
                "{} returned {}",
                self.url("models"),
                response.status()
            )));
        }

        let body: Value = response
            .json()
            .await
            .map_err(|e| RuntimeError::Protocol(e.to_string()))?;
        Ok(parse_model_list(&body))
    }
}

/// Pull model ids out of a `/v1/models` response.
///
/// Tolerant on purpose: the shape is `{"data":[{"id":...}]}` everywhere that matters,
/// but a server returning a bare array should not make Tervin report no models.
pub fn parse_model_list(body: &Value) -> Vec<String> {
    let items = body
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| body.as_array());
    items
        .map(|list| {
            list.iter()
                .filter_map(|m| {
                    m.get("id")
                        .and_then(Value::as_str)
                        .or_else(|| m.get("name").and_then(Value::as_str))
                        .or_else(|| m.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Accept a base URL written the way a person would, with or without `/v1`.
pub fn normalise_base_url(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        // A bare `localhost:1234` is what someone types; http is the only sensible
        // reading for a loopback address.
        format!("http://{trimmed}")
    };
    if with_scheme.ends_with("/v1") {
        with_scheme
    } else {
        format!("{with_scheme}/v1")
    }
}

#[async_trait]
impl AgentRuntime for LocalModelRuntime {
    fn runtime_id(&self) -> &str {
        &self.endpoint.runtime_id
    }

    fn identity(&self) -> AgentIdentity {
        AgentIdentity::new(
            self.endpoint.runtime_id.clone(),
            self.endpoint.display_name.clone(),
            Tier::Conversational,
        )
    }

    async fn discover(&self) -> Discovery {
        let mut notes = Vec::new();
        let (available, version) = match self.models().await {
            Ok(models) if !models.is_empty() => {
                notes.push(format!(
                    "{} model(s) loaded: {}",
                    models.len(),
                    models
                        .iter()
                        .take(4)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                // The first model's name is more useful here than a server version,
                // which most of these do not report.
                (true, models.first().cloned())
            }
            Ok(_) => {
                notes.push(
                    "The endpoint answered but has no model loaded. Load one and refresh."
                        .to_string(),
                );
                (false, None)
            }
            Err(_) => {
                notes.push(format!(
                    "Nothing is answering at {}.",
                    self.endpoint.base_url
                ));
                if !self.endpoint.install_hint.is_empty() {
                    notes.push(self.endpoint.install_hint.clone());
                }
                (false, None)
            }
        };

        notes.push(
            "Answers questions about your workspace and carries context between agents. \
             It cannot run commands or edit files."
                .to_string(),
        );

        Discovery {
            runtime_id: self.endpoint.runtime_id.clone(),
            display_name: self.endpoint.display_name.clone(),
            available,
            version,
            path: Some(self.endpoint.base_url.clone()),
            notes,
            capabilities: Self::static_capabilities(),
        }
    }

    fn capabilities(&self) -> Capabilities {
        Self::static_capabilities()
    }

    async fn launch(&self, config: LaunchConfig) -> Result<LaunchedSession> {
        // A model is chosen, not discovered mid-conversation: fixing it at launch
        // keeps a Thread's answers attributable to one model.
        let model = match config.model.clone() {
            Some(model) => model,
            None => self.models().await?.into_iter().next().ok_or_else(|| {
                RuntimeError::Protocol(
                    "the endpoint has no model loaded, so there is nothing to ask".into(),
                )
            })?,
        };

        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let identity = self.identity();

        let shared = Arc::new(Shared {
            identity: identity.clone(),
            thread_id: config.thread_id.clone(),
            cwd: config.cwd.clone(),
            project: std::path::Path::new(&config.cwd)
                .file_name()
                .and_then(|s| s.to_str())
                .map(String::from),
            model: model.clone(),
            history: Mutex::new(Vec::new()),
            state: Mutex::new(ThreadState::Starting),
            cost: Mutex::new(CostSnapshot::default()),
            diagnostics: Mutex::new(Vec::new()),
            turn_active: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            cancel: tokio::sync::Notify::new(),
            running: AtomicBool::new(true),
            events: events_tx,
        });

        shared.emit(vec![shared.event(
            format!("Session started · {model}"),
            EventPayload::ThreadStarted {
                tier: Tier::Conversational,
                task_title: config.task_title.clone(),
                // The conversation lives in Tervin, so there is nothing on the
                // server to resume.
                resume_id: None,
            },
        )]);
        shared.transition(ThreadState::AwaitingInput);

        let session = LocalSession {
            shared: shared.clone(),
            client: self.client.clone(),
            url: self.url("chat/completions"),
            api_key: self.api_key.clone(),
        };

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

    async fn resume(&self, _resume_id: &str, _config: LaunchConfig) -> Result<LaunchedSession> {
        Err(RuntimeError::Unsupported {
            runtime: self.endpoint.runtime_id.clone(),
            feature: "resuming (the conversation is kept by Tervin, not the server)".into(),
        })
    }
}

/// One message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

struct Shared {
    identity: AgentIdentity,
    thread_id: ThreadId,
    cwd: String,
    project: Option<String>,
    model: String,
    /// The whole conversation. Held here because the server keeps nothing.
    history: Mutex<Vec<ChatMessage>>,
    state: Mutex<ThreadState>,
    cost: Mutex<CostSnapshot>,
    diagnostics: Mutex<Vec<RuntimeDiagnostic>>,
    turn_active: AtomicBool,
    cancelled: AtomicBool,
    /// Wakes the turn when it is cancelled.
    ///
    /// A flag alone is not enough: the stream loop only sees a flag when a chunk
    /// arrives, and a model that has gone quiet — a slow first token, which local
    /// models do constantly — would never be interrupted at all.
    cancel: tokio::sync::Notify,
    running: AtomicBool,
    events: mpsc::UnboundedSender<TervinEvent>,
}

impl Shared {
    fn event(&self, summary: impl Into<String>, payload: EventPayload) -> TervinEvent {
        TervinEvent::new(self.identity.clone(), summary, payload)
            .with_thread(self.thread_id.clone())
            .with_location(self.project.clone(), Some(self.cwd.clone()))
    }

    fn emit(&self, events: Vec<TervinEvent>) {
        for event in events {
            let _ = self.events.send(event);
        }
    }

    fn transition(&self, next: ThreadState) {
        let changed = {
            let mut state = self.state.lock();
            if *state == next {
                false
            } else {
                *state = next;
                true
            }
        };
        if changed {
            self.emit(vec![
                self.event(next.label(), EventPayload::ThreadState { state: next })
            ]);
        }
    }

    fn note(&self, severity: tervin_core::events::Severity, message: impl Into<String>) {
        self.diagnostics.lock().push(RuntimeDiagnostic {
            severity,
            message: message.into(),
            at: tervin_core::now(),
        });
    }
}

/// A conversation with a model endpoint.
pub struct LocalSession {
    shared: Arc<Shared>,
    client: reqwest::Client,
    url: String,
    api_key: Option<String>,
}

#[async_trait]
impl AgentSession for LocalSession {
    async fn send_input(&self, content: String, attachments: Vec<Attachment>) -> Result<()> {
        if self
            .shared
            .turn_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(RuntimeError::Protocol(
                "the model is still answering — interrupt it first".into(),
            ));
        }
        self.shared.cancelled.store(false, Ordering::SeqCst);

        // Attachments become visible prompt text. Nothing implicit is sent: a local
        // endpoint feels safe enough that including scrollback would be tempting,
        // and it is exactly as forbidden here as anywhere else.
        let mut turn = String::new();
        let mut kinds = Vec::new();
        for attachment in &attachments {
            kinds.push(attachment.describe());
            if let Some(text) = attachment.to_prompt_text() {
                turn.push_str(&text);
                turn.push_str("\n\n");
            }
        }
        turn.push_str(&content);

        let mut events = Vec::new();
        if !kinds.is_empty() {
            events.push(self.shared.event(
                format!("Attached {} item(s)", kinds.len()),
                EventPayload::ContextAttached {
                    description: kinds.join(", "),
                    kinds,
                },
            ));
        }
        events.push(self.shared.event(
            first_line(&content, 120),
            EventPayload::UserPrompted { text: content },
        ));
        self.shared.emit(events);

        self.shared.history.lock().push(ChatMessage {
            role: "user".into(),
            content: turn,
        });
        self.shared.transition(ThreadState::Understanding);

        let shared = self.shared.clone();
        let client = self.client.clone();
        let url = self.url.clone();
        let api_key = self.api_key.clone();

        // Answered off-thread so the call returns as soon as the turn has started.
        tokio::spawn(async move {
            // Raced against cancellation so an interrupt lands even while waiting for
            // the first byte, not only between chunks.
            let result = tokio::select! {
                result = stream_turn(&client, &url, api_key.as_deref(), &shared) => result,
                _ = shared.cancel.notified() => {
                    Err(RuntimeError::Protocol("cancelled".into()))
                }
            };
            shared.turn_active.store(false, Ordering::SeqCst);

            match result {
                Ok(()) => {}
                Err(e) if shared.cancelled.load(Ordering::SeqCst) => {
                    // Interrupting is not a failure.
                    let _ = e;
                    *shared.state.lock() = ThreadState::Interrupted;
                    shared.emit(vec![shared.event(
                        "Interrupted",
                        EventPayload::ThreadFailed {
                            reason: "Stopped from Tervin.".into(),
                            recoverable: Some(true),
                        },
                    )]);
                }
                Err(e) => {
                    shared.note(tervin_core::events::Severity::Error, e.to_string());
                    *shared.state.lock() = ThreadState::Failed;
                    shared.emit(vec![shared.event(
                        format!("Failed: {e}"),
                        EventPayload::ThreadFailed {
                            reason: e.to_string(),
                            recoverable: Some(true),
                        },
                    )]);
                }
            }
        });

        Ok(())
    }

    async fn interrupt(&self) -> Result<()> {
        // The flag classifies what happened; the notify actually stops it. Dropping
        // the response closes the connection, which is the only way to stop a server
        // that is mid-generation.
        self.shared.cancelled.store(true, Ordering::SeqCst);
        // `notify_one` rather than `notify_waiters`: it stores a permit, so an
        // interrupt that arrives before the turn starts waiting is not lost.
        self.shared.cancel.notify_one();
        Ok(())
    }

    async fn set_permission_mode(&self, _mode: &str) -> Result<()> {
        Err(RuntimeError::Unsupported {
            runtime: self.shared.identity.runtime_id.clone(),
            feature: "permission modes (there is nothing for this runtime to permit)".into(),
        })
    }

    fn session_metadata(&self) -> SessionMetadata {
        SessionMetadata {
            resume_id: None,
            model: Some(self.shared.model.clone()),
            permission_mode: None,
            runtime_version: None,
            tools: Vec::new(),
            mcp_servers: Vec::new(),
            slash_commands: Vec::new(),
            hook_runs: Vec::new(),
            modes: Vec::<SessionMode>::new(),
            instruction_sources: Vec::new(),
            cwd: Some(self.shared.cwd.clone()),
        }
    }

    fn permissions(&self) -> PermissionState {
        PermissionState {
            mode: "none".into(),
            // Not a gate, and not a gap: there is no action to intercept.
            tervin_can_intercept: false,
            explanation: "This runtime only answers — it cannot run commands or change \
                          files, so there is nothing to approve."
                .into(),
            denials: Vec::new(),
        }
    }

    fn diagnostics(&self) -> Vec<RuntimeDiagnostic> {
        self.shared.diagnostics.lock().clone()
    }

    fn capabilities(&self) -> Capabilities {
        LocalModelRuntime::static_capabilities()
    }

    fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    async fn shutdown(&self) -> Result<()> {
        self.shared.cancelled.store(true, Ordering::SeqCst);
        self.shared.cancel.notify_one();
        self.shared.running.store(false, Ordering::SeqCst);
        Ok(())
    }
}

/// Send the conversation and stream the reply.
async fn stream_turn(
    client: &reqwest::Client,
    url: &str,
    api_key: Option<&str>,
    shared: &Arc<Shared>,
) -> Result<()> {
    let body = json!({
        "model": shared.model,
        "messages": shared.history.lock().clone(),
        "stream": true,
        // Ask for token counts. Servers that do not support it ignore the field.
        "stream_options": { "include_usage": true },
    });

    let mut request = client.post(url).json(&body);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }

    let response = request
        .send()
        .await
        .map_err(|e| RuntimeError::Protocol(describe_transport_error(&e, url)))?;

    if !response.status().is_success() {
        let status = response.status();
        // The body usually says what was actually wrong — a missing model, an
        // unsupported field — and a bare status code would waste that.
        let detail = response.text().await.unwrap_or_default();
        return Err(RuntimeError::Protocol(format!(
            "{status}{}",
            first_line(&detail, 300)
                .is_empty()
                .then(String::new)
                .unwrap_or_else(|| format!(": {}", first_line(&detail, 300)))
        )));
    }

    shared.transition(ThreadState::Understanding);

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut reply = String::new();
    let mut truncated = false;

    while let Some(chunk) = stream.next().await {
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(RuntimeError::Protocol("cancelled".into()));
        }
        let chunk = chunk.map_err(|e| RuntimeError::Protocol(e.to_string()))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Server-sent events: one event per blank-line-terminated block, but every
        // implementation here emits exactly one `data:` line per event, so splitting
        // on newlines is enough and avoids buffering a whole event.
        while let Some(newline) = buffer.find('\n') {
            let line = buffer[..newline].trim().to_string();
            buffer.drain(..=newline);
            if line.is_empty() {
                continue;
            }
            let Some(payload) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(payload) else {
                continue;
            };

            if let Some(usage) = value.get("usage").filter(|u| !u.is_null()) {
                let mut cost = shared.cost.lock();
                cost.input_tokens = usage.get("prompt_tokens").and_then(Value::as_u64);
                cost.output_tokens = usage.get("completion_tokens").and_then(Value::as_u64);
                cost.model = Some(shared.model.clone());
                // No `total_cost_usd`: a model on your own machine has no price, and
                // inventing one would be worse than leaving it blank.
            }

            if let Some(delta) = value
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("delta"))
                .and_then(|d| d.get("content"))
                .and_then(Value::as_str)
            {
                if reply.len() + delta.len() > MAX_REPLY_BYTES {
                    truncated = true;
                } else {
                    reply.push_str(delta);
                }
            }
        }
    }

    if truncated {
        shared.note(
            tervin_core::events::Severity::Warning,
            format!(
                "The reply exceeded {} MB and was truncated.",
                MAX_REPLY_BYTES / (1024 * 1024)
            ),
        );
    }

    let trimmed = reply.trim().to_string();
    if !trimmed.is_empty() {
        shared.history.lock().push(ChatMessage {
            role: "assistant".into(),
            content: trimmed.clone(),
        });
        shared.emit(vec![shared.event(
            first_line(&trimmed, 120),
            EventPayload::AgentMessage {
                text: trimmed,
                is_reasoning: false,
                parent_tool_use_id: None,
            },
        )]);
    }

    let cost = shared.cost.lock().clone();
    *shared.state.lock() = ThreadState::AwaitingInput;
    shared.emit(vec![
        shared.event("Answered", EventPayload::CostUpdated { snapshot: cost }),
        shared.event(
            ThreadState::AwaitingInput.label(),
            EventPayload::ThreadState {
                state: ThreadState::AwaitingInput,
            },
        ),
    ]);
    Ok(())
}

/// Turn a transport failure into something a user can act on.
fn describe_transport_error(error: &reqwest::Error, url: &str) -> String {
    if error.is_connect() {
        return format!(
            "Nothing is answering at {url}. Start the server, or change its address in \
             Settings."
        );
    }
    if error.is_timeout() {
        return format!("{url} did not respond in time.");
    }
    error.to_string()
}

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if line.chars().count() <= max {
        line.trim().to_string()
    } else {
        format!("{}…", line.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_endpoint_never_claims_it_can_act() {
        // The whole reason for a separate tier. Every ability that would imply
        // acting has to be refused, with a reason.
        let caps = LocalModelRuntime::static_capabilities();
        assert_eq!(caps.tier, Tier::Conversational);
        for (name, level) in [
            ("plan_mode", &caps.plan_mode),
            ("tool_events", &caps.tool_events),
            ("file_edits", &caps.file_edits),
            ("native_permission_bridge", &caps.native_permission_bridge),
            ("hooks", &caps.hooks),
            ("mcp", &caps.mcp),
        ] {
            match level {
                CapabilityLevel::Unsupported { reason } => assert!(!reason.is_empty(), "{name}"),
                other => panic!("{name} must be refused, was {other:?}"),
            }
        }
        // And the two things it is actually for.
        assert!(matches!(caps.model_selection, CapabilityLevel::Supported));
        assert!(matches!(caps.multi_turn, CapabilityLevel::Supported));
    }

    #[test]
    fn the_conversational_tier_is_not_ranked_below_a_terminal() {
        // Numbering it 4 would read as "worse than Tier 3", which it is not — it is
        // a different kind of thing.
        assert_eq!(Tier::Conversational.number(), 0);
        assert!(Tier::Conversational.label().contains("cannot act"));
    }

    #[test]
    fn a_base_url_is_accepted_the_way_a_person_writes_it() {
        for input in [
            "http://127.0.0.1:1234",
            "http://127.0.0.1:1234/",
            "http://127.0.0.1:1234/v1",
            "127.0.0.1:1234",
        ] {
            assert_eq!(
                normalise_base_url(input),
                "http://127.0.0.1:1234/v1",
                "input was {input}"
            );
        }
        // An explicit scheme is never rewritten.
        assert_eq!(
            normalise_base_url("https://models.example.com/v1"),
            "https://models.example.com/v1"
        );
    }

    #[test]
    fn model_lists_parse_in_every_shape_a_server_returns() {
        assert_eq!(
            parse_model_list(&json!({"data":[{"id":"qwen3-8b"},{"id":"llama-3.3"}]})),
            vec!["qwen3-8b", "llama-3.3"]
        );
        // Ollama-style, and a bare array, both rather than reporting none.
        assert_eq!(
            parse_model_list(&json!([{"name":"mistral"}])),
            vec!["mistral"]
        );
        assert_eq!(parse_model_list(&json!({"data":[]})), Vec::<String>::new());
        assert_eq!(parse_model_list(&json!({})), Vec::<String>::new());
    }

    #[test]
    fn the_known_endpoints_use_their_documented_ports() {
        let endpoints = known_local_endpoints();
        let by_id = |id: &str| {
            endpoints
                .iter()
                .find(|e| e.runtime_id == id)
                .expect(id)
                .base_url
                .clone()
        };
        assert_eq!(by_id("lmstudio"), "http://127.0.0.1:1234/v1");
        assert_eq!(by_id("ollama"), "http://127.0.0.1:11434/v1");
        assert_eq!(by_id("vllm"), "http://127.0.0.1:8000/v1");
        assert_eq!(by_id("llamacpp"), "http://127.0.0.1:8080/v1");
        for endpoint in &endpoints {
            assert!(!endpoint.install_hint.is_empty(), "{}", endpoint.runtime_id);
        }
    }

    #[tokio::test]
    async fn an_endpoint_that_is_not_running_is_reported_without_failing() {
        // Discovery runs at startup for every configured endpoint; none of them
        // being up is the normal case and must be quiet and fast.
        let rt = LocalModelRuntime::custom("test", "Test", "http://127.0.0.1:1");
        let started = std::time::Instant::now();
        let discovery = rt.discover().await;
        assert!(!discovery.available);
        assert!(discovery
            .notes
            .iter()
            .any(|n| n.contains("Nothing is answering")));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "probing a dead endpoint took {:?}",
            started.elapsed()
        );
    }
}
