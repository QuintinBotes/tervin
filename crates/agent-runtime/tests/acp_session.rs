//! The ACP adapter, driven against a real agent process.
//!
//! Unit tests can prove the parser is right about a payload someone wrote down.
//! They cannot prove the *conversation* works: that the handshake completes, that a
//! notification arriving mid-request is not mistaken for a response, that a
//! permission request Tervin denies is actually denied, or that an agent asking
//! Tervin to run a command gets it run and gets the output back.
//!
//! So these tests speak the protocol for real. A small Python agent sits on the
//! other end of the pipe and plays out a named scenario, and every assertion is on
//! what Tervin produced or on what the agent observed Tervin doing. Nothing here is
//! mocked at the transport.
//!
//! The two invariants worth the whole file:
//!
//! - [`a_denied_permission_is_actually_denied`] — the agent asks, Tervin Rules say
//!   no, and the agent receives the reject option. This is the difference between a
//!   gate and a notification, and it is the reason ACP is worth adopting.
//! - [`an_allowed_permission_never_escalates_to_always`] — Tervin picks
//!   `allow_once` even when `allow_always` is on offer. A standing grant is the
//!   user's to give.

use agent_runtime::acp::AcpRuntime;
use agent_runtime::runtime::{
    AgentRuntime, ArbiterDecision, LaunchConfig, LaunchedSession, PermissionArbiter,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tervin_core::{EventPayload, TervinEvent, ThreadId};

/// Long enough for a Python interpreter to start on a loaded machine.
const TIMEOUT: Duration = Duration::from_secs(30);

// --------------------------------------------------------------- the fake agent

/// A minimal ACP agent. Scenario-driven so each test drives one exchange.
const FAKE_AGENT: &str = r#"
import json, os, sys

SESSION = "fake-session-1"
next_id = 1000

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

def read():
    line = sys.stdin.readline()
    if not line:
        return None
    line = line.strip()
    if not line:
        return read()
    return json.loads(line)

def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})

def update(body):
    notify("session/update", {"sessionId": SESSION, "update": body})

def say(text):
    update({"sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": text}})

def call(method, params):
    """Ask the client something and wait for its answer."""
    global next_id
    rid = next_id
    next_id += 1
    send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
    while True:
        msg = read()
        if msg is None:
            return None
        if "method" in msg:
            # Only session/cancel is expected while a call is outstanding.
            continue
        if msg.get("id") == rid:
            return msg

def scenario_basic():
    say("Looking ")
    say("at the parser.")
    update({"sessionUpdate": "plan", "entries": [
        {"content": "Read the parser", "status": "pending"},
        {"content": "Add a test", "status": "pending"},
    ]})
    update({"sessionUpdate": "tool_call", "toolCallId": "t1",
            "title": "Read src/main.rs", "kind": "read", "status": "pending",
            "rawInput": {"path": "/tmp/main.rs"}})
    update({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
            "status": "completed",
            "content": {"type": "text", "text": "fn main() {}"}})
    return "end_turn"

def ask_permission(command):
    reply = call("session/request_permission", {
        "sessionId": SESSION,
        "toolCall": {"title": "Run `%s`" % command, "kind": "execute",
                     "rawInput": {"command": command}},
        "options": [
            {"optionId": "once", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
            {"optionId": "no", "name": "Reject", "kind": "reject_once"},
        ],
    })
    outcome = (reply or {}).get("result", {}).get("outcome", {})
    say("OUTCOME=%s OPTION=%s" % (outcome.get("outcome"), outcome.get("optionId")))
    return "end_turn"

def scenario_fs():
    allowed = call("fs/read_text_file",
                   {"sessionId": SESSION, "path": os.environ["ALLOWED_FILE"]})
    say("ALLOWED=%s" % (allowed or {}).get("result", {}).get("content"))

    outside = call("fs/read_text_file", {"sessionId": SESSION, "path": "/etc/hosts"})
    say("OUTSIDE=%s" % ("error" if "error" in (outside or {}) else "read"))

    secret = call("fs/read_text_file",
                  {"sessionId": SESSION, "path": os.environ["SECRET_FILE"]})
    say("SECRET=%s" % ("error" if "error" in (secret or {}) else "read"))

    relative = call("fs/read_text_file", {"sessionId": SESSION, "path": "notes.txt"})
    say("RELATIVE=%s" % ("error" if "error" in (relative or {}) else "read"))

    written = call("fs/write_text_file", {
        "sessionId": SESSION,
        "path": os.environ["WRITE_FILE"],
        "content": "written by the agent\n",
    })
    say("WROTE=%s" % ("error" if "error" in (written or {}) else "ok"))
    return "end_turn"

def scenario_terminal():
    created = call("terminal/create", {
        "sessionId": SESSION,
        "command": "/bin/echo",
        "args": ["hello", "from the agent"],
        "cwd": os.environ["PROJECT_ROOT"],
    })
    if "error" in (created or {}):
        say("CREATE=error")
        return "end_turn"
    tid = created["result"]["terminalId"]

    exited = call("terminal/wait_for_exit", {"sessionId": SESSION, "terminalId": tid})
    code = (exited or {}).get("result", {}).get("exitCode")
    out = call("terminal/output", {"sessionId": SESSION, "terminalId": tid})
    body = (out or {}).get("result", {})
    say("EXIT=%s OUTPUT=%s TRUNCATED=%s"
        % (code, (body.get("output") or "").strip(), body.get("truncated")))
    call("terminal/release", {"sessionId": SESSION, "terminalId": tid})
    return "end_turn"

def scenario_terminal_long():
    """Start something that would far outlive the session, then wait on it."""
    created = call("terminal/create", {
        "sessionId": SESSION,
        "command": "/bin/sleep",
        "args": ["120"],
        "cwd": os.environ["PROJECT_ROOT"],
    })
    if "error" in (created or {}):
        say("CREATE=error")
        return "end_turn"
    tid = created["result"]["terminalId"]
    exited = call("terminal/wait_for_exit", {"sessionId": SESSION, "terminalId": tid})
    say("LONG_EXIT=%s" % (exited or {}).get("result"))
    return "end_turn"

def scenario_terminal_denied():
    created = call("terminal/create", {
        "sessionId": SESSION,
        "command": "/bin/echo",
        "args": ["DENY-ME"],
        "cwd": os.environ["PROJECT_ROOT"],
    })
    say("CREATE=%s" % ("error" if "error" in (created or {}) else "ok"))
    return "end_turn"

def scenario_unknown_method():
    reply = call("tervin/does_not_exist", {"sessionId": SESSION})
    say("UNKNOWN=%s" % ("error" if "error" in (reply or {}) else "answered"))
    return "end_turn"

def run(name):
    if name == "basic":
        return scenario_basic()
    if name == "deny":
        return ask_permission("rm -rf /")
    if name == "allow":
        return ask_permission("ls -la")
    if name == "fs":
        return scenario_fs()
    if name == "terminal":
        return scenario_terminal()
    if name == "terminal-long":
        return scenario_terminal_long()
    if name == "terminal-denied":
        return scenario_terminal_denied()
    if name == "unknown-method":
        return scenario_unknown_method()
    if name == "refusal":
        say("I will not do that.")
        return "refusal"
    if name == "noise":
        # Some agents print banners on stdout. It must not break the connection.
        sys.stdout.write("warning: running in experimental mode\n")
        sys.stdout.flush()
        say("still here")
        return "end_turn"
    return "end_turn"

def main():
    scenario = sys.argv[1] if len(sys.argv) > 1 else "basic"
    load_session = os.environ.get("FAKE_LOAD_SESSION", "1") == "1"

    while True:
        msg = read()
        if msg is None:
            return
        method = msg.get("method")
        mid = msg.get("id")

        if method == "initialize":
            send({"jsonrpc": "2.0", "id": mid, "result": {
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": load_session,
                    "promptCapabilities": {"image": True},
                },
                "authMethods": [],
            }})
        elif method == "session/new":
            send({"jsonrpc": "2.0", "id": mid, "result": {
                "sessionId": SESSION,
                "modes": {
                    "currentModeId": "default",
                    "availableModes": [
                        {"id": "default", "name": "Default"},
                        {"id": "careful", "name": "Careful"},
                    ],
                },
            }})
        elif method == "session/load":
            send({"jsonrpc": "2.0", "id": mid, "result": {}})
        elif method == "session/set_mode":
            send({"jsonrpc": "2.0", "id": mid, "result": {}})
        elif method == "session/prompt":
            stop = run(scenario)
            send({"jsonrpc": "2.0", "id": mid, "result": {"stopReason": stop}})
        elif method == "session/cancel":
            pass
        elif mid is not None:
            send({"jsonrpc": "2.0", "id": mid,
                  "error": {"code": -32601, "message": "no such method"}})

main()
"#;

fn python() -> Option<&'static str> {
    for candidate in ["/usr/bin/python3", "python3"] {
        if Path::new(candidate).is_file() {
            return Some(candidate);
        }
        if std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).any(|dir| dir.join(candidate).is_file()))
            .unwrap_or(false)
        {
            return Some(candidate);
        }
    }
    None
}

/// A project directory with the fake agent script in it.
struct Fixture {
    dir: tempfile::TempDir,
    script: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("no temp dir");
        let script = dir.path().join("fake_acp_agent.py");
        std::fs::write(&script, FAKE_AGENT).expect("could not write the agent");
        Self { dir, script }
    }

    fn root(&self) -> PathBuf {
        std::fs::canonicalize(self.dir.path()).expect("could not canonicalise the root")
    }
}

// ------------------------------------------------------------------- arbiters

/// Tervin Rules stand-in.
///
/// Real classification is used for commands, so the risk levels the tests assert
/// on are the ones the product would produce. The extra `DENY-ME` marker exists so
/// a test can force a denial without ever putting a destructive command where a
/// bug could run it.
struct TestArbiter {
    deny_writes: bool,
}

#[async_trait]
impl PermissionArbiter for TestArbiter {
    async fn decide(
        &self,
        _thread_id: &ThreadId,
        tool_name: &str,
        input: &Value,
        cwd: &str,
    ) -> ArbiterDecision {
        if input.to_string().contains("DENY-ME") {
            return ArbiterDecision::Deny {
                reason: "Denied by test policy".into(),
            };
        }
        if self.deny_writes && tool_name == "fs/write_text_file" {
            return ArbiterDecision::Deny {
                reason: "Writes are not permitted by test policy".into(),
            };
        }
        if let Some(command) = input.get("command").and_then(Value::as_str) {
            let risk = rules_engine::classify(command, cwd);
            if risk.level.always_confirm() {
                return ArbiterDecision::Deny {
                    reason: format!("{} risk: {}", risk.level.label(), risk.reasons.join("; ")),
                };
            }
        }
        ArbiterDecision::Allow
    }
}

fn arbiter() -> Arc<dyn PermissionArbiter> {
    Arc::new(TestArbiter { deny_writes: false })
}

// -------------------------------------------------------------------- harness

/// Start the fake agent under the ACP adapter and run one prompt turn.
async fn run_scenario(
    fixture: &Fixture,
    scenario: &str,
    env: Vec<(String, String)>,
    arbiter: Arc<dyn PermissionArbiter>,
) -> Option<(Vec<TervinEvent>, LaunchedSession)> {
    let python = python()?;

    let runtime = AcpRuntime::custom(
        "test-acp",
        "Test ACP agent",
        python,
        vec![fixture.script.display().to_string(), scenario.to_string()],
    )
    .with_arbiter(arbiter);

    let mut config = LaunchConfig::new(ThreadId::new(), fixture.root().display().to_string())
        .with_prompt("do the thing");
    config.env = env;

    let mut launched = tokio::time::timeout(TIMEOUT, runtime.launch(config))
        .await
        .expect("launch timed out")
        .expect("launch failed");

    // Drain until the turn ends, so every assertion sees a complete exchange.
    let events = tokio::time::timeout(TIMEOUT, drain_until_settled(&mut launched))
        .await
        .expect("the turn never ended");

    Some((events, launched))
}

/// Collect events until the Thread reaches a terminal state.
async fn drain_until_settled(launched: &mut LaunchedSession) -> Vec<TervinEvent> {
    let mut events = Vec::new();
    while let Some(event) = launched.events.recv().await {
        let done = matches!(
            &event.payload,
            EventPayload::ThreadCompleted { .. } | EventPayload::ThreadFailed { .. }
        );
        events.push(event);
        if done {
            break;
        }
    }
    events
}

fn kinds(events: &[TervinEvent]) -> Vec<&'static str> {
    events.iter().map(|e| e.kind()).collect()
}

/// Everything the agent said, joined — this is how a scenario reports back what it
/// observed Tervin doing.
fn agent_said(events: &[TervinEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::AgentMessage {
                text,
                is_reasoning: false,
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------- tests

#[tokio::test]
async fn a_full_turn_produces_the_expected_event_stream() {
    let fixture = Fixture::new();
    let Some((events, session)) = run_scenario(&fixture, "basic", Vec::new(), arbiter()).await
    else {
        return; // No Python; nothing to prove here.
    };

    let seen = kinds(&events);
    for expected in [
        "thread.started",
        "user.prompted",
        "plan.proposed",
        "tool.requested",
        "tool.completed",
        "agent.message",
        "thread.completed",
    ] {
        assert!(seen.contains(&expected), "{expected} missing from {seen:?}");
    }

    // Chunks are coalesced: two chunks, one message.
    assert!(
        agent_said(&events).contains("Looking at the parser."),
        "chunks were not coalesced: {:?}",
        agent_said(&events)
    );

    // thread.started must come before anything it contextualises.
    let started = seen.iter().position(|k| *k == "thread.started").unwrap();
    let prompted = seen.iter().position(|k| *k == "user.prompted").unwrap();
    assert!(started < prompted, "events arrived out of order: {seen:?}");

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn a_denied_permission_is_actually_denied() {
    // The whole reason for adopting ACP. The agent asked, Tervin Rules said no,
    // and the agent received the reject option — not a notification after the fact.
    let fixture = Fixture::new();
    let Some((events, session)) = run_scenario(&fixture, "deny", Vec::new(), arbiter()).await
    else {
        return;
    };

    let requested = events
        .iter()
        .find_map(|e| match &e.payload {
            EventPayload::PermissionRequested {
                action,
                risk,
                interceptable,
                ..
            } => Some((action.clone(), risk.clone(), *interceptable)),
            _ => None,
        })
        .expect("no permission.requested");

    assert_eq!(requested.0, "rm -rf /");
    assert_eq!(requested.1.level, tervin_core::RiskLevel::Critical);
    assert!(
        requested.1.enforceable,
        "under ACP the gate is real, so the assessment must say so"
    );
    assert!(requested.2, "the agent was blocked on the answer");

    let denied = events
        .iter()
        .find_map(|e| match &e.payload {
            EventPayload::PermissionDenied {
                action, authority, ..
            } => Some((action.clone(), *authority)),
            _ => None,
        })
        .expect("no permission.denied");
    assert_eq!(denied.0, "rm -rf /");
    assert_eq!(
        denied.1,
        tervin_core::events::DecisionAuthority::Tervin,
        "Tervin decided this, so it must be attributed to Tervin"
    );

    // And the agent genuinely received the rejection.
    let said = agent_said(&events);
    assert!(
        said.contains("OUTCOME=selected") && said.contains("OPTION=no"),
        "the agent did not receive the rejection: {said}"
    );

    let permissions = session.session.permissions();
    assert!(permissions.tervin_can_intercept);
    assert!(
        permissions.denials.iter().any(|d| d.contains("rm -rf")),
        "the denial should be visible on the session: {:?}",
        permissions.denials
    );

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn an_allowed_permission_never_escalates_to_always() {
    // `allow_always` was on offer and must not be taken: a standing grant is the
    // user's to give, not an adapter's to infer from one decision.
    let fixture = Fixture::new();
    let Some((events, session)) = run_scenario(&fixture, "allow", Vec::new(), arbiter()).await
    else {
        return;
    };

    let said = agent_said(&events);
    assert!(
        said.contains("OPTION=once"),
        "expected allow_once, got: {said}"
    );
    assert!(
        !said.contains("OPTION=always"),
        "Tervin must never grant a standing permission on the user's behalf: {said}"
    );

    assert!(events
        .iter()
        .any(|e| matches!(&e.payload, EventPayload::PermissionGranted { .. })));

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn the_filesystem_capability_is_confined_to_the_project() {
    let fixture = Fixture::new();
    let root = fixture.root();

    std::fs::write(root.join("notes.txt"), "the allowed content").unwrap();
    std::fs::write(root.join(".env"), "TOKEN=hunter2").unwrap();

    let env = vec![
        (
            "ALLOWED_FILE".to_string(),
            root.join("notes.txt").display().to_string(),
        ),
        (
            "SECRET_FILE".to_string(),
            root.join(".env").display().to_string(),
        ),
        (
            "WRITE_FILE".to_string(),
            root.join("out").join("generated.txt").display().to_string(),
        ),
    ];

    let Some((events, session)) = run_scenario(&fixture, "fs", env, arbiter()).await else {
        return;
    };
    let said = agent_said(&events);

    assert!(
        said.contains("ALLOWED=the allowed content"),
        "a file inside the project should be readable: {said}"
    );
    assert!(
        said.contains("OUTSIDE=error"),
        "a file outside the project must be refused: {said}"
    );
    assert!(
        said.contains("SECRET=error"),
        "a credential file must be refused even inside the project: {said}"
    );
    assert!(
        said.contains("RELATIVE=error"),
        "a relative path must be refused: {said}"
    );
    assert!(
        said.contains("WROTE=ok"),
        "the write should succeed: {said}"
    );

    // The write really happened, including its parent directory.
    let written = std::fs::read_to_string(root.join("out").join("generated.txt"))
        .expect("the file was not written");
    assert_eq!(written, "written by the agent\n");

    // And it is in the timeline, attributed to Tervin, which performed it.
    let applied = events
        .iter()
        .find_map(|e| match &e.payload {
            EventPayload::PatchApplied { files, authority } => Some((files.clone(), *authority)),
            _ => None,
        })
        .expect("no patch.applied for the write");
    assert_eq!(applied.1, tervin_core::events::DecisionAuthority::Tervin);
    assert_eq!(
        applied.0[0].kind,
        tervin_core::events::FileChangeKind::Created
    );

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn a_write_the_rules_refuse_does_not_touch_the_disk() {
    let fixture = Fixture::new();
    let root = fixture.root();
    std::fs::write(root.join("notes.txt"), "content").unwrap();
    std::fs::write(root.join(".env"), "TOKEN=x").unwrap();

    let target = root.join("must-not-exist.txt");
    let env = vec![
        (
            "ALLOWED_FILE".to_string(),
            root.join("notes.txt").display().to_string(),
        ),
        (
            "SECRET_FILE".to_string(),
            root.join(".env").display().to_string(),
        ),
        ("WRITE_FILE".to_string(), target.display().to_string()),
    ];

    let deny_writes: Arc<dyn PermissionArbiter> = Arc::new(TestArbiter { deny_writes: true });
    let Some((events, session)) = run_scenario(&fixture, "fs", env, deny_writes).await else {
        return;
    };

    assert!(
        agent_said(&events).contains("WROTE=error"),
        "the agent should have been refused: {}",
        agent_said(&events)
    );
    assert!(
        !target.exists(),
        "a refused write must not reach the disk — that is the entire point of the gate"
    );

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn tervin_runs_commands_for_the_agent_and_reports_the_real_exit_status() {
    // Hosting the agent's commands is what makes the gate cover execution, and it
    // means the exit code in the timeline is one Tervin observed rather than one
    // the agent claimed.
    let fixture = Fixture::new();
    let env = vec![(
        "PROJECT_ROOT".to_string(),
        fixture.root().display().to_string(),
    )];

    let Some((events, session)) = run_scenario(&fixture, "terminal", env, arbiter()).await else {
        return;
    };
    let said = agent_said(&events);

    assert!(
        said.contains("EXIT=0"),
        "the agent should see the real exit code: {said}"
    );
    assert!(
        said.contains("OUTPUT=hello from the agent"),
        "the agent should get its command's output back: {said}"
    );
    assert!(
        said.contains("TRUNCATED=False"),
        "short output must not be reported as truncated: {said}"
    );

    let seen = kinds(&events);
    assert!(seen.contains(&"command.started"), "{seen:?}");
    assert!(seen.contains(&"command.completed"), "{seen:?}");
    assert!(
        seen.contains(&"command.output"),
        "the output belongs in the timeline: {seen:?}"
    );

    let completed = events
        .iter()
        .find_map(|e| match &e.payload {
            EventPayload::CommandCompleted {
                command, exit_code, ..
            } => Some((command.clone(), *exit_code)),
            _ => None,
        })
        .expect("no command.completed");
    assert_eq!(completed.1, 0);
    // Recorded the way a user would type it. The quoting matters: an argument with
    // a space in it must not read as two arguments.
    assert_eq!(completed.0, "/bin/echo hello 'from the agent'");

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn a_command_the_rules_refuse_is_never_started() {
    let fixture = Fixture::new();
    let env = vec![(
        "PROJECT_ROOT".to_string(),
        fixture.root().display().to_string(),
    )];

    let Some((events, session)) = run_scenario(&fixture, "terminal-denied", env, arbiter()).await
    else {
        return;
    };

    assert!(
        agent_said(&events).contains("CREATE=error"),
        "the agent should have been refused: {}",
        agent_said(&events)
    );
    assert!(
        !kinds(&events).contains(&"command.started"),
        "a refused command must never start"
    );
    assert!(events
        .iter()
        .any(|e| matches!(&e.payload, EventPayload::PermissionDenied { .. })));

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn an_unimplemented_client_method_is_refused_rather_than_ignored() {
    // Silence would hang the agent forever waiting for a reply.
    let fixture = Fixture::new();
    let Some((events, session)) =
        run_scenario(&fixture, "unknown-method", Vec::new(), arbiter()).await
    else {
        return;
    };
    assert!(
        agent_said(&events).contains("UNKNOWN=error"),
        "expected an error response: {}",
        agent_said(&events)
    );
    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn a_refusal_is_reported_as_a_failure_not_a_completion() {
    let fixture = Fixture::new();
    let Some((events, session)) = run_scenario(&fixture, "refusal", Vec::new(), arbiter()).await
    else {
        return;
    };

    let seen = kinds(&events);
    assert!(seen.contains(&"thread.failed"), "{seen:?}");
    assert!(
        !seen.contains(&"thread.completed"),
        "being declined is not finishing the work: {seen:?}"
    );
    // What the agent said is still kept.
    assert!(agent_said(&events).contains("I will not do that."));

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn a_banner_on_stdout_does_not_break_the_connection() {
    let fixture = Fixture::new();
    let Some((events, session)) = run_scenario(&fixture, "noise", Vec::new(), arbiter()).await
    else {
        return;
    };

    assert!(kinds(&events).contains(&"thread.completed"));
    assert!(agent_said(&events).contains("still here"));

    // The banner is kept as a diagnostic rather than discarded.
    let diagnostics = session.session.diagnostics();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("experimental mode")),
        "the banner should be recorded: {diagnostics:?}"
    );

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn the_session_reports_what_the_handshake_established() {
    let fixture = Fixture::new();
    let Some((_, session)) = run_scenario(&fixture, "basic", Vec::new(), arbiter()).await else {
        return;
    };

    let meta = session.session.session_metadata();
    assert_eq!(
        meta.resume_id.as_deref(),
        Some("fake-session-1"),
        "an agent that declares loadSession should be resumable"
    );
    assert_eq!(meta.permission_mode.as_deref(), Some("default"));

    // The modes offered are the agent's own, not a hard-coded list.
    let ids: Vec<&str> = meta.modes.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["default", "careful"]);

    let caps = session.session.capabilities();
    assert!(caps.resume.is_usable());
    assert!(caps.image_input.is_usable());
    assert!(caps.native_permission_bridge.is_usable());

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn resume_is_withheld_from_an_agent_that_cannot_load_a_session() {
    // Offering a resume that would fail is worse than not offering one.
    let fixture = Fixture::new();
    let env = vec![("FAKE_LOAD_SESSION".to_string(), "0".to_string())];
    let Some((events, session)) = run_scenario(&fixture, "basic", env, arbiter()).await else {
        return;
    };

    assert!(session.session.session_metadata().resume_id.is_none());
    assert!(!session.session.capabilities().resume.is_usable());

    let started = events
        .iter()
        .find_map(|e| match &e.payload {
            EventPayload::ThreadStarted { resume_id, .. } => Some(resume_id.clone()),
            _ => None,
        })
        .expect("no thread.started");
    assert!(
        started.is_none(),
        "thread.started must not offer a resume id"
    );

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn only_modes_the_agent_offers_are_accepted() {
    let fixture = Fixture::new();
    let Some((_, session)) = run_scenario(&fixture, "basic", Vec::new(), arbiter()).await else {
        return;
    };

    session
        .session
        .set_permission_mode("careful")
        .await
        .expect("a mode the agent offers should be accepted");
    assert_eq!(
        session
            .session
            .session_metadata()
            .permission_mode
            .as_deref(),
        Some("careful")
    );

    let err = session
        .session
        .set_permission_mode("made-up-mode")
        .await
        .expect_err("an unknown mode must be rejected");
    // The error names what is actually available, so the UI can say so.
    assert!(err.to_string().contains("careful"), "got {err}");

    let _ = session.session.shutdown().await;
}

#[tokio::test]
async fn a_second_prompt_while_a_turn_is_running_is_refused_clearly() {
    // ACP runs one turn at a time. Queuing silently would reorder the conversation.
    let Some(python) = python() else { return };
    let fixture = Fixture::new();

    let runtime = AcpRuntime::custom(
        "test-acp",
        "Test ACP agent",
        python,
        vec![fixture.script.display().to_string(), "deny".to_string()],
    )
    .with_arbiter(arbiter());

    let config = LaunchConfig::new(ThreadId::new(), fixture.root().display().to_string())
        .with_prompt("first");

    let mut launched = tokio::time::timeout(TIMEOUT, runtime.launch(config))
        .await
        .expect("launch timed out")
        .expect("launch failed");

    // The first turn blocks on the permission gate, so it is still open here.
    let err = launched
        .session
        .send_input("second".to_string(), Vec::new())
        .await
        .expect_err("a concurrent turn must be refused");
    assert!(
        err.to_string().contains("previous turn"),
        "the refusal should explain itself: {err}"
    );

    let _ = tokio::time::timeout(TIMEOUT, drain_until_settled(&mut launched)).await;
    let _ = launched.session.shutdown().await;
}

#[tokio::test]
async fn shutting_down_ends_the_agent_and_everything_it_started() {
    // A session ending must not leave anything running. Two things could leak: the
    // agent process itself, and the commands Tervin started for it. The command here
    // is `sleep 120`, so an orphan would be unmistakable.
    let Some(python) = python() else { return };
    let fixture = Fixture::new();
    let root = fixture.root();

    let runtime = AcpRuntime::custom(
        "test-acp",
        "Test ACP agent",
        python,
        vec![
            fixture.script.display().to_string(),
            "terminal-long".to_string(),
        ],
    )
    .with_arbiter(arbiter());

    let mut config =
        LaunchConfig::new(ThreadId::new(), root.display().to_string()).with_prompt("go");
    config.env = vec![("PROJECT_ROOT".to_string(), root.display().to_string())];

    let mut launched = tokio::time::timeout(TIMEOUT, runtime.launch(config))
        .await
        .expect("launch timed out")
        .expect("launch failed");

    // Wait until the command has actually started.
    let started = tokio::time::timeout(TIMEOUT, async {
        while let Some(event) = launched.events.recv().await {
            if matches!(event.payload, EventPayload::CommandStarted { .. }) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(started, "the command never started");
    assert!(launched.session.is_running());

    launched.session.shutdown().await.expect("shutdown failed");

    // The killed `sleep` reports its own end, which is the proof it did not survive.
    // The command's ending is reported by Tervin, which owned the process — not by
    // the agent, which may never have asked.
    let killed = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = launched.events.recv().await {
            if let EventPayload::CommandCompleted { command, .. } = &event.payload {
                if command.contains("sleep") {
                    return Some(event.summary.clone());
                }
            }
        }
        None
    })
    .await
    .expect("timed out waiting for the command to end");

    let summary = killed.expect("`sleep 120` was never reported as ended");
    assert!(
        summary.contains("terminated by Tervin"),
        "a killed process must not be reported as having exited normally: {summary}"
    );

    // And it is said out loud, rather than a process quietly disappearing.
    assert!(
        launched
            .session
            .diagnostics()
            .iter()
            .any(|d| d.message.contains("Terminated") && d.message.contains("sleep")),
        "killing the command should be recorded: {:?}",
        launched.session.diagnostics()
    );

    // And the agent process itself must be gone, not merely disconnected from.
    let stopped = tokio::time::timeout(Duration::from_secs(20), async {
        while launched.session.is_running() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        stopped.is_ok(),
        "the agent process outlived shutdown — closing stdin alone is not enough, \
         because the descriptor has to actually be dropped"
    );
}
