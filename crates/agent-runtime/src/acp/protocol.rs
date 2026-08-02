//! Agent Client Protocol wire types.
//!
//! ACP is JSON-RPC 2.0 over stdio, standardising how an editor drives a coding
//! agent — the same role LSP plays for language servers. One adapter therefore
//! covers every agent that speaks it, present and future, instead of a bespoke
//! integration per vendor.
//!
//! The reason it matters more than convenience: ACP has
//! `session/request_permission`, an **agent → client** call. The agent asks before
//! acting and waits for the answer. That is a genuine pre-execution gate, which is
//! exactly what Tervin Rules need and exactly what Claude Code's `stream-json`
//! stream does not currently offer. Under ACP, "Deny" actually denies.
//!
//! Types here are deliberately permissive. ACP is young and evolving, so unknown
//! fields are ignored rather than rejected, and unknown notification variants are
//! preserved as unclassified rather than dropped — a spec revision should degrade
//! a Tervin feature, not break the connection.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version Tervin implements.
///
/// Sent during `initialize`; the agent replies with the version it will use.
pub const PROTOCOL_VERSION: u32 = 1;

// ---------------------------------------------------------------- envelopes

/// A JSON-RPC request or notification Tervin sends.
#[derive(Debug, Clone, Serialize)]
pub struct Outgoing {
    pub jsonrpc: &'static str,
    /// Absent for a notification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Outgoing {
    pub fn request(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id: Some(id),
            method: method.into(),
            params: Some(params),
        }
    }

    pub fn notification(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id: None,
            method: method.into(),
            params: Some(params),
        }
    }
}

/// Anything arriving from the agent.
///
/// JSON-RPC multiplexes three shapes down one pipe and they are told apart by
/// which fields are present, not by a discriminator — so this is parsed
/// structurally rather than with a serde tag.
#[derive(Debug, Clone)]
pub enum Incoming {
    /// A response to something Tervin asked.
    Response { id: u64, result: Value },
    /// An error response to something Tervin asked.
    Error { id: u64, code: i64, message: String },
    /// A request *from* the agent, which Tervin must answer.
    Request {
        id: u64,
        method: String,
        params: Value,
    },
    /// A one-way message from the agent.
    Notification { method: String, params: Value },
}

/// Classify a raw JSON-RPC message.
///
/// Returns `None` for anything that is not a recognisable envelope, which the
/// caller records as runtime noise rather than treating as a protocol failure —
/// some agents print diagnostics on stdout.
pub fn classify(value: &Value) -> Option<Incoming> {
    let method = value.get("method").and_then(Value::as_str);
    // An id may be a number or a string; only numeric ids are minted by Tervin,
    // and a string id from an agent is still answerable.
    let id = value.get("id").and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
    });

    match (method, id) {
        // A request from the agent: has both a method and an id.
        (Some(method), Some(id)) => Some(Incoming::Request {
            id,
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        }),
        // A notification: method, no id.
        (Some(method), None) => Some(Incoming::Notification {
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        }),
        // A response: id, no method.
        (None, Some(id)) => {
            if let Some(error) = value.get("error") {
                Some(Incoming::Error {
                    id,
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_string(),
                })
            } else {
                Some(Incoming::Response {
                    id,
                    result: value.get("result").cloned().unwrap_or(Value::Null),
                })
            }
        }
        (None, None) => None,
    }
}

// ------------------------------------------------------------------ methods

/// Methods Tervin calls on the agent.
pub mod agent_method {
    pub const INITIALIZE: &str = "initialize";
    pub const AUTHENTICATE: &str = "authenticate";
    pub const SESSION_NEW: &str = "session/new";
    pub const SESSION_PROMPT: &str = "session/prompt";
    pub const SESSION_LOAD: &str = "session/load";
    pub const SESSION_SET_MODE: &str = "session/set_mode";
    /// A notification: cancellation expects no reply.
    pub const SESSION_CANCEL: &str = "session/cancel";
}

/// Methods the agent calls on Tervin.
pub mod client_method {
    /// The pre-execution gate. This is the method that makes ACP worth adopting.
    pub const REQUEST_PERMISSION: &str = "session/request_permission";
    pub const SESSION_UPDATE: &str = "session/update";
    pub const FS_READ_TEXT_FILE: &str = "fs/read_text_file";
    pub const FS_WRITE_TEXT_FILE: &str = "fs/write_text_file";
    pub const TERMINAL_CREATE: &str = "terminal/create";
    pub const TERMINAL_OUTPUT: &str = "terminal/output";
    pub const TERMINAL_RELEASE: &str = "terminal/release";
    pub const TERMINAL_WAIT_FOR_EXIT: &str = "terminal/wait_for_exit";
    pub const TERMINAL_KILL: &str = "terminal/kill";
}

// --------------------------------------------------------------- initialize

/// What Tervin tells the agent it can do.
///
/// Only capabilities Tervin actually implements are declared. Claiming
/// `fs/write_text_file` and then failing the call would leave an agent unable to
/// make progress with no way to find out why.
#[derive(Debug, Clone, Serialize)]
pub struct ClientCapabilities {
    pub fs: FsCapabilities,
    /// Whether Tervin can run commands on the agent's behalf.
    pub terminal: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FsCapabilities {
    #[serde(rename = "readTextFile")]
    pub read_text_file: bool,
    #[serde(rename = "writeTextFile")]
    pub write_text_file: bool,
}

impl Default for ClientCapabilities {
    fn default() -> Self {
        Self {
            fs: FsCapabilities {
                // Reading is safe and lets an agent work without shelling out.
                read_text_file: true,
                // Writing goes through Tervin Rules like any other file mutation,
                // so it is offered — the gate is what makes it acceptable.
                write_text_file: true,
            },
            // Tervin owns PTYs already, so hosting the agent's commands means its
            // output becomes Blocks like everything else.
            terminal: true,
        }
    }
}

/// What the agent says it can do.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AgentCapabilities {
    /// Whether `session/load` works, i.e. whether sessions can be resumed.
    #[serde(rename = "loadSession")]
    pub load_session: bool,
    #[serde(rename = "promptCapabilities")]
    pub prompt: PromptCapabilities,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PromptCapabilities {
    pub image: bool,
    pub audio: bool,
    #[serde(rename = "embeddedContext")]
    pub embedded_context: bool,
}

/// The agent's `initialize` result.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u32,
    #[serde(rename = "agentCapabilities")]
    pub agent_capabilities: AgentCapabilities,
    /// Authentication methods, if the agent needs one before a session.
    #[serde(rename = "authMethods")]
    pub auth_methods: Vec<AuthMethod>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

// ------------------------------------------------------------- session/update

/// A `session/update` notification.
///
/// The `sessionUpdate` field discriminates. Unknown variants land in `Other` and
/// are surfaced as unclassified rather than discarded — a newer agent must not
/// silently lose events.
#[derive(Debug, Clone)]
pub enum SessionUpdate {
    /// A chunk of the agent's reply.
    AgentMessageChunk { text: String },
    /// A chunk of the agent's reasoning, shown collapsed.
    AgentThoughtChunk { text: String },
    /// The user's own message, echoed back.
    UserMessageChunk { text: String },
    /// A tool call starting.
    ToolCall {
        id: String,
        title: String,
        kind: String,
        status: String,
        raw_input: Value,
    },
    /// A tool call progressing or finishing.
    ToolCallUpdate {
        id: String,
        status: Option<String>,
        content: Option<String>,
    },
    /// The agent's plan.
    Plan { entries: Vec<PlanEntry> },
    /// The agent switched operating mode.
    ModeUpdated { mode: String },
    /// Something Tervin does not model.
    Other { kind: String },
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PlanEntry {
    pub content: String,
    /// `pending`, `in_progress`, or `completed`.
    pub status: String,
    pub priority: Option<String>,
}

/// Parse a `session/update` payload.
pub fn parse_session_update(params: &Value) -> Option<(String, SessionUpdate)> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // The update may be nested under `update` or inline, depending on revision.
    let body = params.get("update").unwrap_or(params);
    let kind = body.get("sessionUpdate").and_then(Value::as_str)?;

    let text_of =
        |field: &str| -> String { body.get(field).and_then(content_text).unwrap_or_default() };

    let update = match kind {
        "agent_message_chunk" => SessionUpdate::AgentMessageChunk {
            text: text_of("content"),
        },
        "agent_thought_chunk" => SessionUpdate::AgentThoughtChunk {
            text: text_of("content"),
        },
        "user_message_chunk" => SessionUpdate::UserMessageChunk {
            text: text_of("content"),
        },
        "tool_call" => SessionUpdate::ToolCall {
            id: body
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            title: body
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            kind: body
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("other")
                .to_string(),
            status: body
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .to_string(),
            raw_input: body.get("rawInput").cloned().unwrap_or(Value::Null),
        },
        "tool_call_update" => SessionUpdate::ToolCallUpdate {
            id: body
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status: body.get("status").and_then(Value::as_str).map(String::from),
            content: body.get("content").and_then(content_text),
        },
        "plan" => SessionUpdate::Plan {
            entries: body
                .get("entries")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|e| serde_json::from_value(e.clone()).ok())
                        .collect()
                })
                .unwrap_or_default(),
        },
        "current_mode_update" => SessionUpdate::ModeUpdated {
            mode: body
                .get("currentModeId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        other => SessionUpdate::Other {
            kind: other.to_string(),
        },
    };

    Some((session_id, update))
}

/// Pull display text out of an ACP content block.
///
/// Content arrives as a `{type, text}` object, an array of them, or a bare string
/// depending on the field and revision, so all three are accepted.
pub fn content_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Object(_) => value
            .get("text")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| {
                // A resource or resource_link block: show its URI rather than
                // nothing.
                value
                    .get("uri")
                    .and_then(Value::as_str)
                    .map(|uri| uri.to_string())
            }),
        Value::Array(items) => {
            let joined: Vec<String> = items.iter().filter_map(content_text).collect();
            (!joined.is_empty()).then(|| joined.join(""))
        }
        _ => None,
    }
}

// --------------------------------------------------------------- permissions

/// A `session/request_permission` payload.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub session_id: String,
    /// What the agent wants to do, as it describes it.
    pub title: String,
    pub kind: String,
    /// The tool's raw arguments, used for risk classification.
    pub raw_input: Value,
    /// The options the agent will accept as an answer.
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once`, `reject_always`.
    pub kind: String,
}

impl PermissionOption {
    pub fn is_allow(&self) -> bool {
        self.kind.starts_with("allow")
    }

    pub fn is_always(&self) -> bool {
        self.kind.ends_with("always")
    }
}

pub fn parse_permission_request(params: &Value) -> Option<PermissionRequest> {
    let tool_call = params.get("toolCall").unwrap_or(params);

    let options = params
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|o| {
                    Some(PermissionOption {
                        id: o.get("optionId")?.as_str()?.to_string(),
                        name: o
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        kind: o
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("allow_once")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Some(PermissionRequest {
        session_id: params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        title: tool_call
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("an action")
            .to_string(),
        kind: tool_call
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("other")
            .to_string(),
        raw_input: tool_call.get("rawInput").cloned().unwrap_or(Value::Null),
        options,
    })
}

/// Why a prompt turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    Unknown,
}

impl StopReason {
    pub fn parse(value: &Value) -> Self {
        match value.get("stopReason").and_then(Value::as_str) {
            Some("end_turn") => Self::EndTurn,
            Some("max_tokens") => Self::MaxTokens,
            Some("max_turn_requests") => Self::MaxTurnRequests,
            Some("refusal") => Self::Refusal,
            Some("cancelled") => Self::Cancelled,
            _ => Self::Unknown,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::EndTurn => "completed",
            Self::MaxTokens => "stopped: token limit",
            Self::MaxTurnRequests => "stopped: request limit",
            Self::Refusal => "the agent declined",
            Self::Cancelled => "cancelled",
            Self::Unknown => "stopped for an unreported reason",
        }
    }

    /// Whether this counts as finishing the work rather than being cut short.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::EndTurn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_a_response() {
        let value = json!({"jsonrpc": "2.0", "id": 7, "result": {"ok": true}});
        match classify(&value) {
            Some(Incoming::Response { id, result }) => {
                assert_eq!(id, 7);
                assert_eq!(result["ok"], json!(true));
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[test]
    fn classifies_an_error_response() {
        let value =
            json!({"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"no such method"}});
        match classify(&value) {
            Some(Incoming::Error { id, code, message }) => {
                assert_eq!(id, 3);
                assert_eq!(code, -32601);
                assert_eq!(message, "no such method");
            }
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn classifies_an_agent_request_by_having_both_method_and_id() {
        // The distinction that matters: a request needs answering, a notification
        // does not, and only the presence of an id tells them apart.
        let value = json!({
            "jsonrpc":"2.0","id":11,
            "method":"session/request_permission",
            "params":{"sessionId":"s1"}
        });
        match classify(&value) {
            Some(Incoming::Request { id, method, .. }) => {
                assert_eq!(id, 11);
                assert_eq!(method, "session/request_permission");
            }
            other => panic!("expected a request, got {other:?}"),
        }
    }

    #[test]
    fn classifies_a_notification() {
        let value = json!({"jsonrpc":"2.0","method":"session/update","params":{}});
        assert!(matches!(
            classify(&value),
            Some(Incoming::Notification { .. })
        ));
    }

    #[test]
    fn accepts_a_string_id_from_an_agent() {
        // Tervin mints numeric ids, but an agent's own request may use a string.
        let value = json!({"jsonrpc":"2.0","id":"42","method":"fs/read_text_file","params":{}});
        assert!(matches!(
            classify(&value),
            Some(Incoming::Request { id: 42, .. })
        ));
    }

    #[test]
    fn ignores_a_line_that_is_not_an_envelope() {
        // Some agents print diagnostics on stdout; that is not a protocol failure.
        assert!(classify(&json!({"hello": "world"})).is_none());
        assert!(classify(&json!("plain text")).is_none());
    }

    #[test]
    fn parses_an_agent_message_chunk() {
        let params = json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "Looking at the code"}
            }
        });
        let (session, update) = parse_session_update(&params).unwrap();
        assert_eq!(session, "s1");
        match update {
            SessionUpdate::AgentMessageChunk { text } => assert_eq!(text, "Looking at the code"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn parses_an_update_that_is_not_nested() {
        // Revisions differ on whether the body sits under `update`.
        let params = json!({
            "sessionId": "s1",
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "inline"}
        });
        let (_, update) = parse_session_update(&params).unwrap();
        assert!(matches!(update, SessionUpdate::AgentMessageChunk { text } if text == "inline"));
    }

    #[test]
    fn parses_a_tool_call_and_its_update() {
        let call = json!({
            "sessionId":"s1",
            "update":{
                "sessionUpdate":"tool_call",
                "toolCallId":"t1",
                "title":"Read src/main.rs",
                "kind":"read",
                "status":"pending",
                "rawInput":{"path":"src/main.rs"}
            }
        });
        match parse_session_update(&call).unwrap().1 {
            SessionUpdate::ToolCall {
                id,
                title,
                kind,
                raw_input,
                ..
            } => {
                assert_eq!(id, "t1");
                assert_eq!(title, "Read src/main.rs");
                assert_eq!(kind, "read");
                assert_eq!(raw_input["path"], json!("src/main.rs"));
            }
            other => panic!("got {other:?}"),
        }

        let update = json!({
            "sessionId":"s1",
            "update":{"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed"}
        });
        match parse_session_update(&update).unwrap().1 {
            SessionUpdate::ToolCallUpdate { id, status, .. } => {
                assert_eq!(id, "t1");
                assert_eq!(status.as_deref(), Some("completed"));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn parses_a_plan() {
        let params = json!({
            "sessionId":"s1",
            "update":{
                "sessionUpdate":"plan",
                "entries":[
                    {"content":"Read the parser","status":"completed"},
                    {"content":"Add a test","status":"pending","priority":"high"}
                ]
            }
        });
        match parse_session_update(&params).unwrap().1 {
            SessionUpdate::Plan { entries } => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].content, "Read the parser");
                assert_eq!(entries[1].status, "pending");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_update_variant_is_kept_not_dropped() {
        // A newer agent must not silently lose events.
        let params = json!({
            "sessionId":"s1",
            "update":{"sessionUpdate":"something_new_in_v3"}
        });
        match parse_session_update(&params).unwrap().1 {
            SessionUpdate::Other { kind } => assert_eq!(kind, "something_new_in_v3"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn content_text_accepts_every_shape_it_arrives_in() {
        assert_eq!(content_text(&json!("bare")).as_deref(), Some("bare"));
        assert_eq!(
            content_text(&json!({"type":"text","text":"object"})).as_deref(),
            Some("object")
        );
        assert_eq!(
            content_text(&json!([{"type":"text","text":"a"},{"type":"text","text":"b"}]))
                .as_deref(),
            Some("ab")
        );
        // A resource block shows its URI rather than nothing.
        assert_eq!(
            content_text(&json!({"type":"resource_link","uri":"file:///x.rs"})).as_deref(),
            Some("file:///x.rs")
        );
        assert_eq!(content_text(&json!(null)), None);
    }

    #[test]
    fn parses_a_permission_request_with_its_options() {
        // The whole reason for adopting ACP: a real pre-execution gate.
        let params = json!({
            "sessionId":"s1",
            "toolCall":{
                "title":"Run `rm -rf build`",
                "kind":"execute",
                "rawInput":{"command":"rm -rf build"}
            },
            "options":[
                {"optionId":"a1","name":"Allow once","kind":"allow_once"},
                {"optionId":"a2","name":"Always allow","kind":"allow_always"},
                {"optionId":"r1","name":"Reject","kind":"reject_once"}
            ]
        });

        let request = parse_permission_request(&params).unwrap();
        assert_eq!(request.session_id, "s1");
        assert_eq!(request.title, "Run `rm -rf build`");
        assert_eq!(request.raw_input["command"], json!("rm -rf build"));
        assert_eq!(request.options.len(), 3);

        let allow_once = &request.options[0];
        assert!(allow_once.is_allow() && !allow_once.is_always());
        assert!(request.options[1].is_allow() && request.options[1].is_always());
        assert!(!request.options[2].is_allow());
    }

    #[test]
    fn a_permission_request_without_options_still_parses() {
        // Degraded but usable: Tervin can still show what was asked.
        let request = parse_permission_request(&json!({"sessionId":"s1"})).unwrap();
        assert!(request.options.is_empty());
        assert_eq!(request.title, "an action");
    }

    #[test]
    fn parses_every_stop_reason_and_only_end_turn_is_success() {
        assert_eq!(
            StopReason::parse(&json!({"stopReason":"end_turn"})),
            StopReason::EndTurn
        );
        assert!(StopReason::EndTurn.is_success());
        for (wire, expected) in [
            ("max_tokens", StopReason::MaxTokens),
            ("max_turn_requests", StopReason::MaxTurnRequests),
            ("refusal", StopReason::Refusal),
            ("cancelled", StopReason::Cancelled),
        ] {
            let parsed = StopReason::parse(&json!({"stopReason": wire}));
            assert_eq!(parsed, expected);
            // Being cut short is not success, and the UI must not present it as one.
            assert!(!parsed.is_success(), "{wire} should not be success");
            assert!(!parsed.label().is_empty());
        }
        assert_eq!(StopReason::parse(&json!({})), StopReason::Unknown);
    }

    #[test]
    fn declared_capabilities_match_what_tervin_implements() {
        // Claiming a capability and then failing the call leaves an agent stuck
        // with no way to find out why.
        let caps = ClientCapabilities::default();
        assert!(caps.fs.read_text_file);
        assert!(caps.fs.write_text_file);
        assert!(caps.terminal);
    }

    #[test]
    fn an_initialize_result_with_unknown_fields_still_parses() {
        // ACP is young; a spec addition must not break the connection.
        let result: InitializeResult = serde_json::from_value(json!({
            "protocolVersion": 1,
            "agentCapabilities": {"loadSession": true, "somethingNew": 42},
            "authMethods": [{"id":"oauth","name":"Sign in"}],
            "unknownTopLevel": "ignored"
        }))
        .unwrap();
        assert_eq!(result.protocol_version, 1);
        assert!(result.agent_capabilities.load_session);
        assert_eq!(result.auth_methods.len(), 1);
    }

    #[test]
    fn outgoing_requests_and_notifications_serialise_correctly() {
        let request = serde_json::to_value(Outgoing::request(1, "initialize", json!({}))).unwrap();
        assert_eq!(request["jsonrpc"], json!("2.0"));
        assert_eq!(request["id"], json!(1));

        // A notification must carry no id at all, or the agent will try to reply.
        let notification =
            serde_json::to_value(Outgoing::notification("session/cancel", json!({}))).unwrap();
        assert!(notification.get("id").is_none());
    }
}
