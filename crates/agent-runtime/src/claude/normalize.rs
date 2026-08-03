//! Translating Claude Code's `stream-json` output into Tervin events.
//!
//! Kept as a pure state machine — JSON in, events out, no I/O — so it can be
//! tested against a transcript captured from the real CLI rather than against an
//! assumed schema.
//!
//! Unrecognised message types become `runtime.unclassified` and keep their raw
//! payload. Dropping them would make the timeline quietly incomplete, and
//! guessing at them would make it quietly wrong; the third option is to show that
//! something happened which Tervin does not model.

use serde_json::Value;
use std::collections::HashMap;
use tervin_core::events::{
    CostSnapshot, DecisionAuthority, FileChange, FileChangeKind, PlanStep, RawRef, Severity,
    TestOutcome,
};
use tervin_core::{
    AgentIdentity, DiagnosticId, EventPayload, Link, RiskAssessment, TervinEvent, ThreadId,
    ThreadState,
};

/// A tool call awaiting its result.
#[derive(Debug, Clone)]
struct PendingTool {
    name: String,
    /// Compact human-readable rendering of the input.
    summary: String,
    /// For `Bash`, the command being run.
    command: Option<String>,
    /// For file tools, the path.
    path: Option<String>,
    started_at: tervin_core::Timestamp,
}

/// Normalises one Claude Code session's output stream.
pub struct Normalizer {
    thread_id: ThreadId,
    agent: AgentIdentity,
    cwd: String,
    project: Option<String>,
    pending_tools: HashMap<String, PendingTool>,
    state: ThreadState,
    /// The runtime's own session id, needed to resume.
    pub resume_id: Option<String>,
    pub model: Option<String>,
    pub runtime_version: Option<String>,
    pub tools: Vec<String>,
    pub mcp_servers: Vec<crate::runtime::McpServerState>,
    pub slash_commands: Vec<String>,
    pub cost: CostSnapshot,
    pub denials: Vec<String>,
    /// Raw payloads to persist, keyed by the pointer referenced from events.
    pub raw_sink: Vec<(String, String)>,
    /// Which account this Thread runs as, used in a sign-in message.
    account_hint: Option<String>,
    /// Every hook run observed, for the Bridge panel.
    pub hook_runs: Vec<crate::runtime::HookRun>,
    /// The subagent currently doing the work, if the parent handed off to one.
    subagent: Option<SubagentRun>,
    seq: u64,
}

/// What is known about a running subagent, carried so its end can be reported.
///
/// The runtime announces a subagent's progress but never its completion; the only
/// signal is the parent's `Task` tool returning. Holding the last progress report
/// is what lets that return say how much the subagent actually did.
#[derive(Debug, Clone)]
struct SubagentRun {
    tool_use_id: String,
    subagent_type: String,
    tool_uses: u64,
    total_tokens: u64,
    elapsed_ms: u64,
}

impl Normalizer {
    pub fn new(thread_id: ThreadId, agent: AgentIdentity, cwd: impl Into<String>) -> Self {
        let cwd = cwd.into();
        let project = std::path::Path::new(&cwd)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
        Self {
            thread_id,
            agent,
            cwd,
            project,
            pending_tools: HashMap::new(),
            state: ThreadState::Starting,
            resume_id: None,
            model: None,
            runtime_version: None,
            tools: Vec::new(),
            mcp_servers: Vec::new(),
            slash_commands: Vec::new(),
            cost: CostSnapshot::default(),
            denials: Vec::new(),
            raw_sink: Vec::new(),
            account_hint: None,
            hook_runs: Vec::new(),
            subagent: None,
            seq: 0,
        }
    }

    /// Where the runtime says this Thread is working.
    ///
    /// Its own answer, from `init` and kept current, rather than the directory
    /// Tervin asked for. Every path an agent touches is relative to this.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn state(&self) -> ThreadState {
        self.state
    }

    fn event(&self, summary: impl Into<String>, payload: EventPayload) -> TervinEvent {
        TervinEvent::new(self.agent.clone(), summary, payload)
            .with_thread(self.thread_id.clone())
            .with_location(self.project.clone(), Some(self.cwd.clone()))
    }

    fn event_with_links(
        &self,
        summary: impl Into<String>,
        payload: EventPayload,
        links: Vec<Link>,
    ) -> TervinEvent {
        self.event(summary, payload).with_links(links)
    }

    /// Store a raw payload and return a reference to it.
    fn stash_raw(&mut self, value: &Value) -> RawRef {
        self.seq += 1;
        let pointer = format!("{}#{}", self.thread_id, self.seq);
        let body = value.to_string();
        let byte_len = body.len();
        self.raw_sink.push((pointer.clone(), body));
        RawRef {
            kind: "claude-code/stream-json".to_string(),
            pointer,
            byte_len,
            // Redaction happens at export, not here: the local store keeps what
            // the runtime actually said so an audit is faithful.
            redacted: false,
        }
    }

    fn transition(&mut self, next: ThreadState, out: &mut Vec<TervinEvent>) {
        if self.state != next {
            self.state = next;
            out.push(self.event(next.label(), EventPayload::ThreadState { state: next }));
        }
    }

    /// Consume one parsed JSON line.
    pub fn ingest(&mut self, value: &Value) -> Vec<TervinEvent> {
        let mut out = Vec::new();
        let msg_type = value.get("type").and_then(Value::as_str).unwrap_or("");

        match msg_type {
            "system" => self.ingest_system(value, &mut out),
            "assistant" => self.ingest_assistant(value, &mut out),
            "user" => self.ingest_user(value, &mut out),
            "result" => self.ingest_result(value, &mut out),
            "stream_event" => {
                // Partial message deltas. Useful for live typing but too noisy
                // for a timeline, so they are not turned into events.
            }
            "rate_limit_event" => {
                if let Some(info) = value.get("rate_limit_info") {
                    let status = info
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    if status != "allowed" {
                        let raw = self.stash_raw(value);
                        out.push(
                            self.event(
                                format!("Rate limit: {status}"),
                                EventPayload::RuntimeUnclassified {
                                    source_type: "rate_limit_event".to_string(),
                                },
                            )
                            .with_raw(raw),
                        );
                    }
                }
            }
            other => {
                let raw = self.stash_raw(value);
                out.push(
                    self.event(
                        format!("Unrecognised runtime message: {other}"),
                        EventPayload::RuntimeUnclassified {
                            source_type: other.to_string(),
                        },
                    )
                    .with_raw(raw),
                );
            }
        }

        out
    }

    fn ingest_system(&mut self, value: &Value, out: &mut Vec<TervinEvent>) {
        let subtype = value.get("subtype").and_then(Value::as_str).unwrap_or("");

        match subtype {
            "init" => {
                self.resume_id = value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(String::from);
                self.model = value.get("model").and_then(Value::as_str).map(String::from);
                self.runtime_version = value
                    .get("claude_code_version")
                    .and_then(Value::as_str)
                    .map(String::from);
                if let Some(cwd) = value.get("cwd").and_then(Value::as_str) {
                    self.cwd = cwd.to_string();
                }
                self.tools = string_array(value.get("tools"));
                self.slash_commands = value
                    .get("slash_commands")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| {
                                v.as_str()
                                    .map(String::from)
                                    .or_else(|| v.get("name")?.as_str().map(String::from))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.mcp_servers = value
                    .get("mcp_servers")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| {
                                Some(crate::runtime::McpServerState {
                                    name: v.get("name")?.as_str()?.to_string(),
                                    status: v
                                        .get("status")
                                        .and_then(Value::as_str)
                                        .unwrap_or("unknown")
                                        .to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                self.agent.model = self.model.clone();
                self.agent.version = self.runtime_version.clone();

                if let Some(model) = &self.model {
                    self.cost.model = Some(model.clone());
                }

                let raw = self.stash_raw(value);
                out.push(
                    self.event(
                        match &self.model {
                            Some(m) => format!("Session started · {m}"),
                            None => "Session started".to_string(),
                        },
                        EventPayload::ThreadStarted {
                            tier: tervin_core::Tier::Structured,
                            task_title: None,
                            resume_id: self.resume_id.clone(),
                        },
                    )
                    .with_raw(raw),
                );
                self.transition(ThreadState::Understanding, out);
            }

            // A hook starting is not an outcome. Recording it would double every
            // hook in the timeline for no added information.
            "hook_started" => {}

            "hook_response" => self.ingest_hook_response(value, out),

            // A subagent reporting itself. Every field the timeline needs is already
            // here — what it is, what it is doing, how much it has done — and all of
            // it used to be discarded as unclassified, which is why a Thread running
            // a subagent looked like a Thread that had stopped.
            "task_progress" => {
                let raw = self.stash_raw(value);
                let text = |key: &str| {
                    value
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                let usage = value.get("usage");
                let num = |key: &str| {
                    usage
                        .and_then(|u| u.get(key))
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                };

                let tool_use_id = text("tool_use_id");
                let subagent_type = {
                    let t = text("subagent_type");
                    if t.is_empty() {
                        "subagent".to_string()
                    } else {
                        t
                    }
                };
                let description = text("description");
                let tool_uses = num("tool_uses");
                let total_tokens = num("total_tokens");
                let elapsed_ms = num("duration_ms");

                self.subagent = Some(SubagentRun {
                    tool_use_id: tool_use_id.clone(),
                    subagent_type: subagent_type.clone(),
                    tool_uses,
                    total_tokens,
                    elapsed_ms,
                });

                let summary = if description.is_empty() {
                    format!("{subagent_type} · {tool_uses} tools")
                } else {
                    format!("{subagent_type} · {description}")
                };
                out.push(
                    self.event(
                        summary,
                        EventPayload::SubagentProgress {
                            tool_use_id,
                            subagent_type,
                            description,
                            tool_uses,
                            total_tokens,
                            elapsed_ms,
                        },
                    )
                    .with_raw(raw),
                );
            }

            "status" | "compact_boundary" => {
                let raw = self.stash_raw(value);
                out.push(
                    self.event(
                        format!("Runtime: {subtype}"),
                        EventPayload::RuntimeUnclassified {
                            source_type: format!("system/{subtype}"),
                        },
                    )
                    .with_raw(raw),
                );
            }

            _ => {
                let raw = self.stash_raw(value);
                out.push(
                    self.event(
                        format!("Runtime: {subtype}"),
                        EventPayload::RuntimeUnclassified {
                            source_type: format!("system/{subtype}"),
                        },
                    )
                    .with_raw(raw),
                );
            }
        }
    }

    /// Record one of the user's own hooks running.
    ///
    /// Hooks are the most invisible part of a Claude Code setup: they run silently,
    /// and a broken one quietly degrades every session with no message anywhere. So
    /// each run is kept as session metadata, and the two cases that changed what
    /// happened get more than that:
    ///
    /// - **A hook that blocked something** becomes a real `permission.denied`,
    ///   attributed to the provider — because the user's own hook decided, not
    ///   Tervin. Using the existing vocabulary rather than inventing a
    ///   Claude-Code-specific event is what keeps the event stream provider-neutral.
    /// - **A hook that failed** becomes a diagnostic, since the session is now
    ///   running differently than the user configured and nothing else would say so.
    ///
    /// Tervin's own gate is excluded: it reports its decisions directly, and counting
    /// them here would both double them and present Tervin's work as the user's
    /// configuration.
    fn ingest_hook_response(&mut self, value: &Value, out: &mut Vec<TervinEvent>) {
        let name = value
            .get("hook_name")
            .and_then(Value::as_str)
            .unwrap_or("hook")
            .to_string();
        let event_name = value
            .get("hook_event")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let exit_code = value.get("exit_code").and_then(Value::as_i64).unwrap_or(0) as i32;
        let outcome = value
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let stderr = value
            .get("stderr")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        // Tervin's gate identifies itself two ways, and it needs both. The runtime
        // echoes the hook's command line back only when the hook *blocked*; a hook that
        // merely failed carries nothing but its own stderr. Matching on the command line
        // alone therefore recognised the gate exactly when it worked and missed it every
        // time it broke — so Tervin reported its own failures as the user's hooks.
        let is_tervin = value.to_string().contains(crate::claude::hooks::HOOK_FLAG)
            || stderr
                .as_deref()
                .is_some_and(|s| s.contains(crate::claude::hooks::HOOK_STDERR_PREFIX));

        self.hook_runs.push(crate::runtime::HookRun {
            name: name.clone(),
            event: event_name,
            exit_code,
            outcome: outcome.clone(),
            message: stderr.clone(),
            is_tervin,
        });

        if is_tervin {
            return;
        }

        // Exit 2 is the runtime's only blocking code.
        if exit_code == 2 {
            let raw = self.stash_raw(value);
            let reason = stderr
                .clone()
                .unwrap_or_else(|| format!("Blocked by your `{name}` hook."));
            self.denials.push(format!("{name}: {reason}"));
            out.push(
                self.event(
                    format!("Blocked by your `{name}` hook"),
                    EventPayload::PermissionDenied {
                        request_id: None,
                        action: name.clone(),
                        // The user's own hook decided. Attributing this to Tervin
                        // would claim a decision Tervin did not make.
                        authority: DecisionAuthority::ProviderNative,
                        reason: Some(reason),
                    },
                )
                .with_raw(raw),
            );
            return;
        }

        // Anything else non-zero is a hook that did not work. The session is now
        // running differently than configured, and nothing else would say so.
        if exit_code != 0 || outcome != "success" {
            let raw = self.stash_raw(value);
            out.push(
                self.event(
                    format!("Your `{name}` hook failed (exit {exit_code})"),
                    EventPayload::DiagnosticDetected {
                        diagnostic_id: DiagnosticId::new(),
                        severity: Severity::Warning,
                        message: stderr.unwrap_or_else(|| {
                            format!("`{name}` exited {exit_code} with no explanation.")
                        }),
                        path: None,
                        line: None,
                        source: Some("claude-code/hook".to_string()),
                    },
                )
                .with_raw(raw),
            );
        }
    }

    fn ingest_assistant(&mut self, value: &Value, out: &mut Vec<TervinEvent>) {
        let message = value.get("message");
        let parent = value
            .get("parent_tool_use_id")
            .and_then(Value::as_str)
            .map(String::from);

        if let Some(usage) = message.and_then(|m| m.get("usage")) {
            self.absorb_usage(usage);
        }

        let blocks = message
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for block in blocks {
            let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
            match kind {
                "thinking" => {
                    let text = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if text.trim().is_empty() {
                        continue;
                    }
                    self.transition(ThreadState::Understanding, out);
                    out.push(self.event(
                        first_line(&text, 120),
                        EventPayload::AgentMessage {
                            text,
                            is_reasoning: true,
                            parent_tool_use_id: parent.clone(),
                        },
                    ));
                }

                "text" => {
                    let text = block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if text.trim().is_empty() {
                        continue;
                    }
                    out.push(self.event(
                        first_line(&text, 120),
                        EventPayload::AgentMessage {
                            text,
                            is_reasoning: false,
                            parent_tool_use_id: parent.clone(),
                        },
                    ));
                }

                "tool_use" => self.ingest_tool_use(&block, parent.clone(), out),

                other => {
                    out.push(self.event(
                        format!("Unrecognised content block: {other}"),
                        EventPayload::RuntimeUnclassified {
                            source_type: format!("assistant/{other}"),
                        },
                    ));
                }
            }
        }
    }

    fn ingest_tool_use(
        &mut self,
        block: &Value,
        parent: Option<String>,
        out: &mut Vec<TervinEvent>,
    ) {
        let id = block
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let input = block.get("input").cloned().unwrap_or(Value::Null);

        let command = input
            .get("command")
            .and_then(Value::as_str)
            .map(String::from);
        let path = input
            .get("file_path")
            .or_else(|| input.get("path"))
            .or_else(|| input.get("notebook_path"))
            .and_then(Value::as_str)
            .map(String::from);
        let summary = summarise_tool_input(&name, &input);

        self.pending_tools.insert(
            id.clone(),
            PendingTool {
                name: name.clone(),
                summary: summary.clone(),
                command: command.clone(),
                path: path.clone(),
                started_at: tervin_core::now(),
            },
        );

        // Every tool call is recorded, then specialised below where Tervin can
        // say something more precise than "a tool ran".
        out.push(self.event(
            format!("{name}: {summary}"),
            EventPayload::ToolRequested {
                tool_use_id: id.clone(),
                tool_name: name.clone(),
                input_summary: summary.clone(),
                parent_tool_use_id: parent.clone(),
            },
        ));

        match name.as_str() {
            "Bash" | "BashOutput" => {
                let Some(cmd) = command else { return };
                let risk = rules_engine::classify(&cmd, &self.cwd);
                let is_test = looks_like_tests(&cmd);

                // The runtime executes this itself, so Tervin observes rather
                // than gates it. Saying otherwise would misrepresent the gate.
                let mut risk = risk;
                risk.enforceable = false;

                out.push(self.event(
                    format!("$ {}", first_line(&cmd, 120)),
                    EventPayload::CommandProposed {
                        command: cmd.clone(),
                        cwd: Some(self.cwd.clone()),
                        risk,
                    },
                ));
                out.push(self.event(
                    format!("$ {}", first_line(&cmd, 120)),
                    EventPayload::CommandStarted {
                        command: cmd.clone(),
                        block_id: None,
                    },
                ));

                if is_test {
                    self.transition(ThreadState::Testing, out);
                    out.push(self.event(
                        format!("Running tests: {}", first_line(&cmd, 80)),
                        EventPayload::TestStarted {
                            suite: cmd.clone(),
                            block_id: None,
                        },
                    ));
                } else {
                    self.transition(ThreadState::Executing, out);
                }
            }

            "Read" | "NotebookRead" => {
                self.transition(ThreadState::Reading, out);
                if let Some(p) = path {
                    out.push(self.event_with_links(
                        format!("Read {}", short_path(&p)),
                        EventPayload::FileRead {
                            path: p.clone(),
                            lines: input.get("limit").and_then(Value::as_u64).map(|n| n as u32),
                        },
                        vec![Link::File {
                            path: p,
                            line: None,
                        }],
                    ));
                }
            }

            "Glob" | "Grep" | "WebSearch" | "WebFetch" => {
                self.transition(ThreadState::Reading, out);
            }

            "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
                self.transition(ThreadState::Editing, out);
                if let Some(p) = path {
                    let kind = if name == "Write" {
                        FileChangeKind::Created
                    } else {
                        FileChangeKind::Modified
                    };
                    out.push(self.event_with_links(
                        format!("Proposed change to {}", short_path(&p)),
                        EventPayload::PatchProposed {
                            files: vec![FileChange {
                                path: p.clone(),
                                kind,
                                added_lines: None,
                                removed_lines: None,
                            }],
                            unified_diff: None,
                        },
                        vec![Link::File {
                            path: p,
                            line: None,
                        }],
                    ));
                }
            }

            // The agent's todo list is the closest thing it has to a plan.
            "TodoWrite" => {
                let steps: Vec<PlanStep> = input
                    .get("todos")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| {
                                Some(PlanStep {
                                    description: t.get("content")?.as_str()?.to_string(),
                                    touches: Vec::new(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if !steps.is_empty() {
                    self.transition(ThreadState::Planning, out);
                    out.push(self.event(
                        format!("Plan updated · {} steps", steps.len()),
                        EventPayload::PlanProposed {
                            steps,
                            raw_text: None,
                        },
                    ));
                }
            }

            // Leaving plan mode is an explicit request for approval.
            "ExitPlanMode" => {
                let raw_text = input.get("plan").and_then(Value::as_str).map(String::from);
                let steps = raw_text
                    .as_deref()
                    .map(parse_plan_steps)
                    .unwrap_or_default();
                out.push(self.event(
                    "Plan ready for approval",
                    EventPayload::PlanProposed { steps, raw_text },
                ));
                self.transition(ThreadState::WaitingForPermission, out);
            }

            "Task" => {
                self.transition(ThreadState::Understanding, out);
            }

            _ => {
                // An MCP tool, a plugin tool, or something new. Recorded by the
                // generic tool.requested above; Tervin says nothing more because
                // it knows nothing more.
                self.transition(ThreadState::WaitingForExternalTool, out);
            }
        }
    }

    fn ingest_user(&mut self, value: &Value, out: &mut Vec<TervinEvent>) {
        let blocks = value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        // `tool_use_result` sits beside the message and carries the streams for
        // Bash calls, which the content block alone does not.
        let structured = value.get("tool_use_result");

        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let id = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let is_error = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let content = tool_result_text(&block);

            let pending = self.pending_tools.remove(&id);
            let (name, summary, command, path, started_at) = match pending {
                Some(p) => (p.name, p.summary, p.command, p.path, Some(p.started_at)),
                None => ("tool".to_string(), String::new(), None, None, None),
            };
            let duration_ms =
                started_at.map(|t| (tervin_core::now() - t).num_milliseconds().max(0) as u64);

            out.push(self.event(
                format!("{name} {}", if is_error { "failed" } else { "completed" }),
                EventPayload::ToolCompleted {
                    tool_use_id: id.clone(),
                    tool_name: name.clone(),
                    is_error,
                    output_summary: first_line(&content, 200),
                    duration_ms,
                },
            ));

            // A subagent's `Task` returning is the only signal it has finished; the
            // runtime reports progress but never completion. Said explicitly, with
            // what it cost, so the Thread visibly becomes its parent's again rather
            // than leaving a subagent that appears to still be running.
            if self.subagent.as_ref().is_some_and(|s| s.tool_use_id == id) {
                let run = self.subagent.take().expect("checked just above");
                out.push(self.event(
                    format!(
                        "{} finished · {} tools · {} tokens",
                        run.subagent_type, run.tool_uses, run.total_tokens
                    ),
                    EventPayload::SubagentFinished {
                        tool_use_id: run.tool_use_id,
                        subagent_type: run.subagent_type,
                        tool_uses: run.tool_uses,
                        total_tokens: run.total_tokens,
                        elapsed_ms: run.elapsed_ms,
                    },
                ));
            }

            match name.as_str() {
                "Bash" | "BashOutput" => {
                    let cmd = command.unwrap_or_default();
                    let stdout = structured
                        .and_then(|s| s.get("stdout"))
                        .and_then(Value::as_str)
                        .unwrap_or(&content);
                    let interrupted = structured
                        .and_then(|s| s.get("interrupted"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);

                    if !stdout.trim().is_empty() {
                        out.push(self.event(
                            first_line(stdout, 120),
                            EventPayload::CommandOutput {
                                stream: tervin_core::events::OutputStream::Stdout,
                                excerpt: truncate(stdout, 4000),
                                block_id: None,
                            },
                        ));
                    }

                    // Claude Code reports success or failure, not an exit status.
                    // The code below is derived, and the summary says so rather
                    // than presenting a fabricated number as fact.
                    let exit_code = if interrupted {
                        130
                    } else if is_error {
                        1
                    } else {
                        0
                    };
                    out.push(self.event(
                        format!(
                            "$ {} — {}",
                            first_line(&cmd, 80),
                            if interrupted {
                                "interrupted"
                            } else if is_error {
                                "reported as failed (exit status not reported by the runtime)"
                            } else {
                                "reported as succeeded"
                            }
                        ),
                        EventPayload::CommandCompleted {
                            command: cmd.clone(),
                            exit_code,
                            duration_ms: duration_ms.unwrap_or(0),
                            // Derived above from `is_error` and `interrupted`. Claude Code
                            // never reports a status, so a Block from this shows none.
                            exit_code_reported: false,
                            block_id: None,
                        },
                    ));

                    if looks_like_tests(&cmd) {
                        let summary = block_engine::parse::extract(stdout, &self.cwd).tests;
                        let (passed, failed, skipped) = summary
                            .as_ref()
                            .map(|t| (t.passed, t.failed, t.skipped))
                            .unwrap_or((0, 0, 0));
                        let outcome = if is_error || failed > 0 {
                            TestOutcome::Failed
                        } else {
                            TestOutcome::Passed
                        };
                        out.push(self.event(
                            match outcome {
                                TestOutcome::Passed => format!("Tests passed ({passed})"),
                                _ => format!("Tests failed ({failed} failing)"),
                            },
                            EventPayload::TestCompleted {
                                suite: cmd,
                                outcome,
                                passed,
                                failed,
                                skipped,
                                duration_ms,
                                block_id: None,
                            },
                        ));
                    }

                    // Compiler and linter output in the result is worth surfacing
                    // as diagnostics regardless of what the command was.
                    for d in block_engine::parse::extract(stdout, &self.cwd).diagnostics {
                        if d.severity != Severity::Error {
                            continue;
                        }
                        let links = d
                            .path
                            .clone()
                            .map(|p| {
                                vec![Link::File {
                                    path: p,
                                    line: d.line,
                                }]
                            })
                            .unwrap_or_default();
                        out.push(self.event_with_links(
                            first_line(&d.message, 120),
                            EventPayload::DiagnosticDetected {
                                diagnostic_id: DiagnosticId::new(),
                                severity: d.severity,
                                message: d.message,
                                path: d.path,
                                line: d.line,
                                source: d.source,
                            },
                            links,
                        ));
                    }
                }

                "Edit" | "Write" | "MultiEdit" | "NotebookEdit" if !is_error => {
                    if let Some(p) = path {
                        let kind = if name == "Write" {
                            FileChangeKind::Created
                        } else {
                            FileChangeKind::Modified
                        };
                        out.push(self.event_with_links(
                            format!("Changed {}", short_path(&p)),
                            EventPayload::PatchApplied {
                                files: vec![FileChange {
                                    path: p.clone(),
                                    kind,
                                    added_lines: None,
                                    removed_lines: None,
                                }],
                                // The runtime applied this itself; Tervin observed
                                // it rather than authorising it.
                                authority: DecisionAuthority::ProviderNative,
                            },
                            vec![Link::File {
                                path: p,
                                line: None,
                            }],
                        ));
                    }
                }

                _ => {}
            }

            // A refusal reads as an error result mentioning permission.
            if is_error && mentions_permission(&content) {
                self.denials.push(summary.clone());
                out.push(self.event(
                    format!("{name} was not permitted"),
                    EventPayload::PermissionDenied {
                        request_id: None,
                        action: summary,
                        authority: DecisionAuthority::ProviderNative,
                        reason: Some(first_line(&content, 200)),
                    },
                ));
                self.transition(ThreadState::WaitingForPermission, out);
            }
        }
    }

    fn ingest_result(&mut self, value: &Value, out: &mut Vec<TervinEvent>) {
        if let Some(usage) = value.get("usage") {
            self.absorb_usage(usage);
        }
        if let Some(cost) = value.get("total_cost_usd").and_then(Value::as_f64) {
            self.cost.total_cost_usd = Some(cost);
        }
        if let Some(models) = value.get("modelUsage").and_then(Value::as_object) {
            if let Some((_, first)) = models.iter().next() {
                self.cost.context_window = first.get("contextWindow").and_then(Value::as_u64);
            }
        }
        out.push(self.event(
            "Usage updated",
            EventPayload::CostUpdated {
                snapshot: self.cost.clone(),
            },
        ));

        // Anything the runtime refused is reported, so a Thread that looks
        // finished cannot hide that it was blocked part-way.
        if let Some(denials) = value.get("permission_denials").and_then(Value::as_array) {
            for denial in denials {
                let tool = denial
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                let detail = denial
                    .get("tool_input")
                    .map(|i| summarise_tool_input(tool, i))
                    .unwrap_or_default();
                self.denials.push(format!("{tool}: {detail}"));
                out.push(self.event(
                    format!("{tool} was not permitted"),
                    EventPayload::PermissionDenied {
                        request_id: None,
                        action: format!("{tool}: {detail}"),
                        authority: DecisionAuthority::ProviderNative,
                        reason: Some(
                            "Refused by the agent runtime's own permission system.".to_string(),
                        ),
                    },
                ));
            }
        }

        let is_error = value
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let subtype = value.get("subtype").and_then(Value::as_str).unwrap_or("");
        let duration_ms = value.get("duration_ms").and_then(Value::as_u64);
        let text = value
            .get("result")
            .and_then(Value::as_str)
            .map(String::from);

        let raw = self.stash_raw(value);
        if is_error || subtype.contains("error") {
            self.state = ThreadState::Failed;
            let detail = value
                .get("api_error_status")
                .and_then(Value::as_str)
                .or(text.as_deref())
                .unwrap_or(subtype);

            // An expired login is the most common way a working setup stops
            // working, and as a bare "Failed:" it reads as a broken Tervin. Name
            // the cause and the fix instead: nothing here is recoverable by
            // retrying, only by signing in.
            let (summary, reason, recoverable) = match self.auth_failure_hint(detail) {
                Some(hint) => (
                    format!("Sign-in needed: {detail}"),
                    format!("{detail}\n\n{hint}"),
                    Some(true),
                ),
                None => (
                    format!("Failed: {detail}"),
                    text.clone().unwrap_or_else(|| subtype.to_string()),
                    Some(subtype == "error_max_turns"),
                ),
            };

            out.push(
                self.event(
                    summary,
                    EventPayload::ThreadFailed {
                        reason,
                        recoverable,
                    },
                )
                .with_raw(raw),
            );
        } else {
            // Work is done, but if files changed a human still has to look.
            self.state = ThreadState::Completed;
            out.push(
                self.event(
                    "Completed",
                    EventPayload::ThreadCompleted {
                        result: text,
                        duration_ms,
                        cost: Some(self.cost.clone()),
                    },
                )
                .with_raw(raw),
            );
        }
    }

    fn absorb_usage(&mut self, usage: &Value) {
        let get = |k: &str| usage.get(k).and_then(Value::as_u64);
        // Accumulate rather than overwrite: usage arrives per message.
        if let Some(v) = get("input_tokens") {
            self.cost.input_tokens = Some(self.cost.input_tokens.unwrap_or(0).max(v));
        }
        if let Some(v) = get("output_tokens") {
            self.cost.output_tokens = Some(self.cost.output_tokens.unwrap_or(0).max(v));
        }
        if let Some(v) = get("cache_read_input_tokens") {
            self.cost.cache_read_tokens = Some(self.cost.cache_read_tokens.unwrap_or(0).max(v));
        }
        if let Some(v) = get("cache_creation_input_tokens") {
            self.cost.cache_write_tokens = Some(self.cost.cache_write_tokens.unwrap_or(0).max(v));
        }
        let used = self.cost.input_tokens.unwrap_or(0)
            + self.cost.cache_read_tokens.unwrap_or(0)
            + self.cost.cache_write_tokens.unwrap_or(0);
        if used > 0 {
            self.cost.context_used = Some(used);
        }
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

    /// Recognise an authentication failure and say how to fix it.
    ///
    /// This is the single most common way a working setup stops working, and as a
    /// bare "Failed: API Error: 401" it reads as a bug in Tervin. Detection is on
    /// the message text because the runtime does not give authentication its own
    /// `subtype` — so the match is deliberately narrow, and anything unrecognised
    /// keeps the plain error rather than being given advice that might be wrong.
    fn auth_failure_hint(&self, detail: &str) -> Option<String> {
        let lower = detail.to_ascii_lowercase();
        let is_auth = lower.contains("401")
            || lower.contains("oauth")
            || lower.contains("failed to authenticate")
            || lower.contains("authentication_error")
            || lower.contains("invalid api key")
            || lower.contains("invalid bearer token")
            || (lower.contains("token") && lower.contains("expired"));
        if !is_auth {
            return None;
        }

        let account = self
            .account_hint
            .clone()
            .unwrap_or_else(|| "an account Tervin could not identify".to_string());
        Some(format!(
            "This is a sign-in problem, not a Tervin failure: the agent started and \
             answered, but the credentials it used are no longer valid.\n\n\
             This Thread ran as {}.\n\n\
             Either pick a profile for the account you actually use — Settings › Agents \
             lists the ones Tervin found from your shell aliases — or sign that account \
             in by running `claude` then `/login` in a pane. Start a new Thread once it \
             succeeds.",
            account
        ))
    }

    /// The account this Thread was launched against, for a sign-in message that
    /// names the right one.
    ///
    /// Set from the launch environment rather than guessed, because with several
    /// profiles configured, "sign in again" is useless without knowing which.
    pub fn set_account_hint(&mut self, hint: Option<String>) {
        self.account_hint = hint;
    }

    /// Record an interruption requested from Tervin.
    pub fn interrupted(&mut self) -> Vec<TervinEvent> {
        self.state = ThreadState::Interrupted;
        vec![self.event(
            "Stopped by user",
            EventPayload::ThreadFailed {
                reason: "Interrupted from Tervin.".to_string(),
                recoverable: Some(true),
            },
        )]
    }

    /// Record that the process ended without a result message.
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

    /// A permission decision Tervin itself made, when the runtime asked.
    pub fn tervin_decision(&mut self, tool: &str, allowed: bool, reason: &str) -> Vec<TervinEvent> {
        let mut out = Vec::new();
        if allowed {
            out.push(self.event(
                format!("{tool} approved"),
                EventPayload::PermissionGranted {
                    request_id: None,
                    action: tool.to_string(),
                    authority: DecisionAuthority::Tervin,
                    scope: reason.to_string(),
                },
            ));
        } else {
            out.push(self.event(
                format!("{tool} denied"),
                EventPayload::PermissionDenied {
                    request_id: None,
                    action: tool.to_string(),
                    authority: DecisionAuthority::Tervin,
                    reason: Some(reason.to_string()),
                },
            ));
        }
        out
    }

    /// Mark that a plan was approved and work may continue.
    pub fn plan_approved(&mut self) -> Vec<TervinEvent> {
        let mut out = vec![self.event(
            "Plan approved",
            EventPayload::PlanApproved {
                authority: DecisionAuthority::Tervin,
            },
        )];
        self.transition(ThreadState::Executing, &mut out);
        out
    }

    /// A risk assessment for an action Tervin observed but could not gate.
    pub fn observed_risk(&self, command: &str) -> RiskAssessment {
        let mut risk = rules_engine::classify(command, &self.cwd);
        risk.enforceable = false;
        risk
    }
}

// ---------------------------------------------------------------- helpers

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// A compact, human-readable rendering of a tool's arguments.
///
/// Never a raw JSON dump in the common cases: a timeline row has to be scannable.
pub fn summarise_tool_input(name: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(Value::as_str);

    match name {
        "Bash" | "BashOutput" => s("command").map(|c| first_line(c, 160).to_string()),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "NotebookRead" => {
            s("file_path").or(s("notebook_path")).map(short_path)
        }
        "Glob" => s("pattern").map(String::from),
        "Grep" => s("pattern").map(|p| format!("/{p}/")),
        "WebFetch" | "WebSearch" => s("url").or(s("query")).map(String::from),
        "Task" => s("description").map(String::from),
        "TodoWrite" => input
            .get("todos")
            .and_then(Value::as_array)
            .map(|a| format!("{} items", a.len())),
        _ => None,
    }
    .unwrap_or_else(|| {
        // Unknown tool: show something bounded rather than nothing.
        let compact = input.to_string();
        if compact == "null" {
            String::new()
        } else {
            truncate(&compact, 160)
        }
    })
}

/// Extract text from a `tool_result` block, which may be a string or blocks.
fn tool_result_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn mentions_permission(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("permission")
        || lower.contains("not allowed")
        || lower.contains("denied")
        || lower.contains("requires approval")
}

/// Whether a command is running tests, so the Thread can say "Testing".
pub fn looks_like_tests(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    const MARKERS: [&str; 12] = [
        "cargo test",
        "cargo nextest",
        "npm test",
        "npm run test",
        "pnpm test",
        "yarn test",
        "jest",
        "vitest",
        "pytest",
        "go test",
        "mvn test",
        "gradle test",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

/// Split a plan's markdown into steps, one per bullet or numbered line.
fn parse_plan_steps(text: &str) -> Vec<PlanStep> {
    text.lines()
        .map(str::trim)
        .filter_map(|line| {
            let stripped = line
                .strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .or_else(|| {
                    line.split_once(". ")
                        .and_then(|(n, rest)| n.chars().all(|c| c.is_ascii_digit()).then_some(rest))
                })?;
            let step = stripped.trim();
            (!step.is_empty()).then(|| PlanStep {
                description: step.to_string(),
                touches: Vec::new(),
            })
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
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

/// Shorten a path for display, keeping the last two components.
fn short_path(path: impl AsRef<str>) -> String {
    let path = path.as_ref();
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
    use tervin_core::Tier;

    fn normalizer() -> Normalizer {
        Normalizer::new(
            ThreadId::new(),
            AgentIdentity::new("claude-code", "Claude Code", Tier::Structured),
            "/Users/dev/proj",
        )
    }

    fn kinds(events: &[TervinEvent]) -> Vec<&'static str> {
        events.iter().map(|e| e.kind()).collect()
    }

    /// The transcript captured from the real CLI, so these tests are pinned to
    /// observed behaviour rather than an assumed schema.
    fn fixture() -> Vec<Value> {
        include_str!("../../tests/fixtures/claude_stream_sample.jsonl")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    #[test]
    fn normalises_a_real_captured_session() {
        let mut n = normalizer();
        let mut all = Vec::new();
        for msg in fixture() {
            all.extend(n.ingest(&msg));
        }

        let seen = kinds(&all);
        for expected in [
            "thread.started",
            "tool.requested",
            "command.proposed",
            "command.started",
            "tool.completed",
            "command.completed",
            "agent.message",
            "cost.updated",
            "thread.completed",
        ] {
            assert!(seen.contains(&expected), "missing {expected} in {seen:?}");
        }

        assert_eq!(n.state(), ThreadState::Completed);
        assert_eq!(n.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(n.runtime_version.as_deref(), Some("2.1.220"));
        assert!(n.resume_id.is_some(), "resume id must be captured");
        assert!(n.cost.total_cost_usd.unwrap_or(0.0) > 0.0);
        assert!(!n.tools.is_empty());
        assert!(!n.mcp_servers.is_empty());
    }

    #[test]
    fn every_event_carries_its_thread_and_location() {
        let mut n = normalizer();
        let mut all = Vec::new();
        for msg in fixture() {
            all.extend(n.ingest(&msg));
        }
        assert!(!all.is_empty());
        for e in &all {
            assert!(e.thread_id.is_some(), "{} lacked a thread id", e.kind());
            assert!(!e.summary.is_empty(), "{} lacked a summary", e.kind());
            assert!(e.cwd.is_some(), "{} lacked a cwd", e.kind());
        }
    }

    #[test]
    fn a_bash_tool_call_is_risk_classified_but_marked_unenforceable() {
        // Tervin can see what the runtime is about to run, but cannot stop it.
        // Presenting that as an enforced gate would be a lie.
        let mut n = normalizer();
        let msg = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "id": "t1", "name": "Bash",
                "input": {"command": "rm -rf /"}
            }]}
        });
        let events = n.ingest(&msg);
        let proposed = events
            .iter()
            .find_map(|e| match &e.payload {
                EventPayload::CommandProposed { risk, .. } => Some(risk),
                _ => None,
            })
            .expect("no command.proposed");

        assert_eq!(proposed.level, tervin_core::RiskLevel::Critical);
        assert!(
            !proposed.enforceable,
            "an observed action must not claim to be gated"
        );
    }

    #[test]
    fn unknown_message_types_are_kept_not_dropped() {
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({"type": "something_new_in_v9"}));
        assert_eq!(kinds(&events), vec!["runtime.unclassified"]);
        // The raw payload is retained so it can be inspected.
        assert!(events[0].raw.is_some());
        assert_eq!(n.raw_sink.len(), 1);
    }

    #[test]
    fn unknown_content_blocks_are_kept_not_dropped() {
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "future_block", "data": 1}]}
        }));
        assert_eq!(kinds(&events), vec!["runtime.unclassified"]);
    }

    #[test]
    fn thinking_blocks_are_marked_as_reasoning() {
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "thinking", "thinking": "considering options"}]}
        }));
        let reasoning = events.iter().any(|e| {
            matches!(
                &e.payload,
                EventPayload::AgentMessage {
                    is_reasoning: true,
                    ..
                }
            )
        });
        assert!(
            reasoning,
            "reasoning must be distinguishable so it can stay collapsed"
        );
    }

    #[test]
    fn empty_thinking_blocks_produce_no_event() {
        // The real stream emits an empty thinking block before tool calls.
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "thinking", "thinking": ""}]}
        }));
        assert!(events.is_empty(), "got {:?}", kinds(&events));
    }

    #[test]
    fn test_commands_produce_test_events_with_parsed_counts() {
        let mut n = normalizer();
        n.ingest(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "id": "t1", "name": "Bash",
                "input": {"command": "cargo test --all"}
            }]}
        }));
        assert_eq!(n.state(), ThreadState::Testing);

        let events = n.ingest(&serde_json::json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result", "tool_use_id": "t1", "is_error": false,
                "content": "test result: ok. 12 passed; 0 failed; 1 ignored"
            }]},
            "tool_use_result": {"stdout": "test result: ok. 12 passed; 0 failed; 1 ignored", "stderr": ""}
        }));

        let completed = events
            .iter()
            .find_map(|e| match &e.payload {
                EventPayload::TestCompleted {
                    passed,
                    failed,
                    outcome,
                    ..
                } => Some((*passed, *failed, *outcome)),
                _ => None,
            })
            .expect("no test.completed");
        assert_eq!(completed, (12, 0, TestOutcome::Passed));
    }

    #[test]
    fn a_failing_command_reports_that_the_exit_status_was_not_given() {
        // Claude Code reports success or failure, not an exit code. The derived
        // value must not be presented as something the runtime stated.
        let mut n = normalizer();
        n.ingest(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "id": "t1", "name": "Bash",
                "input": {"command": "false"}
            }]}
        }));
        let events = n.ingest(&serde_json::json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": "boom"
            }]}
        }));
        let completed = events
            .iter()
            .find(|e| matches!(e.payload, EventPayload::CommandCompleted { .. }))
            .expect("no command.completed");
        assert!(
            completed.summary.contains("not reported"),
            "summary should disclose the derivation: {}",
            completed.summary
        );
    }

    #[test]
    fn file_edits_become_patch_events_linked_to_the_file() {
        let mut n = normalizer();
        n.ingest(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "id": "e1", "name": "Edit",
                "input": {"file_path": "/Users/dev/proj/src/lib.rs"}
            }]}
        }));
        assert_eq!(n.state(), ThreadState::Editing);

        let events = n.ingest(&serde_json::json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result", "tool_use_id": "e1", "is_error": false, "content": "ok"
            }]}
        }));
        let applied = events
            .iter()
            .find(|e| matches!(e.payload, EventPayload::PatchApplied { .. }))
            .expect("no patch.applied");
        // Applied by the runtime, not authorised by Tervin.
        match &applied.payload {
            EventPayload::PatchApplied { authority, .. } => {
                assert_eq!(*authority, DecisionAuthority::ProviderNative)
            }
            _ => unreachable!(),
        }
        assert!(matches!(applied.links.first(), Some(Link::File { .. })));
    }

    #[test]
    fn exit_plan_mode_parses_steps_and_waits_for_permission() {
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "id": "p1", "name": "ExitPlanMode",
                "input": {"plan": "Here is the plan:\n- Add a parser\n- Wire it up\n2. Write tests"}
            }]}
        }));
        let steps = events
            .iter()
            .find_map(|e| match &e.payload {
                EventPayload::PlanProposed { steps, .. } => Some(steps.clone()),
                _ => None,
            })
            .expect("no plan.proposed");
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].description, "Add a parser");
        assert_eq!(n.state(), ThreadState::WaitingForPermission);
    }

    #[test]
    fn permission_denials_in_the_result_are_surfaced() {
        // A Thread that was blocked must not look like a clean completion.
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "permission_denials": [{
                "tool_name": "Write",
                "tool_input": {"file_path": "/etc/hosts", "content": "x"}
            }],
            "total_cost_usd": 0.01
        }));
        let denied = events
            .iter()
            .find(|e| matches!(e.payload, EventPayload::PermissionDenied { .. }))
            .expect("denial not surfaced");
        match &denied.payload {
            EventPayload::PermissionDenied { authority, .. } => {
                assert_eq!(*authority, DecisionAuthority::ProviderNative)
            }
            _ => unreachable!(),
        }
        assert_eq!(n.denials.len(), 1);
    }

    #[test]
    fn an_expired_login_is_reported_as_a_sign_in_problem_with_the_fix() {
        // The most common way a working setup stops working. As a bare
        // "Failed: API Error: 401" it reads as a bug in Tervin.
        let mut n = normalizer();
        n.set_account_hint(Some("the account in ~/.claude-work".to_string()));
        let events = n.ingest(&serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "result": "Failed to authenticate. API Error: 401 OAuth access token has expired. \
                       Re-authenticate to continue."
        }));

        let failed = events
            .iter()
            .find(|e| e.kind() == "thread.failed")
            .expect("no thread.failed");
        assert!(
            failed.summary.starts_with("Sign-in needed:"),
            "summary was {}",
            failed.summary
        );

        match &failed.payload {
            EventPayload::ThreadFailed {
                reason,
                recoverable,
            } => {
                // Names the account, says what to run, and keeps the original error.
                assert!(reason.contains("~/.claude-work"), "{reason}");
                assert!(reason.contains("/login"), "{reason}");
                // And points at the fix that is usually the right one.
                assert!(reason.contains("Settings"), "{reason}");
                assert!(
                    reason.contains("401"),
                    "the original error must survive: {reason}"
                );
                assert_eq!(*recoverable, Some(true));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn an_ordinary_failure_is_not_given_sign_in_advice() {
        // Detection is on message text, so it has to stay narrow: advice that does
        // not apply is worse than no advice.
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "result", "subtype": "success", "is_error": true,
            "result": "Tool execution failed: file not found"
        }));
        let failed = events.iter().find(|e| e.kind() == "thread.failed").unwrap();
        assert!(failed.summary.starts_with("Failed:"), "{}", failed.summary);
        match &failed.payload {
            EventPayload::ThreadFailed { reason, .. } => {
                assert!(!reason.contains("/login"), "{reason}")
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_sign_in_message_never_contains_a_credential() {
        // An error message is exactly the wrong place for a secret to surface.
        let config = crate::runtime::LaunchConfig {
            env: vec![(
                "ANTHROPIC_API_KEY".to_string(),
                "sk-ant-super-secret".to_string(),
            )],
            ..crate::runtime::LaunchConfig::new(ThreadId::new(), "/tmp")
        };
        let hint = super::super::account_hint(&config).expect("no hint");
        assert!(!hint.contains("sk-ant-super-secret"), "{hint}");
        assert!(hint.contains("ANTHROPIC_API_KEY"), "{hint}");

        let mut n = normalizer();
        n.set_account_hint(Some(hint));
        let events = n.ingest(&serde_json::json!({
            "type": "result", "subtype": "success", "is_error": true,
            "result": "API Error: 401 authentication_error"
        }));
        let failed = events.iter().find(|e| e.kind() == "thread.failed").unwrap();
        match &failed.payload {
            EventPayload::ThreadFailed { reason, .. } => {
                assert!(!reason.contains("sk-ant-super-secret"), "{reason}")
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_profile_with_no_config_dir_says_it_runs_the_default_account() {
        // The trap: a profile that sets nothing runs `~/.claude`, which is often not
        // the account the user signs into from their shell. "Re-authenticate" alone
        // sends them to fix the wrong account.
        let config = crate::runtime::LaunchConfig::new(ThreadId::new(), "/tmp");
        let hint = super::super::account_hint(&config).expect("no hint");
        assert!(hint.contains("default account"), "{hint}");
        assert!(hint.contains("CLAUDE_CONFIG_DIR"), "{hint}");
    }

    #[test]
    fn a_config_dir_profile_is_named_over_a_generic_account() {
        let config = crate::runtime::LaunchConfig {
            env: vec![
                (
                    "CLAUDE_CONFIG_DIR".to_string(),
                    "/Users/dev/.claude-work".to_string(),
                ),
                ("ANTHROPIC_API_KEY".to_string(), "sk-x".to_string()),
            ],
            ..crate::runtime::LaunchConfig::new(ThreadId::new(), "/tmp")
        };
        let hint = super::super::account_hint(&config).unwrap();
        assert!(hint.contains("/Users/dev/.claude-work"), "{hint}");
    }

    #[test]
    fn a_hook_that_starts_produces_no_event() {
        // Recording the start as well as the outcome would double every hook in the
        // timeline for no added information.
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "system", "subtype": "hook_started",
            "hook_id": "abc", "hook_name": "SessionStart:startup",
            "hook_event": "SessionStart"
        }));
        assert!(events.is_empty());
    }

    #[test]
    fn a_successful_hook_is_recorded_without_cluttering_the_timeline() {
        // Shape taken from a real `--include-hook-events` stream.
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "system", "subtype": "hook_response",
            "hook_id": "8b8684da", "hook_name": "SessionStart:startup",
            "hook_event": "SessionStart",
            "stdout": "{\"continue\":true,\"suppressOutput\":true}\n",
            "stderr": "", "exit_code": 0, "outcome": "success"
        }));
        assert!(events.is_empty(), "a working hook is not news: {events:?}");

        assert_eq!(n.hook_runs.len(), 1);
        let run = &n.hook_runs[0];
        assert_eq!(run.name, "SessionStart:startup");
        assert_eq!(run.event, "SessionStart");
        assert_eq!(run.exit_code, 0);
        assert!(!run.is_tervin);
        assert!(run.message.is_none(), "empty stderr should not be stored");
    }

    #[test]
    fn a_hook_that_blocks_becomes_a_provider_native_denial() {
        // The user's own hook decided. Attributing it to Tervin would claim a
        // decision Tervin did not make — and the timeline has to say who did.
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "system", "subtype": "hook_response",
            "hook_name": "PreToolUse:Bash", "hook_event": "PreToolUse",
            "stderr": "Blocked: no writes to production.",
            "exit_code": 2, "outcome": "blocked"
        }));

        let denied = events
            .iter()
            .find(|e| e.kind() == "permission.denied")
            .expect("no permission.denied");
        match &denied.payload {
            EventPayload::PermissionDenied {
                action,
                authority,
                reason,
                ..
            } => {
                assert_eq!(action, "PreToolUse:Bash");
                assert_eq!(*authority, DecisionAuthority::ProviderNative);
                assert!(reason.as_deref().is_some_and(|r| r.contains("production")));
            }
            other => panic!("got {other:?}"),
        }
        // And it shows up as a denial on the session, alongside the runtime's own.
        assert!(n.denials.iter().any(|d| d.contains("production")));
    }

    #[test]
    fn a_failing_hook_is_reported_because_nothing_else_would_say_so() {
        // A broken hook silently changes how every session behaves.
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "system", "subtype": "hook_response",
            "hook_name": "PostToolUse:Edit", "hook_event": "PostToolUse",
            "stderr": "prettier: command not found",
            "exit_code": 127, "outcome": "error"
        }));

        let diagnostic = events
            .iter()
            .find(|e| e.kind() == "diagnostic.detected")
            .expect("no diagnostic");
        assert!(
            diagnostic.summary.contains("PostToolUse:Edit"),
            "the failing hook must be named: {}",
            diagnostic.summary
        );
        match &diagnostic.payload {
            EventPayload::DiagnosticDetected {
                message, source, ..
            } => {
                assert!(message.contains("prettier"));
                assert_eq!(source.as_deref(), Some("claude-code/hook"));
            }
            other => panic!("got {other:?}"),
        }
        // A failure is not a block: the tool still ran.
        assert!(!events.iter().any(|e| e.kind() == "permission.denied"));
    }

    #[test]
    fn tervins_own_gate_is_not_double_reported_as_a_user_hook() {
        // The gate emits its own events. Counting them here would both duplicate
        // them and present Tervin's work as the user's configuration.
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "system", "subtype": "hook_response",
            "hook_name": "PreToolUse:Bash", "hook_event": "PreToolUse",
            "stderr": "PreToolUse:Bash hook error: [/Apps/Tervin --tervin-hook /run/h.sock]: \
                       Denied by Tervin Rules: irreversible",
            "exit_code": 2, "outcome": "blocked"
        }));
        assert!(
            events.is_empty(),
            "Tervin's gate reports itself: {events:?}"
        );

        // Still recorded, and marked so the UI can tell them apart.
        assert_eq!(n.hook_runs.len(), 1);
        assert!(n.hook_runs[0].is_tervin);
        assert!(n.denials.is_empty(), "the gate records its own denials");
    }

    /// Shape copied from a real session, not invented.
    fn task_progress(tools: u64, tokens: u64, description: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "system", "subtype": "task_progress",
            "description": description,
            "last_tool_name": "Read",
            "session_id": "9443933a-9e17-4bee-9255-59879e365cca",
            "subagent_type": "Explore",
            "task_id": "a91691aff493eeff9",
            "tool_use_id": "toolu_01YCTLiAk7kgCRN5orFTFBAf",
            "usage": {"duration_ms": 22979, "tool_uses": tools, "total_tokens": tokens},
            "uuid": "e642c7f2-48b1-47af-a258-bedaca5ebf93"
        })
    }

    #[test]
    fn a_working_subagent_is_reported_rather_than_discarded() {
        // The case this exists for. A `Task` hands off to a subagent that can read
        // twenty files over several minutes, and every one of those was thrown away
        // as unclassified. The timeline showed the parent's single call and then
        // nothing, so a Thread doing plenty of work read as a Thread that had died —
        // which is how it was read, and nudged, and reported as stopped.
        let mut n = normalizer();
        let events = n.ingest(&task_progress(10, 157251, "Reading ThreadPanel.tsx"));

        let progress = events
            .iter()
            .find(|e| e.kind() == "subagent.progress")
            .expect("a subagent at work is news");
        match &progress.payload {
            EventPayload::SubagentProgress {
                subagent_type,
                description,
                tool_uses,
                total_tokens,
                tool_use_id,
                ..
            } => {
                assert_eq!(subagent_type, "Explore");
                assert_eq!(description, "Reading ThreadPanel.tsx");
                assert_eq!(*tool_uses, 10);
                assert_eq!(*total_tokens, 157251);
                // Carries its parent, so its work can be attributed rather than
                // appearing to be the main agent's.
                assert_eq!(tool_use_id, "toolu_01YCTLiAk7kgCRN5orFTFBAf");
            }
            other => panic!("got {other:?}"),
        }
        assert!(
            !events.iter().any(|e| e.kind() == "runtime.unclassified"),
            "a subagent is not an unclassified runtime message"
        );
    }

    #[test]
    fn a_subagent_is_reported_as_finished_when_its_task_returns() {
        // The runtime announces a subagent's progress and never its completion. Left
        // alone, the last thing the timeline says is that a subagent was working,
        // which is indistinguishable from one still working an hour later.
        let mut n = normalizer();
        n.ingest(&task_progress(10, 157251, "Reading ThreadPanel.tsx"));
        n.ingest(&serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "id": "toolu_01YCTLiAk7kgCRN5orFTFBAf",
                "name": "Task", "input": {"description": "explore"}
            }]}
        }));

        let events = n.ingest(&serde_json::json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_01YCTLiAk7kgCRN5orFTFBAf",
                "content": "done"
            }]}
        }));

        let finished = events
            .iter()
            .find(|e| e.kind() == "subagent.finished")
            .expect("the subagent's end must be reported");
        match &finished.payload {
            EventPayload::SubagentFinished {
                subagent_type,
                tool_uses,
                total_tokens,
                ..
            } => {
                assert_eq!(subagent_type, "Explore");
                // Carried from the last progress report, because the completion
                // itself says nothing about what the subagent actually did.
                assert_eq!(*tool_uses, 10);
                assert_eq!(*total_tokens, 157251);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn an_unrelated_tool_completing_does_not_end_the_subagent() {
        // A subagent runs while the parent is idle, but the parent's own earlier
        // tools can still be settling. Only the `Task` it belongs to ends it.
        let mut n = normalizer();
        n.ingest(&task_progress(3, 900, "Reading store.ts"));
        let events = n.ingest(&serde_json::json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result", "tool_use_id": "toolu_somethingelse",
                "content": "ok"
            }]}
        }));
        assert!(!events.iter().any(|e| e.kind() == "subagent.finished"));
    }

    #[test]
    fn a_gate_that_failed_is_still_recognised_as_tervins_own() {
        // The case that was wrong. When the gate blocks, the runtime echoes its command
        // line back and the flag is there to find. When the gate merely fails there is
        // no command line — only the hook's own stderr — so matching on the flag alone
        // recognised the gate exactly when it worked and missed it every time it broke.
        // Tervin then showed its own dead socket as a hook the user had configured.
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "system", "subtype": "hook_response",
            "hook_name": "PreToolUse:Bash", "hook_event": "PreToolUse",
            "stderr": "Tervin hook: Tervin did not answer within 5s.",
            "exit_code": 1, "outcome": "error"
        }));

        assert!(
            events.is_empty(),
            "Tervin's own failure is not a diagnostic about the user's setup: {events:?}"
        );
        assert_eq!(n.hook_runs.len(), 1);
        assert!(
            n.hook_runs[0].is_tervin,
            "a gate failure carries no command line, so the stderr prefix has to carry it"
        );
    }

    #[test]
    fn the_hook_client_and_the_normalizer_agree_on_the_prefix() {
        // The two ends drifting apart is what caused the misattribution, and nothing
        // else would catch it: each side compiles perfectly well on its own.
        let mut n = normalizer();
        n.ingest(&serde_json::json!({
            "type": "system", "subtype": "hook_response",
            "hook_name": "PreToolUse:Bash", "hook_event": "PreToolUse",
            "stderr": format!(
                "{}could not reach Tervin at /run/h.sock (No such file or directory). \
                 This tool call was NOT checked against Tervin Rules.",
                crate::claude::hooks::HOOK_STDERR_PREFIX
            ),
            "exit_code": 1, "outcome": "error"
        }));
        assert!(n.hook_runs[0].is_tervin);
    }

    #[test]
    fn a_user_hook_that_fails_is_still_reported_as_theirs() {
        // The other half of the fix: widening the match must not swallow the user's own
        // broken hooks, which are the reason the diagnostic exists at all.
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "system", "subtype": "hook_response",
            "hook_name": "PreToolUse:Bash", "hook_event": "PreToolUse",
            "stderr": "my-gate.sh: line 3: jq: command not found",
            "exit_code": 127, "outcome": "error"
        }));
        assert!(!n.hook_runs[0].is_tervin);
        assert!(events.iter().any(|e| e.kind() == "diagnostic.detected"));
    }

    #[test]
    fn an_error_result_fails_the_thread() {
        let mut n = normalizer();
        let events = n.ingest(&serde_json::json!({
            "type": "result", "subtype": "error_max_turns", "is_error": true
        }));
        assert_eq!(n.state(), ThreadState::Failed);
        let failed = events
            .iter()
            .find_map(|e| match &e.payload {
                EventPayload::ThreadFailed { recoverable, .. } => Some(*recoverable),
                _ => None,
            })
            .expect("no thread.failed");
        assert_eq!(failed, Some(true));
    }

    #[test]
    fn a_process_that_dies_mid_run_is_reported_as_disconnected() {
        let mut n = normalizer();
        let events = n.disconnected("process exited with status 1");
        assert_eq!(n.state(), ThreadState::Disconnected);
        assert_eq!(kinds(&events), vec!["thread.failed"]);
    }

    #[test]
    fn a_completed_thread_is_not_overwritten_by_a_later_disconnect() {
        let mut n = normalizer();
        n.ingest(&serde_json::json!({"type":"result","subtype":"success","is_error":false}));
        assert!(n.disconnected("exited").is_empty());
        assert_eq!(n.state(), ThreadState::Completed);
    }

    #[test]
    fn attachments_are_recorded_as_explicit_context() {
        // Local-first privacy: what left the machine is always on the record.
        let mut n = normalizer();
        let events = n.user_prompt(
            "fix this",
            &[crate::runtime::Attachment::File {
                path: "src/a.rs".to_string(),
            }],
        );
        assert_eq!(kinds(&events)[0], "context.attached");
        match &events[0].payload {
            EventPayload::ContextAttached { description, .. } => {
                assert!(description.contains("src/a.rs"))
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn tervin_decisions_are_attributed_to_tervin() {
        let mut n = normalizer();
        let events = n.tervin_decision("Bash", false, "Denied by Tervin Rules");
        match &events[0].payload {
            EventPayload::PermissionDenied { authority, .. } => {
                assert_eq!(*authority, DecisionAuthority::Tervin)
            }
            _ => panic!("expected a denial"),
        }
    }

    #[test]
    fn tool_summaries_are_readable_not_json_dumps() {
        assert_eq!(
            summarise_tool_input("Bash", &serde_json::json!({"command": "ls -la"})),
            "ls -la"
        );
        assert_eq!(
            summarise_tool_input("Read", &serde_json::json!({"file_path": "/a/b/c/d.rs"})),
            "c/d.rs"
        );
        assert_eq!(
            summarise_tool_input("Grep", &serde_json::json!({"pattern": "TODO"})),
            "/TODO/"
        );
        // An unknown tool still yields something bounded.
        let unknown =
            summarise_tool_input("mcp__thing__do", &serde_json::json!({"a": "x".repeat(500)}));
        assert!(unknown.chars().count() <= 161);
    }

    #[test]
    fn identifies_test_commands() {
        for cmd in [
            "cargo test",
            "npm run test -- --watch",
            "pytest -q",
            "go test ./...",
        ] {
            assert!(looks_like_tests(cmd), "{cmd} not recognised as tests");
        }
        assert!(!looks_like_tests("cargo build"));
        // "latest" contains "test" but is not a test command.
        assert!(!looks_like_tests("npm install latest"));
    }
}
