//! Turning `codex exec --json` output into Tervin events.
//!
//! ## What this was built against
//!
//! Not documentation. `codex exec --json` was run against codex-cli **0.146.0** and its
//! stdout captured, which established the envelope:
//!
//! ```text
//! {"type":"thread.started","thread_id":"019fc392-d9b6-7731-abd6-7e6d5804184d"}
//! {"type":"turn.started"}
//! {"type":"error","message":"…"}
//! {"type":"item.completed","item":{"id":"item_0","type":"error","message":"…"}}
//! {"type":"turn.failed","error":{"message":"…"}}
//! ```
//!
//! and one thing that matters more than it looks: **stdout is pure JSONL and the tracing
//! logs go to stderr.** A reader that merged them would try to parse `2026-08-02T17:43:29
//! ERROR codex_api…` as an event on every retry.
//!
//! The item vocabulary comes from `codex app-server generate-json-schema`, which the
//! binary produces itself — so the field names below are the ones Codex serialises rather
//! than names guessed from a blog post.
//!
//! ## What could not be verified, and what that changes
//!
//! Driving a real turn needs OpenAI credentials, so the success path — an agent message,
//! a command execution — was never observed live. The schema gives the item shapes, but
//! the schema is the *app-server* protocol, which uses `camelCase`, while the `exec`
//! envelope above uses `snake_case`. Whether `exec` spells an item `commandExecution` or
//! `command_execution` is genuinely ambiguous: **both strings are present in the binary.**
//!
//! Rather than pick one and hope, every field carries a `serde(alias)` for the other
//! spelling. It costs nothing, cannot be wrong, and removes the guess entirely.
//!
//! Anything not recognised becomes `runtime.unclassified` — kept and shown as
//! unrecognised, never dropped and never guessed at, which is the same rule the other
//! adapters follow.

use serde::Deserialize;
use tervin_core::events::{FileChange, FileChangeKind, OutputStream, Severity};
use tervin_core::{AgentIdentity, EventPayload, TervinEvent, ThreadState};

/// Longest text kept on a single event.
const MAX_TEXT: usize = 16 * 1024;

/// State carried across lines, since an event alone does not say what turn it belongs to.
pub struct CodexNormalizer {
    agent: AgentIdentity,
    cwd: String,
    project: Option<String>,
    /// The thread id Codex reported, which is also its resume handle.
    thread_id: Option<String>,
    /// Lines that were not JSON, or were JSON of a shape this build cannot use.
    unrecognised: usize,
}

impl CodexNormalizer {
    pub fn new(agent: AgentIdentity, cwd: impl Into<String>, project: Option<String>) -> Self {
        Self {
            agent,
            cwd: cwd.into(),
            project,
            thread_id: None,
            unrecognised: 0,
        }
    }

    /// Codex's own session id, once it has reported one. This is what `codex exec resume`
    /// takes, so it doubles as the resume handle.
    pub fn session_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    pub fn unrecognised(&self) -> usize {
        self.unrecognised
    }

    fn event(&self, summary: impl Into<String>, payload: EventPayload) -> TervinEvent {
        let mut event = TervinEvent::new(self.agent.clone(), summary, payload);
        event.project = self.project.clone();
        event.cwd = Some(self.cwd.clone());
        event
    }

    /// Interpret one line of stdout.
    ///
    /// A blank line yields nothing. A line that is not JSON is counted and reported as
    /// unclassified rather than dropped: a silent parser is how a protocol change becomes
    /// a Thread that looks like it did nothing.
    pub fn line(&mut self, line: &str) -> Vec<TervinEvent> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let parsed: Envelope = match serde_json::from_str(trimmed) {
            Ok(parsed) => parsed,
            Err(_) => {
                self.unrecognised += 1;
                return vec![self.unclassified("not json", trimmed)];
            }
        };

        match parsed {
            Envelope::ThreadStarted { thread_id } => {
                self.thread_id = Some(thread_id.clone());
                vec![self.event(
                    "Session started · codex".to_string(),
                    EventPayload::ThreadStarted {
                        tier: tervin_core::Tier::Structured,
                        task_title: None,
                        // Codex resumes by thread id, so this really is a resume handle.
                        resume_id: Some(thread_id),
                    },
                )]
            }

            Envelope::TurnStarted => vec![self.event(
                "Working".to_string(),
                EventPayload::ThreadState {
                    state: ThreadState::Understanding,
                },
            )],

            Envelope::TurnCompleted { usage } => {
                let mut out = Vec::new();
                // Only when Codex actually reported usage. A cost event with zeroes reads
                // as "this turn was free", which is a different claim from "not reported".
                if let Some(usage) = usage {
                    out.push(self.event(
                        format!(
                            "{} in, {} out",
                            usage.input_tokens.unwrap_or(0),
                            usage.output_tokens.unwrap_or(0)
                        ),
                        EventPayload::CostUpdated {
                            snapshot: tervin_core::events::CostSnapshot {
                                input_tokens: usage.input_tokens,
                                output_tokens: usage.output_tokens,
                                cache_read_tokens: usage.cached_input_tokens,
                                cache_write_tokens: None,
                                // Codex reports tokens, not money. Deriving a figure from
                                // a price list that changes would be worse than none.
                                total_cost_usd: None,
                                context_window: None,
                                context_used: None,
                                // Codex reports the model on the turn, not on usage.
                                model: None,
                            },
                        },
                    ));
                }
                out.push(self.event(
                    "Turn finished".to_string(),
                    EventPayload::ThreadCompleted {
                        result: None,
                        duration_ms: None,
                        cost: None,
                    },
                ));
                out
            }

            Envelope::TurnFailed { error } => {
                let reason = error.map(|e| e.message).unwrap_or_else(|| {
                    "The turn failed and the runtime gave no reason.".to_string()
                });
                vec![self.event(
                    format!("Failed: {}", first_line(&reason, 120)),
                    EventPayload::ThreadFailed {
                        reason,
                        recoverable: None,
                    },
                )]
            }

            // A retry notice, not necessarily fatal — Codex emits several of these while
            // reconnecting and then carries on. Reported as a diagnostic rather than as a
            // failed Thread, because marking the Thread failed on the first one would be
            // wrong four times out of five.
            Envelope::Error { message } => vec![self.event(
                first_line(&message, 120),
                EventPayload::DiagnosticDetected {
                    diagnostic_id: tervin_core::DiagnosticId::new(),
                    severity: Severity::Error,
                    message: clamp(&message),
                    path: None,
                    line: None,
                    source: Some("codex".to_string()),
                },
            )],

            Envelope::ItemStarted { item } => self.item(item, false),
            Envelope::ItemCompleted { item } => self.item(item, true),

            Envelope::Unknown(value) => {
                let kind = value
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no type")
                    .to_string();
                self.unrecognised += 1;
                vec![self.unclassified(&kind, &value.to_string())]
            }
        }
    }

    /// One thread item.
    ///
    /// `completed` distinguishes the two notifications: a command that has only started
    /// has no exit code yet, and reporting one would be inventing it.
    fn item(&mut self, item: Item, completed: bool) -> Vec<TervinEvent> {
        match item {
            Item::AgentMessage { text, .. } if !text.trim().is_empty() => {
                // Only on completion: Codex sends the same item started and completed, and
                // recording both would double every message in the timeline and in prompt
                // history.
                if !completed {
                    return Vec::new();
                }
                vec![self.event(
                    first_line(&text, 120),
                    EventPayload::AgentMessage {
                        text: clamp(&text),
                        is_reasoning: false,
                        parent_tool_use_id: None,
                    },
                )]
            }

            Item::Reasoning { summary, .. } if completed => {
                let text = summary.join("\n");
                if text.trim().is_empty() {
                    return Vec::new();
                }
                vec![self.event(
                    format!("Thinking: {}", first_line(&text, 100)),
                    EventPayload::AgentMessage {
                        text: clamp(&text),
                        // Marked, which keeps it out of prompt search where it would bury
                        // what the person actually wrote.
                        is_reasoning: true,
                        parent_tool_use_id: None,
                    },
                )]
            }

            Item::Plan { text, .. } if completed && !text.trim().is_empty() => vec![self.event(
                format!("Plan: {}", first_line(&text, 100)),
                EventPayload::PlanProposed {
                    steps: text
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(|l| tervin_core::events::PlanStep {
                            description: l.to_string(),
                            touches: Vec::new(),
                        })
                        .collect(),
                    // The plan as Codex wrote it, so nothing is lost to the line split.
                    raw_text: Some(clamp(&text)),
                },
            )],

            Item::CommandExecution {
                command,
                cwd,
                status,
                exit_code,
                duration_ms,
                aggregated_output,
                ..
            } => {
                let mut out = Vec::new();
                if !completed {
                    out.push(self.event(
                        format!("$ {}", first_line(&command, 120)),
                        EventPayload::CommandStarted {
                            command: command.clone(),
                            block_id: None,
                        },
                    ));
                    // The command may run somewhere other than where the Thread started,
                    // and every later event should read as coming from there.
                    if let Some(cwd) = cwd {
                        if !cwd.trim().is_empty() {
                            self.cwd = cwd;
                        }
                    }
                    return out;
                }

                if let Some(output) = aggregated_output.as_deref() {
                    if !output.trim().is_empty() {
                        out.push(self.event(
                            first_line(output, 120),
                            EventPayload::CommandOutput {
                                // Codex aggregates the two streams, so claiming stderr for
                                // part of it would be a guess.
                                stream: OutputStream::Stdout,
                                excerpt: clamp(output),
                                block_id: None,
                            },
                        ));
                    }
                }

                // Codex declines a command the sandbox or the user refused. That is not a
                // failure of the command — it never ran.
                if status == CommandStatus::Declined {
                    out.push(self.event(
                        format!("$ {} — declined", first_line(&command, 100)),
                        EventPayload::PermissionDenied {
                            request_id: None,
                            action: command,
                            // Codex's own sandbox or approval policy refused it, not
                            // Tervin. Saying otherwise would credit a gate that never
                            // fired, which is the exact claim Tervin must not make.
                            authority: tervin_core::events::DecisionAuthority::ProviderNative,
                            reason: Some("Codex declined to run it.".to_string()),
                        },
                    ));
                    return out;
                }

                // `exitCode` is nullable in Codex's own schema, so a real status is
                // reported when there is one and never fabricated when there is not.
                let reported = exit_code.is_some();
                let code = exit_code.unwrap_or(if status == CommandStatus::Failed {
                    1
                } else {
                    0
                });
                out.push(self.event(
                    format!(
                        "$ {} — {}",
                        first_line(&command, 80),
                        match exit_code {
                            Some(code) => format!("exit {code}"),
                            None if status == CommandStatus::Failed =>
                                "reported as failed (no exit status)".to_string(),
                            None => "reported as succeeded (no exit status)".to_string(),
                        }
                    ),
                    EventPayload::CommandCompleted {
                        command,
                        exit_code: code,
                        duration_ms: duration_ms.unwrap_or(0),
                        exit_code_reported: reported,
                        block_id: None,
                    },
                ));
                out
            }

            Item::FileChange { changes, .. } if completed => changes
                .into_iter()
                .filter(|c| !c.path.trim().is_empty())
                .map(|change| {
                    let path = change.path.clone();
                    self.event(
                        format!("Changed {path}"),
                        EventPayload::FileChanged {
                            change: FileChange {
                                path,
                                kind: change.kind.into(),
                                added_lines: None,
                                removed_lines: None,
                            },
                        },
                    )
                })
                .collect(),

            Item::McpToolCall {
                server,
                tool,
                status,
                ..
            } if completed => vec![self.event(
                format!("{server}/{tool}"),
                EventPayload::ToolCompleted {
                    tool_use_id: String::new(),
                    tool_name: format!("{server}/{tool}"),
                    is_error: status == CommandStatus::Failed,
                    output_summary: String::new(),
                    duration_ms: None,
                },
            )],

            Item::WebSearch { query, .. } if completed => vec![self.event(
                format!("Searched the web: {}", first_line(&query, 90)),
                EventPayload::ToolCompleted {
                    tool_use_id: String::new(),
                    tool_name: "web_search".to_string(),
                    is_error: false,
                    output_summary: query,
                    duration_ms: None,
                },
            )],

            Item::Error { message } => vec![self.event(
                first_line(&message, 120),
                EventPayload::DiagnosticDetected {
                    diagnostic_id: tervin_core::DiagnosticId::new(),
                    severity: Severity::Error,
                    message: clamp(&message),
                    path: None,
                    line: None,
                    source: Some("codex".to_string()),
                },
            )],

            // A modelled item with nothing to say — an empty message, or a `started`
            // notification for a kind only reported on completion.
            Item::AgentMessage { .. }
            | Item::Reasoning { .. }
            | Item::Plan { .. }
            | Item::FileChange { .. }
            | Item::McpToolCall { .. }
            | Item::WebSearch { .. } => Vec::new(),

            Item::Unknown(value) => {
                let kind = value
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no type")
                    .to_string();
                self.unrecognised += 1;
                vec![self.unclassified(&format!("item {kind}"), &value.to_string())]
            }
        }
    }

    /// Kept rather than dropped, and shown as unrecognised rather than guessed at.
    fn unclassified(&self, kind: &str, raw: &str) -> TervinEvent {
        self.event(
            format!("Unrecognised from codex: {kind}"),
            EventPayload::RuntimeUnclassified {
                source_type: format!("codex/{kind} · {}", first_line(raw, 200)),
            },
        )
    }
}

// ------------------------------------------------------------------ wire types

/// The `exec --json` envelope, as captured from codex-cli 0.146.0.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Envelope {
    #[serde(rename = "thread.started")]
    ThreadStarted {
        #[serde(alias = "threadId")]
        thread_id: String,
    },
    #[serde(rename = "turn.started")]
    TurnStarted,
    #[serde(rename = "turn.completed")]
    TurnCompleted {
        #[serde(default)]
        usage: Option<Usage>,
    },
    #[serde(rename = "turn.failed")]
    TurnFailed {
        #[serde(default)]
        error: Option<ErrorBody>,
    },
    #[serde(rename = "item.started")]
    ItemStarted { item: Item },
    #[serde(rename = "item.completed")]
    ItemCompleted { item: Item },
    /// A bare error line. Codex emits several while reconnecting.
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        message: String,
    },
    /// Anything else. `untagged` last, so it only catches what the variants above miss.
    #[serde(untagged)]
    Unknown(serde_json::Value),
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default, alias = "inputTokens")]
    input_tokens: Option<u64>,
    #[serde(default, alias = "outputTokens")]
    output_tokens: Option<u64>,
    #[serde(default, alias = "cachedInputTokens")]
    cached_input_tokens: Option<u64>,
}

/// A thread item.
///
/// Field names come from `codex app-server generate-json-schema`. Every one carries an
/// alias for the other casing, because the app-server protocol is camelCase while the
/// `exec` envelope is snake_case and both spellings appear in the binary — so which one
/// `exec` uses is not something to guess at.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Item {
    #[serde(rename = "agentMessage", alias = "agent_message")]
    AgentMessage {
        #[serde(default)]
        text: String,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default)]
        summary: Vec<String>,
    },
    #[serde(rename = "plan", alias = "todoList", alias = "todo_list")]
    Plan {
        #[serde(default)]
        text: String,
    },
    #[serde(rename = "commandExecution", alias = "command_execution")]
    CommandExecution {
        #[serde(default)]
        command: String,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        status: CommandStatus,
        #[serde(default, alias = "exitCode")]
        exit_code: Option<i32>,
        #[serde(default, alias = "durationMs")]
        duration_ms: Option<u64>,
        #[serde(default, alias = "aggregatedOutput")]
        aggregated_output: Option<String>,
    },
    #[serde(rename = "fileChange", alias = "file_change")]
    FileChange {
        #[serde(default)]
        changes: Vec<Change>,
    },
    #[serde(rename = "mcpToolCall", alias = "mcp_tool_call")]
    McpToolCall {
        #[serde(default)]
        server: String,
        #[serde(default)]
        tool: String,
        #[serde(default)]
        status: CommandStatus,
    },
    #[serde(rename = "webSearch", alias = "web_search")]
    WebSearch {
        #[serde(default)]
        query: String,
    },
    /// Seen in the captured output: `{"id":"item_0","type":"error","message":"…"}`.
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        message: String,
    },
    #[serde(untagged)]
    Unknown(serde_json::Value),
}

#[derive(Debug, Deserialize, Default, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum CommandStatus {
    #[serde(alias = "in_progress")]
    InProgress,
    Completed,
    Failed,
    /// The sandbox or the user refused it. The command never ran, which is not the same
    /// as failing.
    Declined,
    #[serde(other)]
    #[default]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct Change {
    #[serde(default)]
    path: String,
    #[serde(default)]
    kind: ChangeKind,
}

#[derive(Debug, Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum ChangeKind {
    Add,
    #[default]
    Update,
    Delete,
    #[serde(other)]
    Other,
}

impl From<ChangeKind> for FileChangeKind {
    fn from(kind: ChangeKind) -> Self {
        match kind {
            ChangeKind::Add => Self::Created,
            ChangeKind::Delete => Self::Deleted,
            // An unrecognised kind is reported as a modification: something changed, and
            // that much is true whatever the label.
            ChangeKind::Update | ChangeKind::Other => Self::Modified,
        }
    }
}

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let trimmed = line.trim();
    let clipped: String = trimmed.chars().take(max).collect();
    if clipped.chars().count() < trimmed.chars().count() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

fn clamp(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= MAX_TEXT {
        return trimmed.to_string();
    }
    let mut end = MAX_TEXT;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… truncated", &trimmed[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tervin_core::Tier;

    /// The four lines `codex exec --json` actually wrote, copied from a captured run of
    /// codex-cli 0.146.0 rather than composed from the shape this parser expects.
    const REAL_THREAD_STARTED: &str =
        r#"{"type":"thread.started","thread_id":"019fc392-d9b6-7731-abd6-7e6d5804184d"}"#;
    const REAL_TURN_STARTED: &str = r#"{"type":"turn.started"}"#;
    const REAL_ERROR: &str = r#"{"type":"error","message":"Reconnecting... 2/5 (unexpected status 401 Unauthorized: Missing bearer or basic authentication in header, url: wss://api.openai.com/v1/responses)"}"#;
    const REAL_ITEM_ERROR: &str = r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Falling back from WebSockets to HTTPS transport."}}"#;
    const REAL_TURN_FAILED: &str =
        r#"{"type":"turn.failed","error":{"message":"unexpected status 401 Unauthorized"}}"#;

    fn normalizer() -> CodexNormalizer {
        CodexNormalizer::new(
            AgentIdentity::new("codex", "Codex", Tier::Structured),
            "/proj",
            Some("proj".to_string()),
        )
    }

    fn kinds(events: &[TervinEvent]) -> Vec<&'static str> {
        events.iter().map(|e| e.payload.kind()).collect()
    }

    #[test]
    fn reads_the_real_captured_session() {
        let mut n = normalizer();

        let started = n.line(REAL_THREAD_STARTED);
        assert_eq!(kinds(&started), vec!["thread.started"]);
        // The thread id is Codex's resume handle, which is what `codex exec resume` takes.
        assert_eq!(n.session_id(), Some("019fc392-d9b6-7731-abd6-7e6d5804184d"));
        match &started[0].payload {
            EventPayload::ThreadStarted {
                tier, resume_id, ..
            } => {
                assert_eq!(*tier, Tier::Structured);
                assert_eq!(
                    resume_id.as_deref(),
                    Some("019fc392-d9b6-7731-abd6-7e6d5804184d")
                );
            }
            other => panic!("expected thread.started, got {other:?}"),
        }

        assert_eq!(kinds(&n.line(REAL_TURN_STARTED)), vec!["thread.state"]);

        // A reconnect notice is a diagnostic, not a dead Thread. Codex emits several and
        // then carries on, so failing the Thread on the first would be wrong most times.
        assert_eq!(kinds(&n.line(REAL_ERROR)), vec!["diagnostic.detected"]);
        assert_eq!(kinds(&n.line(REAL_ITEM_ERROR)), vec!["diagnostic.detected"]);

        // This one really is the end.
        let failed = n.line(REAL_TURN_FAILED);
        assert_eq!(kinds(&failed), vec!["thread.failed"]);
        match &failed[0].payload {
            EventPayload::ThreadFailed { reason, .. } => assert!(reason.contains("401")),
            other => panic!("expected thread.failed, got {other:?}"),
        }

        // Every line was understood.
        assert_eq!(n.unrecognised(), 0);
    }

    #[test]
    fn a_tracing_log_line_is_reported_rather_than_silently_dropped() {
        // Logs go to stderr, so this should not happen — but if a caller ever merges the
        // streams, the failure has to be visible rather than a Thread that did nothing.
        let mut n = normalizer();
        let events =
            n.line("2026-08-02T17:43:29.751696Z ERROR codex_api::endpoint: failed to connect");
        assert_eq!(kinds(&events), vec!["runtime.unclassified"]);
        assert_eq!(n.unrecognised(), 1);
    }

    #[test]
    fn a_blank_line_is_nothing_and_is_not_counted_as_broken() {
        let mut n = normalizer();
        assert!(n.line("").is_empty());
        assert!(n.line("   \n").is_empty());
        assert_eq!(n.unrecognised(), 0);
    }

    #[test]
    fn an_agent_message_is_recorded_once_on_completion() {
        // Codex reports the same item started and completed. Recording both would double
        // every message in the timeline and in prompt history.
        let mut n = normalizer();
        let item = r#"{"item":{"id":"i1","type":"agentMessage","text":"I will run the tests."}}"#;
        let started = format!("{{\"type\":\"item.started\",{}", &item[1..]);
        let completed = format!("{{\"type\":\"item.completed\",{}", &item[1..]);

        assert!(n.line(&started).is_empty());
        let events = n.line(&completed);
        assert_eq!(kinds(&events), vec!["agent.message"]);
        match &events[0].payload {
            EventPayload::AgentMessage {
                text, is_reasoning, ..
            } => {
                assert_eq!(text, "I will run the tests.");
                assert!(!is_reasoning);
            }
            other => panic!("expected agent.message, got {other:?}"),
        }
    }

    #[test]
    fn either_casing_of_an_item_type_is_accepted() {
        // The `exec` envelope is snake_case and the app-server schema is camelCase, and
        // both spellings are present in the binary. Accepting either removes the guess.
        for spelling in ["agentMessage", "agent_message"] {
            let mut n = normalizer();
            let line = format!(
                r#"{{"type":"item.completed","item":{{"id":"i1","type":"{spelling}","text":"hello"}}}}"#
            );
            assert_eq!(kinds(&n.line(&line)), vec!["agent.message"], "{spelling}");
            assert_eq!(n.unrecognised(), 0, "{spelling}");
        }
    }

    #[test]
    fn either_casing_of_a_field_is_accepted() {
        for line in [
            r#"{"type":"item.completed","item":{"id":"i1","type":"commandExecution","command":"cargo test","cwd":"/proj","status":"completed","exitCode":0,"durationMs":1500}}"#,
            r#"{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"cargo test","cwd":"/proj","status":"completed","exit_code":0,"duration_ms":1500}}"#,
        ] {
            let mut n = normalizer();
            let events = n.line(line);
            match events.iter().find_map(|e| match &e.payload {
                EventPayload::CommandCompleted {
                    exit_code,
                    duration_ms,
                    exit_code_reported,
                    ..
                } => Some((*exit_code, *duration_ms, *exit_code_reported)),
                _ => None,
            }) {
                Some((code, duration, reported)) => {
                    assert_eq!(code, 0);
                    assert_eq!(duration, 1500);
                    assert!(reported, "a real exit code was not marked as reported");
                }
                None => panic!("no completion parsed from {line}"),
            }
        }
    }

    #[test]
    fn a_command_reports_its_exit_code_when_codex_gives_one() {
        let mut n = normalizer();
        let start = r#"{"type":"item.started","item":{"id":"c1","type":"commandExecution","command":"cargo test","cwd":"/proj/ui","status":"inProgress"}}"#;
        assert_eq!(kinds(&n.line(start)), vec!["command.started"]);
        // The command's own directory becomes the context for later events: an agent can
        // run something somewhere other than where the Thread started.
        assert_eq!(
            n.line(REAL_TURN_STARTED)[0].cwd.as_deref(),
            Some("/proj/ui")
        );

        let done = r#"{"type":"item.completed","item":{"id":"c1","type":"commandExecution","command":"cargo test","cwd":"/proj/ui","status":"failed","exitCode":101,"aggregatedOutput":"test result: FAILED. 2 failed"}}"#;
        let events = n.line(done);
        assert_eq!(kinds(&events), vec!["command.output", "command.completed"]);
        assert!(events[0].summary.contains("FAILED"));
        assert!(events[1].summary.contains("exit 101"));
    }

    #[test]
    fn a_command_with_no_exit_code_keeps_the_outcome_and_admits_the_gap() {
        // `exitCode` is nullable in Codex's own schema. Inventing 1 would put a number in
        // the Block that nothing reported.
        let mut n = normalizer();
        let line = r#"{"type":"item.completed","item":{"id":"c1","type":"commandExecution","command":"flaky","cwd":"/proj","status":"failed"}}"#;
        let events = n.line(line);

        match &events[0].payload {
            EventPayload::CommandCompleted {
                exit_code,
                exit_code_reported,
                ..
            } => {
                assert!(!exit_code_reported, "a derived code was marked as reported");
                // Still 1 on the event for callers that want a number, but flagged — the
                // Block drops it and shows the status instead.
                assert_eq!(*exit_code, 1);
            }
            other => panic!("expected command.completed, got {other:?}"),
        }
        assert!(events[0].summary.contains("no exit status"));
    }

    #[test]
    fn a_declined_command_is_a_refusal_and_not_a_failure() {
        // Codex's sandbox or approval policy refused it, so it never ran. Reporting a
        // failed command would say the opposite of what happened.
        let mut n = normalizer();
        let line = r#"{"type":"item.completed","item":{"id":"c1","type":"commandExecution","command":"rm -rf /","cwd":"/proj","status":"declined"}}"#;
        let events = n.line(line);

        assert_eq!(kinds(&events), vec!["permission.denied"]);
        match &events[0].payload {
            EventPayload::PermissionDenied {
                authority, action, ..
            } => {
                assert_eq!(action, "rm -rf /");
                // Credited to Codex, not to Tervin Rules — claiming Tervin's gate fired
                // when it did not would be the worst kind of wrong here.
                assert_eq!(
                    *authority,
                    tervin_core::events::DecisionAuthority::ProviderNative
                );
            }
            other => panic!("expected permission.denied, got {other:?}"),
        }
    }

    #[test]
    fn reasoning_is_marked_so_it_stays_out_of_prompt_search() {
        let mut n = normalizer();
        let line = r#"{"type":"item.completed","item":{"id":"r1","type":"reasoning","summary":["weighing two approaches","the second is simpler"]}}"#;
        let events = n.line(line);

        match &events[0].payload {
            EventPayload::AgentMessage {
                is_reasoning, text, ..
            } => {
                assert!(is_reasoning);
                assert!(text.contains("weighing two approaches"));
            }
            other => panic!("expected agent.message, got {other:?}"),
        }
    }

    #[test]
    fn a_file_change_reaches_review() {
        let mut n = normalizer();
        let line = r#"{"type":"item.completed","item":{"id":"f1","type":"fileChange","status":"completed","changes":[{"path":"/proj/src/main.rs","kind":"update"},{"path":"/proj/new.rs","kind":"add"},{"path":"/proj/old.rs","kind":"delete"}]}}"#;
        let events = n.line(line);

        assert_eq!(events.len(), 3);
        let kinds: Vec<FileChangeKind> = events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::FileChanged { change } => Some(change.kind),
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                FileChangeKind::Modified,
                FileChangeKind::Created,
                FileChangeKind::Deleted
            ]
        );
    }

    #[test]
    fn an_unrecognised_change_kind_is_still_reported_as_a_change() {
        // Something changed, and that much is true whatever the label. Dropping it would
        // hide a file edit from Review.
        let mut n = normalizer();
        let line = r#"{"type":"item.completed","item":{"id":"f1","type":"fileChange","status":"completed","changes":[{"path":"/proj/x.rs","kind":"somethingNew"}]}}"#;
        let events = n.line(line);
        assert_eq!(kinds(&events), vec!["file.changed"]);
    }

    #[test]
    fn usage_is_only_reported_when_codex_reports_it() {
        // A cost event full of zeroes reads as "this turn was free", which is a different
        // claim from "not reported".
        let mut n = normalizer();
        assert_eq!(
            kinds(&n.line(r#"{"type":"turn.completed"}"#)),
            vec!["thread.completed"]
        );

        let mut n = normalizer();
        let with_usage = r#"{"type":"turn.completed","usage":{"input_tokens":1200,"output_tokens":340,"cached_input_tokens":900}}"#;
        let events = n.line(with_usage);
        assert_eq!(kinds(&events), vec!["cost.updated", "thread.completed"]);
        match &events[0].payload {
            EventPayload::CostUpdated { snapshot } => {
                assert_eq!(snapshot.input_tokens, Some(1200));
                assert_eq!(snapshot.cache_read_tokens, Some(900));
                // Codex reports tokens, never money.
                assert_eq!(snapshot.total_cost_usd, None);
            }
            other => panic!("expected cost.updated, got {other:?}"),
        }
    }

    #[test]
    fn an_event_type_this_build_does_not_model_is_kept() {
        // A Codex release adding an event must not make a Thread look like it did nothing.
        let mut n = normalizer();
        let events = n.line(r#"{"type":"turn.compacted","detail":"context compacted"}"#);
        assert_eq!(kinds(&events), vec!["runtime.unclassified"]);
        assert!(events[0].summary.contains("turn.compacted"));
        assert_eq!(n.unrecognised(), 1);
    }

    #[test]
    fn an_item_type_this_build_does_not_model_is_kept() {
        let mut n = normalizer();
        let line = r#"{"type":"item.completed","item":{"id":"x1","type":"imageGeneration","status":"completed"}}"#;
        let events = n.line(line);
        assert_eq!(kinds(&events), vec!["runtime.unclassified"]);
        assert!(events[0].summary.contains("imageGeneration"));
    }

    #[test]
    fn over_long_text_is_truncated_visibly_at_a_character_boundary() {
        let mut n = normalizer();
        let huge = "é".repeat(MAX_TEXT);
        let line = serde_json::json!({
            "type": "item.completed",
            "item": { "id": "i1", "type": "agentMessage", "text": huge }
        })
        .to_string();

        match &n.line(&line)[0].payload {
            EventPayload::AgentMessage { text, .. } => {
                assert!(text.ends_with("… truncated"));
                assert!(!text.contains('\u{FFFD}'), "cut mid-character");
            }
            other => panic!("expected agent.message, got {other:?}"),
        }
    }

    #[test]
    fn every_event_carries_the_project_and_directory() {
        // Blocks and prompt search are scoped by these, and an event without them cannot
        // be found again by project.
        let mut n = normalizer();
        let events = n.line(REAL_THREAD_STARTED);
        assert_eq!(events[0].project.as_deref(), Some("proj"));
        assert_eq!(events[0].cwd.as_deref(), Some("/proj"));
    }
}
