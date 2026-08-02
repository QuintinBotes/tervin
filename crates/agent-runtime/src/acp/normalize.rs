//! Translating ACP session updates into Tervin events.
//!
//! Pure state machine — updates in, events out, no I/O — so it can be tested
//! against recorded protocol traffic rather than an assumed schema.
//!
//! ACP streams text as *chunks*, so the same message arrives as many updates. They
//! are coalesced here rather than emitted individually: a timeline with one row per
//! token is unreadable, and the event stream is meant to be a record of what
//! happened, not a transport artefact.

use super::protocol::{PlanEntry, SessionUpdate, StopReason};
use serde_json::Value;
use tervin_core::events::{
    CostSnapshot, DecisionAuthority, FileChange, FileChangeKind, OutputStream, PlanStep,
};
use tervin_core::{
    AgentIdentity, EventPayload, Link, RiskAssessment, TervinEvent, ThreadId, ThreadState, Tier,
};

/// A tool call in flight, so an update can be matched to what started it.
#[derive(Debug, Clone)]
struct PendingTool {
    title: String,
    kind: String,
    /// The command, for an `execute` call.
    command: Option<String>,
    /// The path, for a file call.
    path: Option<String>,
    started: tervin_core::Timestamp,
}

/// Normalises one ACP session.
pub struct Normalizer {
    thread_id: ThreadId,
    agent: AgentIdentity,
    cwd: String,
    project: Option<String>,
    state: ThreadState,
    tools: std::collections::HashMap<String, PendingTool>,
    /// Text accumulated for the current assistant message.
    message_buffer: String,
    /// Reasoning accumulated for the current thought.
    thought_buffer: String,
    pub session_id: Option<String>,
    pub mode: Option<String>,
    pub cost: CostSnapshot,
    pub denials: Vec<String>,
}

impl Normalizer {
    pub fn new(thread_id: ThreadId, agent: AgentIdentity, cwd: impl Into<String>) -> Self {
        let cwd = cwd.into();
        let project = std::path::Path::new(&cwd)
            .file_name()
            .and_then(|s| s.to_str())
            .map(String::from);
        Self {
            thread_id,
            agent,
            cwd,
            project,
            state: ThreadState::Starting,
            tools: std::collections::HashMap::new(),
            message_buffer: String::new(),
            thought_buffer: String::new(),
            session_id: None,
            mode: None,
            cost: CostSnapshot::default(),
            denials: Vec::new(),
        }
    }

    pub fn state(&self) -> ThreadState {
        self.state
    }

    fn event(&self, summary: impl Into<String>, payload: EventPayload) -> TervinEvent {
        TervinEvent::new(self.agent.clone(), summary, payload)
            .with_thread(self.thread_id.clone())
            .with_location(self.project.clone(), Some(self.cwd.clone()))
    }

    fn transition(&mut self, next: ThreadState, out: &mut Vec<TervinEvent>) {
        if self.state != next {
            self.state = next;
            out.push(self.event(next.label(), EventPayload::ThreadState { state: next }));
        }
    }

    /// Record the session Tervin was given, and that the Thread has started.
    pub fn started(&mut self, session_id: String, resumable: bool) -> Vec<TervinEvent> {
        self.session_id = Some(session_id.clone());
        let mut out = vec![self.event(
            "Session started",
            EventPayload::ThreadStarted {
                tier: Tier::Structured,
                task_title: None,
                // Only claim resumability when the agent said it supports loading.
                resume_id: resumable.then_some(session_id),
            },
        )];
        self.transition(ThreadState::Understanding, &mut out);
        out
    }

    /// Consume one `session/update`.
    pub fn ingest(&mut self, update: SessionUpdate) -> Vec<TervinEvent> {
        let mut out = Vec::new();

        match update {
            // Chunks are buffered, not emitted: one timeline row per token is
            // unreadable.
            SessionUpdate::AgentMessageChunk { text } => {
                self.message_buffer.push_str(&text);
            }
            SessionUpdate::AgentThoughtChunk { text } => {
                self.thought_buffer.push_str(&text);
                self.transition(ThreadState::Understanding, &mut out);
            }
            SessionUpdate::UserMessageChunk { .. } => {
                // Tervin already recorded the prompt when it sent it; echoing it
                // back would duplicate the timeline row.
            }

            SessionUpdate::ToolCall {
                id,
                title,
                kind,
                raw_input,
                ..
            } => {
                // A tool call ends the current message: flush it first so the
                // timeline reads in the order things happened.
                out.extend(self.flush_buffers());

                let command = raw_input
                    .get("command")
                    .and_then(Value::as_str)
                    .map(String::from);
                let path = raw_input
                    .get("path")
                    .or_else(|| raw_input.get("abs_path"))
                    .or_else(|| raw_input.get("file_path"))
                    .and_then(Value::as_str)
                    .map(String::from);

                self.tools.insert(
                    id.clone(),
                    PendingTool {
                        title: title.clone(),
                        kind: kind.clone(),
                        command: command.clone(),
                        path: path.clone(),
                        started: tervin_core::now(),
                    },
                );

                out.push(self.event(
                    format!("{kind}: {title}"),
                    EventPayload::ToolRequested {
                        tool_use_id: id,
                        tool_name: kind.clone(),
                        input_summary: title.clone(),
                        parent_tool_use_id: None,
                    },
                ));

                // ACP names the tool's intent, so the state is known rather than
                // guessed from a tool name.
                match kind.as_str() {
                    "execute" => {
                        if let Some(command) = &command {
                            let mut risk = rules_engine::classify(command, &self.cwd);
                            // Under ACP the gate is real, so this is enforceable —
                            // unlike an action merely observed after the fact.
                            risk.enforceable = true;
                            out.push(self.event(
                                format!("$ {}", first_line(command, 120)),
                                EventPayload::CommandProposed {
                                    command: command.clone(),
                                    cwd: Some(self.cwd.clone()),
                                    risk,
                                },
                            ));
                        }
                        self.transition(ThreadState::Executing, &mut out);
                    }
                    "read" => {
                        if let Some(path) = &path {
                            out.push(
                                self.event(
                                    format!("Read {}", short_path(path)),
                                    EventPayload::FileRead {
                                        path: path.clone(),
                                        lines: None,
                                    },
                                )
                                .with_links(vec![Link::File {
                                    path: path.clone(),
                                    line: None,
                                }]),
                            );
                        }
                        self.transition(ThreadState::Reading, &mut out);
                    }
                    "edit" | "delete" | "move" => {
                        if let Some(path) = &path {
                            out.push(
                                self.event(
                                    format!("Proposed change to {}", short_path(path)),
                                    EventPayload::PatchProposed {
                                        files: vec![FileChange {
                                            path: path.clone(),
                                            kind: match kind.as_str() {
                                                "delete" => FileChangeKind::Deleted,
                                                "move" => FileChangeKind::Renamed,
                                                _ => FileChangeKind::Modified,
                                            },
                                            added_lines: None,
                                            removed_lines: None,
                                        }],
                                        unified_diff: None,
                                    },
                                )
                                .with_links(vec![Link::File {
                                    path: path.clone(),
                                    line: None,
                                }]),
                            );
                        }
                        self.transition(ThreadState::Editing, &mut out);
                    }
                    "think" => self.transition(ThreadState::Understanding, &mut out),
                    "search" | "fetch" => self.transition(ThreadState::Reading, &mut out),
                    _ => self.transition(ThreadState::WaitingForExternalTool, &mut out),
                }
            }

            SessionUpdate::ToolCallUpdate {
                id,
                status,
                content,
            } => {
                let Some(pending) = self.tools.get(&id).cloned() else {
                    // An update for a call Tervin never saw start. Recorded rather
                    // than dropped: it still happened.
                    out.push(self.event(
                        format!("Tool update for an unknown call {id}"),
                        EventPayload::RuntimeUnclassified {
                            source_type: "acp/tool_call_update".to_string(),
                        },
                    ));
                    return out;
                };

                let finished = matches!(status.as_deref(), Some("completed") | Some("failed"));
                if !finished {
                    return out;
                }
                self.tools.remove(&id);

                let is_error = status.as_deref() == Some("failed");
                let duration = (tervin_core::now() - pending.started)
                    .num_milliseconds()
                    .max(0) as u64;
                let output = content.unwrap_or_default();

                out.push(self.event(
                    format!(
                        "{} {}",
                        pending.title,
                        if is_error { "failed" } else { "completed" }
                    ),
                    EventPayload::ToolCompleted {
                        tool_use_id: id,
                        tool_name: pending.kind.clone(),
                        is_error,
                        output_summary: first_line(&output, 200),
                        duration_ms: Some(duration),
                    },
                ));

                if pending.kind == "execute" {
                    let command = pending.command.clone().unwrap_or_default();
                    if !output.trim().is_empty() {
                        out.push(self.event(
                            first_line(&output, 120),
                            EventPayload::CommandOutput {
                                stream: OutputStream::Stdout,
                                excerpt: truncate(&output, 4000),
                                block_id: None,
                            },
                        ));
                    }
                    // ACP reports success or failure, not an exit status. The
                    // summary says so rather than presenting a derived number as
                    // something the agent stated.
                    out.push(self.event(
                        format!(
                            "$ {} — {}",
                            first_line(&command, 80),
                            if is_error {
                                "reported as failed (exit status not reported by the protocol)"
                            } else {
                                "reported as succeeded"
                            }
                        ),
                        EventPayload::CommandCompleted {
                            command,
                            exit_code: if is_error { 1 } else { 0 },
                            duration_ms: duration,
                            // Derived from a tool-call status: this path is a report that
                            // the call failed, not a process exit status.
                            exit_code_reported: false,
                            block_id: None,
                        },
                    ));
                }

                if matches!(pending.kind.as_str(), "edit" | "delete" | "move") && !is_error {
                    if let Some(path) = pending.path {
                        out.push(
                            self.event(
                                format!("Changed {}", short_path(&path)),
                                EventPayload::PatchApplied {
                                    files: vec![FileChange {
                                        path: path.clone(),
                                        kind: FileChangeKind::Modified,
                                        added_lines: None,
                                        removed_lines: None,
                                    }],
                                    // Under ACP the write was gated by Tervin, so
                                    // the authority genuinely is Tervin's.
                                    authority: DecisionAuthority::Tervin,
                                },
                            )
                            .with_links(vec![Link::File { path, line: None }]),
                        );
                    }
                }
            }

            SessionUpdate::Plan { entries } => {
                out.extend(self.flush_buffers());
                let steps = plan_steps(&entries);
                if !steps.is_empty() {
                    out.push(self.event(
                        format!("Plan · {} steps", steps.len()),
                        EventPayload::PlanProposed {
                            steps,
                            raw_text: None,
                        },
                    ));
                    self.transition(ThreadState::Planning, &mut out);
                }
            }

            SessionUpdate::ModeUpdated { mode } => {
                self.mode = Some(mode.clone());
                out.push(self.event(
                    format!("Mode: {mode}"),
                    EventPayload::RuntimeUnclassified {
                        source_type: "acp/current_mode_update".to_string(),
                    },
                ));
            }

            SessionUpdate::Other { kind } => {
                out.push(self.event(
                    format!("Unmodelled update: {kind}"),
                    EventPayload::RuntimeUnclassified {
                        source_type: format!("acp/{kind}"),
                    },
                ));
            }
        }

        out
    }

    /// Emit whatever text has accumulated, as one message each.
    pub fn flush_buffers(&mut self) -> Vec<TervinEvent> {
        let mut out = Vec::new();

        if !self.thought_buffer.trim().is_empty() {
            let text = std::mem::take(&mut self.thought_buffer);
            out.push(self.event(
                first_line(&text, 120),
                EventPayload::AgentMessage {
                    text,
                    is_reasoning: true,
                    parent_tool_use_id: None,
                },
            ));
        } else {
            self.thought_buffer.clear();
        }

        if !self.message_buffer.trim().is_empty() {
            let text = std::mem::take(&mut self.message_buffer);
            out.push(self.event(
                first_line(&text, 120),
                EventPayload::AgentMessage {
                    text,
                    is_reasoning: false,
                    parent_tool_use_id: None,
                },
            ));
        } else {
            self.message_buffer.clear();
        }

        out
    }

    /// A permission decision Tervin made when the agent asked.
    ///
    /// Attributed to Tervin because under ACP the gate is real: the agent was
    /// waiting for this answer and honours it.
    pub fn decided(&mut self, action: &str, allowed: bool, reason: &str) -> Vec<TervinEvent> {
        if !allowed {
            self.denials.push(action.to_string());
        }
        vec![if allowed {
            self.event(
                format!("{action} approved"),
                EventPayload::PermissionGranted {
                    request_id: None,
                    action: action.to_string(),
                    authority: DecisionAuthority::Tervin,
                    scope: reason.to_string(),
                },
            )
        } else {
            self.event(
                format!("{action} denied"),
                EventPayload::PermissionDenied {
                    request_id: None,
                    action: action.to_string(),
                    authority: DecisionAuthority::Tervin,
                    reason: Some(reason.to_string()),
                },
            )
        }]
    }

    /// A file Tervin wrote on the agent's behalf, through `fs/write_text_file`.
    ///
    /// Attributed to Tervin because Tervin performed the write, after its own gate.
    pub fn file_written(&mut self, path: &str, existed: bool) -> Vec<TervinEvent> {
        vec![self
            .event(
                format!("Wrote {}", short_path(path)),
                EventPayload::PatchApplied {
                    files: vec![FileChange {
                        path: path.to_string(),
                        kind: if existed {
                            FileChangeKind::Modified
                        } else {
                            FileChangeKind::Created
                        },
                        added_lines: None,
                        removed_lines: None,
                    }],
                    authority: DecisionAuthority::Tervin,
                },
            )
            .with_links(vec![Link::File {
                path: path.to_string(),
                line: None,
            }])]
    }

    /// A command Tervin started for the agent, through `terminal/create`.
    pub fn command_started(&mut self, command: &str) -> Vec<TervinEvent> {
        let mut out = vec![self.event(
            format!("$ {}", first_line(command, 120)),
            EventPayload::CommandStarted {
                command: command.to_string(),
                block_id: None,
            },
        )];
        self.transition(ThreadState::Executing, &mut out);
        out
    }

    /// A command Tervin ran for the agent finished.
    ///
    /// The exit code here is a real one — Tervin owned the process — unlike the
    /// success/failure flag a `tool_call_update` carries. When `terminated` is set,
    /// Tervin killed it, and the summary says so rather than presenting the
    /// stand-in code as something the process reported.
    pub fn command_finished(
        &mut self,
        command: &str,
        exit_code: i32,
        duration_ms: u64,
        output: &str,
        terminated: bool,
    ) -> Vec<TervinEvent> {
        let mut out = Vec::new();
        if !output.trim().is_empty() {
            out.push(self.event(
                first_line(output, 120),
                EventPayload::CommandOutput {
                    stream: OutputStream::Stdout,
                    excerpt: truncate(output, 4000),
                    block_id: None,
                },
            ));
        }
        let ending = if terminated {
            "terminated by Tervin".to_string()
        } else {
            format!("exit {exit_code}")
        };
        out.push(self.event(
            format!("$ {} — {ending}", first_line(command, 80)),
            EventPayload::CommandCompleted {
                command: command.to_string(),
                exit_code,
                duration_ms,
                // A real status: this comes from waiting on the ACP terminal, which is
                // why the summary above prints the number.
                exit_code_reported: true,
                block_id: None,
            },
        ));
        out
    }

    /// The agent is waiting on a permission answer.
    pub fn awaiting_permission(&mut self, action: &str, risk: RiskAssessment) -> Vec<TervinEvent> {
        let mut out = self.flush_buffers();
        out.push(self.event(
            format!("Permission requested: {action}"),
            EventPayload::PermissionRequested {
                request_id: tervin_core::RequestId::new(),
                action: action.to_string(),
                risk,
                // The distinguishing property of ACP: the agent is blocked on the
                // answer, so this really is interceptable.
                interceptable: true,
            },
        ));
        self.transition(ThreadState::WaitingForPermission, &mut out);
        out
    }

    /// A prompt turn ended.
    pub fn turn_ended(&mut self, reason: StopReason) -> Vec<TervinEvent> {
        let mut out = self.flush_buffers();

        if reason.is_success() {
            self.state = ThreadState::Completed;
            out.push(self.event(
                "Completed",
                EventPayload::ThreadCompleted {
                    result: None,
                    duration_ms: None,
                    cost: Some(self.cost.clone()),
                },
            ));
        } else {
            // Being cut short is not completion, and the UI must not show it as
            // one.
            self.state = if reason == StopReason::Cancelled {
                ThreadState::Interrupted
            } else {
                ThreadState::Failed
            };
            out.push(self.event(
                reason.label(),
                EventPayload::ThreadFailed {
                    reason: reason.label().to_string(),
                    recoverable: Some(matches!(
                        reason,
                        StopReason::MaxTokens | StopReason::MaxTurnRequests | StopReason::Cancelled
                    )),
                },
            ));
        }
        out
    }

    /// The process ended without a proper turn end.
    pub fn disconnected(&mut self, detail: &str) -> Vec<TervinEvent> {
        if self.state.is_terminal() {
            return Vec::new();
        }
        self.state = ThreadState::Disconnected;
        vec![self.event(
            "Agent process ended unexpectedly",
            EventPayload::ThreadFailed {
                reason: detail.to_string(),
                recoverable: Some(true),
            },
        )]
    }

    /// Record the user's own turn.
    pub fn user_prompt(
        &mut self,
        text: &str,
        attachments: &[crate::runtime::Attachment],
    ) -> Vec<TervinEvent> {
        let mut out = Vec::new();
        if !attachments.is_empty() {
            let kinds: Vec<String> = attachments.iter().map(|a| a.describe()).collect();
            out.push(self.event(
                format!("Attached {} item(s)", kinds.len()),
                EventPayload::ContextAttached {
                    description: kinds.join(", "),
                    kinds,
                },
            ));
        }
        out.push(self.event(
            first_line(text, 120),
            EventPayload::UserPrompted {
                text: text.to_string(),
            },
        ));
        self.transition(ThreadState::Understanding, &mut out);
        out
    }
}

/// Turn ACP plan entries into Tervin plan steps.
fn plan_steps(entries: &[PlanEntry]) -> Vec<PlanStep> {
    entries
        .iter()
        .filter(|e| !e.content.trim().is_empty())
        .map(|e| PlanStep {
            description: e.content.clone(),
            touches: Vec::new(),
        })
        .collect()
}

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    truncate(line.trim(), max)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    format!("{}…", text.chars().take(max).collect::<String>())
}

fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match parts.len() {
        0 => path.to_string(),
        1 => parts[0].to_string(),
        n => format!("{}/{}", parts[n - 2], parts[n - 1]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::protocol::parse_session_update;
    use serde_json::json;

    fn normalizer() -> Normalizer {
        Normalizer::new(
            ThreadId::new(),
            AgentIdentity::new("acp", "ACP agent", Tier::Structured),
            "/Users/dev/proj",
        )
    }

    fn feed(n: &mut Normalizer, params: serde_json::Value) -> Vec<TervinEvent> {
        let (_, update) = parse_session_update(&params).expect("unparseable update");
        n.ingest(update)
    }

    fn kinds(events: &[TervinEvent]) -> Vec<&'static str> {
        events.iter().map(|e| e.kind()).collect()
    }

    fn chunk(text: &str) -> serde_json::Value {
        json!({
            "sessionId":"s1",
            "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}
        })
    }

    #[test]
    fn message_chunks_coalesce_into_one_event() {
        // One timeline row per token would be unreadable.
        let mut n = normalizer();
        assert!(feed(&mut n, chunk("Looking ")).is_empty());
        assert!(feed(&mut n, chunk("at the ")).is_empty());
        assert!(feed(&mut n, chunk("parser.")).is_empty());

        let flushed = n.flush_buffers();
        assert_eq!(kinds(&flushed), vec!["agent.message"]);
        match &flushed[0].payload {
            EventPayload::AgentMessage {
                text, is_reasoning, ..
            } => {
                assert_eq!(text, "Looking at the parser.");
                assert!(!is_reasoning);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_tool_call_flushes_the_message_before_it() {
        // Otherwise the timeline reads out of order.
        let mut n = normalizer();
        feed(&mut n, chunk("I will read the file."));
        let events = feed(
            &mut n,
            json!({
                "sessionId":"s1",
                "update":{
                    "sessionUpdate":"tool_call","toolCallId":"t1",
                    "title":"Read src/main.rs","kind":"read","status":"pending",
                    "rawInput":{"path":"src/main.rs"}
                }
            }),
        );
        let seen = kinds(&events);
        let message_at = seen.iter().position(|k| *k == "agent.message");
        let tool_at = seen.iter().position(|k| *k == "tool.requested");
        assert!(message_at.is_some() && tool_at.is_some());
        assert!(
            message_at < tool_at,
            "the message must precede the tool call"
        );
    }

    #[test]
    fn thoughts_are_marked_as_reasoning() {
        let mut n = normalizer();
        feed(
            &mut n,
            json!({
                "sessionId":"s1",
                "update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"considering"}}
            }),
        );
        let flushed = n.flush_buffers();
        assert!(flushed.iter().any(|e| matches!(
            &e.payload,
            EventPayload::AgentMessage {
                is_reasoning: true,
                ..
            }
        )));
    }

    #[test]
    fn an_execute_tool_call_is_risk_classified_as_enforceable() {
        // The point of ACP: the agent is blocked on the answer, so the gate is
        // real and the risk assessment must say so.
        let mut n = normalizer();
        let events = feed(
            &mut n,
            json!({
                "sessionId":"s1",
                "update":{
                    "sessionUpdate":"tool_call","toolCallId":"t1",
                    "title":"Run rm -rf /","kind":"execute","status":"pending",
                    "rawInput":{"command":"rm -rf /"}
                }
            }),
        );
        let risk = events
            .iter()
            .find_map(|e| match &e.payload {
                EventPayload::CommandProposed { risk, .. } => Some(risk),
                _ => None,
            })
            .expect("no command.proposed");
        assert_eq!(risk.level, tervin_core::RiskLevel::Critical);
        assert!(
            risk.enforceable,
            "under ACP the action is genuinely gated, unlike one merely observed"
        );
        assert_eq!(n.state(), ThreadState::Executing);
    }

    #[test]
    fn a_completed_execute_call_reports_output_and_a_derived_status() {
        let mut n = normalizer();
        feed(
            &mut n,
            json!({
                "sessionId":"s1",
                "update":{
                    "sessionUpdate":"tool_call","toolCallId":"t1",
                    "title":"Run tests","kind":"execute","status":"pending",
                    "rawInput":{"command":"cargo test"}
                }
            }),
        );
        let events = feed(
            &mut n,
            json!({
                "sessionId":"s1",
                "update":{
                    "sessionUpdate":"tool_call_update","toolCallId":"t1",
                    "status":"completed","content":{"type":"text","text":"test result: ok"}
                }
            }),
        );

        let seen = kinds(&events);
        assert!(seen.contains(&"tool.completed"));
        assert!(seen.contains(&"command.output"));

        let completed = events
            .iter()
            .find(|e| matches!(e.payload, EventPayload::CommandCompleted { .. }))
            .expect("no command.completed");
        // The exit code is derived, and the summary must say so.
        assert!(
            completed.summary.contains("reported as succeeded"),
            "summary was {}",
            completed.summary
        );
    }

    #[test]
    fn an_in_progress_update_emits_nothing() {
        // Only a terminal status closes a call; progress is not an event.
        let mut n = normalizer();
        feed(
            &mut n,
            json!({"sessionId":"s1","update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"x","kind":"read","status":"pending","rawInput":{}}}),
        );
        let events = feed(
            &mut n,
            json!({"sessionId":"s1","update":{"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"in_progress"}}),
        );
        assert!(events.is_empty());
    }

    #[test]
    fn an_edit_that_completes_is_attributed_to_tervin() {
        // Tervin gated the write, so the authority genuinely is Tervin's — unlike
        // the Claude Code adapter, where it is provider-native.
        let mut n = normalizer();
        feed(
            &mut n,
            json!({"sessionId":"s1","update":{"sessionUpdate":"tool_call","toolCallId":"e1","title":"Edit lib.rs","kind":"edit","status":"pending","rawInput":{"path":"/p/src/lib.rs"}}}),
        );
        let events = feed(
            &mut n,
            json!({"sessionId":"s1","update":{"sessionUpdate":"tool_call_update","toolCallId":"e1","status":"completed"}}),
        );
        let applied = events
            .iter()
            .find(|e| matches!(e.payload, EventPayload::PatchApplied { .. }))
            .expect("no patch.applied");
        match &applied.payload {
            EventPayload::PatchApplied { authority, .. } => {
                assert_eq!(*authority, DecisionAuthority::Tervin)
            }
            _ => unreachable!(),
        }
        assert!(matches!(applied.links.first(), Some(Link::File { .. })));
    }

    #[test]
    fn an_update_for_an_unknown_call_is_recorded_not_dropped() {
        let mut n = normalizer();
        let events = feed(
            &mut n,
            json!({"sessionId":"s1","update":{"sessionUpdate":"tool_call_update","toolCallId":"ghost","status":"completed"}}),
        );
        assert_eq!(kinds(&events), vec!["runtime.unclassified"]);
    }

    #[test]
    fn a_plan_becomes_plan_proposed() {
        let mut n = normalizer();
        let events = feed(
            &mut n,
            json!({
                "sessionId":"s1",
                "update":{"sessionUpdate":"plan","entries":[
                    {"content":"Read the parser","status":"pending"},
                    {"content":"Add a test","status":"pending"},
                    {"content":"   ","status":"pending"}
                ]}
            }),
        );
        let steps = events
            .iter()
            .find_map(|e| match &e.payload {
                EventPayload::PlanProposed { steps, .. } => Some(steps.clone()),
                _ => None,
            })
            .expect("no plan.proposed");
        // Blank entries are dropped rather than shown as empty steps.
        assert_eq!(steps.len(), 2);
        assert_eq!(n.state(), ThreadState::Planning);
    }

    #[test]
    fn a_permission_request_is_marked_interceptable() {
        let mut n = normalizer();
        let events = n.awaiting_permission("rm -rf build", RiskAssessment::benign());
        let requested = events
            .iter()
            .find_map(|e| match &e.payload {
                EventPayload::PermissionRequested { interceptable, .. } => Some(*interceptable),
                _ => None,
            })
            .expect("no permission.requested");
        assert!(requested, "under ACP the agent is blocked on the answer");
        assert_eq!(n.state(), ThreadState::WaitingForPermission);
    }

    #[test]
    fn a_command_tervin_ran_itself_reports_a_real_exit_code() {
        // Distinct from a tool_call_update, where success is a flag and the exit
        // status is not reported at all.
        let mut n = normalizer();
        let events = n.command_finished("cargo test", 101, 4200, "error: 1 failed", false);
        let completed = events
            .iter()
            .find_map(|e| match &e.payload {
                EventPayload::CommandCompleted {
                    exit_code,
                    duration_ms,
                    ..
                } => Some((*exit_code, *duration_ms)),
                _ => None,
            })
            .expect("no command.completed");
        assert_eq!(completed, (101, 4200));
        assert!(events.iter().any(|e| e.kind() == "command.output"));
        assert!(
            events.iter().any(|e| e.summary.contains("exit 101")),
            "the summary should carry the real status"
        );
    }

    #[test]
    fn a_terminated_command_does_not_present_its_stand_in_code_as_a_status() {
        // A killed process reports no exit code. Showing "-1" as though the program
        // returned it would be a small lie in the place a user looks first.
        let mut n = normalizer();
        let events = n.command_finished("sleep 120", -1, 40, "", true);
        let completed = events
            .iter()
            .find(|e| e.kind() == "command.completed")
            .expect("no command.completed");
        assert!(
            completed.summary.contains("terminated by Tervin"),
            "summary was {}",
            completed.summary
        );
        assert!(!completed.summary.contains("exit -1"));
    }

    #[test]
    fn a_file_tervin_wrote_records_whether_it_was_created() {
        let mut n = normalizer();
        match &n.file_written("/p/src/new.rs", false)[0].payload {
            EventPayload::PatchApplied { files, authority } => {
                assert_eq!(files[0].kind, FileChangeKind::Created);
                assert_eq!(*authority, DecisionAuthority::Tervin);
            }
            other => panic!("got {other:?}"),
        }
        match &n.file_written("/p/src/old.rs", true)[0].payload {
            EventPayload::PatchApplied { files, .. } => {
                assert_eq!(files[0].kind, FileChangeKind::Modified)
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn decisions_are_attributed_to_tervin() {
        let mut n = normalizer();
        match &n.decided("rm -rf /", false, "Denied by Tervin Rules")[0].payload {
            EventPayload::PermissionDenied { authority, .. } => {
                assert_eq!(*authority, DecisionAuthority::Tervin)
            }
            other => panic!("got {other:?}"),
        }
        assert_eq!(n.denials.len(), 1);
    }

    #[test]
    fn only_end_turn_completes_the_thread() {
        for (reason, expected) in [
            (StopReason::EndTurn, ThreadState::Completed),
            (StopReason::MaxTokens, ThreadState::Failed),
            (StopReason::Refusal, ThreadState::Failed),
            (StopReason::Cancelled, ThreadState::Interrupted),
        ] {
            let mut n = normalizer();
            let events = n.turn_ended(reason);
            assert_eq!(n.state(), expected, "{reason:?}");
            assert!(!events.is_empty());
        }
    }

    #[test]
    fn a_refusal_is_not_presented_as_success() {
        // Being cut short must never look like finishing the work.
        let mut n = normalizer();
        let events = n.turn_ended(StopReason::Refusal);
        assert!(events.iter().any(|e| e.kind() == "thread.failed"));
        assert!(!events.iter().any(|e| e.kind() == "thread.completed"));
    }

    #[test]
    fn a_completed_thread_is_not_overwritten_by_a_later_disconnect() {
        let mut n = normalizer();
        n.turn_ended(StopReason::EndTurn);
        assert!(n.disconnected("process exited").is_empty());
        assert_eq!(n.state(), ThreadState::Completed);
    }

    #[test]
    fn resumability_is_claimed_only_when_the_agent_supports_loading() {
        let mut n = normalizer();
        let events = n.started("s1".to_string(), false);
        match &events[0].payload {
            EventPayload::ThreadStarted { resume_id, .. } => assert!(
                resume_id.is_none(),
                "must not offer resume when the agent cannot load a session"
            ),
            other => panic!("got {other:?}"),
        }

        let mut n2 = normalizer();
        match &n2.started("s2".to_string(), true)[0].payload {
            EventPayload::ThreadStarted { resume_id, .. } => {
                assert_eq!(resume_id.as_deref(), Some("s2"))
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn every_event_carries_its_thread_and_location() {
        let mut n = normalizer();
        let mut all = n.started("s1".to_string(), true);
        all.extend(feed(&mut n, chunk("hello")));
        all.extend(n.flush_buffers());
        all.extend(n.turn_ended(StopReason::EndTurn));

        assert!(!all.is_empty());
        for event in &all {
            assert!(
                event.thread_id.is_some(),
                "{} lacked a thread",
                event.kind()
            );
            assert!(event.cwd.is_some(), "{} lacked a cwd", event.kind());
            assert!(
                !event.summary.is_empty(),
                "{} lacked a summary",
                event.kind()
            );
        }
    }

    #[test]
    fn an_unmodelled_update_is_kept() {
        let mut n = normalizer();
        let events = feed(
            &mut n,
            json!({"sessionId":"s1","update":{"sessionUpdate":"future_thing"}}),
        );
        assert_eq!(kinds(&events), vec!["runtime.unclassified"]);
    }
}
