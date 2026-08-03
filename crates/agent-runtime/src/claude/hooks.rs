//! Tervin Rules as a real gate for Claude Code, via a `PreToolUse` hook.
//!
//! The `stream-json` protocol lets Tervin *watch* what an agent does. Hooks let it
//! *stop* one. A `PreToolUse` hook runs before a tool executes and can deny it, so
//! this is what turns Tervin's risk assessment from a label into an answer.
//!
//! ## How it fits together
//!
//! 1. Tervin opens a Unix socket in its own runtime directory, owner-only.
//! 2. It writes a settings file registering a `PreToolUse` hook that runs Tervin's
//!    own executable with `--tervin-hook <socket>`, and passes it to the agent with
//!    `--settings`. That flag loads *additional* settings, so the user's own
//!    configuration is never read, rewritten, or overridden.
//! 3. Before each tool call the agent runs that command, handing it the tool name
//!    and arguments on stdin.
//! 4. The command asks the running Tervin, which consults Tervin Rules. A refusal is
//!    written to stderr and exits 2. Anything else says nothing and exits 0.
//!
//! ## Tervin can only tighten, never loosen
//!
//! The protocol offers `allow`, which skips the runtime's *own* permission checks.
//! Tervin never returns it. When Rules do not object the hook says **nothing at all**
//! and exits 0, which is how this protocol spells "no opinion — carry on through the
//! normal flow". So enabling this gate can only ever add a refusal; it can never turn
//! an action the runtime would have asked about into one it performs silently. A
//! safety feature that quietly disables another safety feature is not one.
//!
//! Saying "no opinion" out loud is not equivalent to staying quiet. The runtime
//! accepts `allow`, `deny` and `ask` and nothing else; anything else ends the turn
//! immediately and reports success. Since this gate sees every tool call, that
//! landed on the first one and killed every Thread at its first action.
//!
//! ## What this gate cannot do
//!
//! Exit code 0 with no decision means "no opinion", and any exit code other than 2
//! is a non-blocking error — so if Tervin is unreachable or slow, **the tool call
//! proceeds**. The gate fails open. That is a property of the hook design, not a
//! choice, and it is stated in the session's permission explanation rather than
//! hidden: an ACP agent blocked on `session/request_permission` is a strictly
//! stronger gate than this one.

use crate::runtime::{ArbiterDecision, PermissionArbiter};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tervin_core::ThreadId;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// How long the agent waits for Tervin before treating the hook as failed.
///
/// Short on purpose. A decision is a lock acquisition and a pattern match; if it
/// has not happened in a second something is wrong, and blocking the agent for
/// longer trades a real stall against a gate that has already failed.
pub const HOOK_TIMEOUT_SECS: u64 = 5;

/// Largest hook payload Tervin will read.
///
/// Tool input can contain a whole file's contents, so this is generous — but not
/// unbounded, because the hook socket is reachable by anything running as this user.
const MAX_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

/// What a `PreToolUse` hook receives on stdin.
///
/// Deliberately partial: the runtime sends more than this and adds fields over
/// time, so unknown ones are ignored rather than making the hook fail — a failing
/// hook is a hole in the gate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HookRequest {
    pub hook_event_name: String,
    pub session_id: String,
    pub cwd: String,
    pub tool_name: String,
    pub tool_input: Value,
    pub tool_use_id: String,
    /// The runtime's own mode, recorded so the timeline can show the two together.
    pub permission_mode: String,
}

/// Tervin's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookDecision {
    /// Rules refused. The tool does not run.
    Deny { reason: String },
    /// Rules had no objection. The runtime's own checks still apply.
    Defer { reason: String },
}

impl HookDecision {
    /// The decision as it travels from Tervin to its own hook client.
    ///
    /// Not what the client prints. `defer` is Tervin's word for "no objection" and
    /// is meaningless to the runtime, which would end the turn on being handed it —
    /// so only a denial reaches the runtime, as a reason on stderr with exit 2.
    pub fn to_json(&self) -> Value {
        let (decision, reason) = match self {
            Self::Deny { reason } => ("deny", reason),
            Self::Defer { reason } => ("defer", reason),
        };
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": decision,
                "permissionDecisionReason": reason,
            }
        })
    }

    /// The process exit code that goes with this decision.
    ///
    /// A deny must exit 2: that is the only code the runtime treats as blocking.
    /// Printing a `deny` decision and exiting 0 would be read as advisory, and the
    /// tool would run.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Deny { .. } => 2,
            Self::Defer { .. } => 0,
        }
    }

    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }
}

// ------------------------------------------------------------------- settings

/// The settings Tervin passes with `--settings`.
///
/// Only hooks. Nothing else is set, so this cannot change a model, a tool
/// allowlist, or anything else the user configured — the file exists to add a gate
/// and to do nothing else.
pub fn hook_settings(executable: &Path, socket: &Path) -> Value {
    json!({
        "hooks": {
            "PreToolUse": [
                {
                    // Every tool, not only Bash: a write to a credential file or a
                    // web fetch is as much a decision as a command.
                    "matcher": "*",
                    "hooks": [
                        {
                            "type": "command",
                            "command": executable.display().to_string(),
                            "args": ["--tervin-hook", socket.display().to_string()],
                            "timeout": HOOK_TIMEOUT_SECS,
                            "statusMessage": "Checking Tervin Rules",
                        }
                    ]
                }
            ]
        }
    })
}

/// Write the settings file for one session and return its path.
///
/// Named by thread id so concurrent Threads cannot collide, and written with
/// owner-only permissions because it names the socket that answers for Tervin.
pub fn write_hook_settings(
    dir: &Path,
    thread_id: &ThreadId,
    executable: &Path,
    socket: &Path,
) -> std::io::Result<PathBuf> {
    tervin_core::paths::create_private_dir(dir)?;
    let path = dir.join(format!("hooks-{thread_id}.json"));
    let body = serde_json::to_string_pretty(&hook_settings(executable, socket))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, body)?;
    set_owner_only(&path)?;
    Ok(path)
}

/// The socket path for one session.
pub fn socket_path(dir: &Path, thread_id: &ThreadId) -> PathBuf {
    // Unix socket paths are length-limited (~104 bytes on macOS), so this stays
    // short rather than descriptive.
    dir.join(format!("h-{}.sock", short_id(thread_id)))
}

fn short_id(thread_id: &ThreadId) -> String {
    let text = thread_id.to_string();
    text.chars().rev().take(12).collect::<String>()
}

fn set_owner_only(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

// --------------------------------------------------------------------- server

/// A live hook gate for one session.
///
/// Dropping it removes the socket, so a finished Thread leaves nothing behind that
/// could answer for Tervin.
pub struct HookGate {
    socket: PathBuf,
    settings: PathBuf,
    /// Set the first time a hook actually calls in, which is the only proof the
    /// gate exists rather than being configured.
    confirmed: Arc<std::sync::atomic::AtomicBool>,
    /// Number of tool calls Tervin refused.
    denials: Arc<parking_lot::Mutex<Vec<String>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl HookGate {
    pub fn settings_path(&self) -> &Path {
        &self.settings
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    /// True once a hook has genuinely called in.
    pub fn confirmed(&self) -> bool {
        self.confirmed.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn denials(&self) -> Vec<String> {
        self.denials.lock().clone()
    }
}

impl Drop for HookGate {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(&self.settings);
    }
}

/// Something that turns a hook request into a decision.
///
/// Separate from [`PermissionArbiter`] so the gate can be tested without a rules
/// engine, and so a caller can record the decision in a Thread's timeline.
/// `decide` is async because the only handler that ships consults
/// [`PermissionArbiter`], which is async. It used to be sync, and `ArbiterHandler`
/// bridged the gap with `Handle::block_on`. That is called from `serve_one`, which
/// runs inside `tokio::spawn`, and `block_on` panics when called from within an
/// async context: the task died, the socket closed with no reply, and the hook
/// client waited its full timeout before failing open. Every single tool call.
///
/// The suite did not catch it because the only handler with a test was the trivial
/// one, which never called `block_on`. Awaiting properly removes the bridge rather
/// than moving it to another thread.
#[async_trait::async_trait]
pub trait HookHandler: Send + Sync {
    async fn decide(&self, request: &HookRequest) -> HookDecision;
}

/// Notified of every decision, so a caller can record it.
pub type DecisionObserver = Box<dyn Fn(&HookRequest, &HookDecision) + Send + Sync>;

/// A handler backed by Tervin Rules.
pub struct ArbiterHandler {
    arbiter: Arc<dyn PermissionArbiter>,
    thread_id: ThreadId,
    /// Where decisions go to become timeline events.
    ///
    /// A refusal that only appears in a status line is not an audit trail: the
    /// point of the gate is that what was stopped is inspectable afterwards.
    observer: Option<DecisionObserver>,
}

impl ArbiterHandler {
    pub fn new(arbiter: Arc<dyn PermissionArbiter>, thread_id: ThreadId) -> Self {
        Self {
            arbiter,
            thread_id,
            observer: None,
        }
    }

    /// Record every decision as it is made.
    pub fn observed_by(mut self, observer: DecisionObserver) -> Self {
        self.observer = Some(observer);
        self
    }
}

#[async_trait::async_trait]
impl HookHandler for ArbiterHandler {
    async fn decide(&self, request: &HookRequest) -> HookDecision {
        let decision = self
            .arbiter
            .decide(
                &self.thread_id,
                &request.tool_name,
                &request.tool_input,
                &request.cwd,
            )
            .await;

        let decision = match decision {
            ArbiterDecision::Deny { reason } => HookDecision::Deny {
                reason: format!("Denied by Tervin Rules: {reason}"),
            },
            // Never `allow`: see the module docs. Tervin can only tighten.
            ArbiterDecision::Allow => HookDecision::Defer {
                reason: "Tervin Rules did not object; the runtime's own checks still apply."
                    .to_string(),
            },
        };

        if let Some(observer) = &self.observer {
            observer(request, &decision);
        }
        decision
    }
}

/// Start the gate: bind the socket, write the settings, and serve decisions.
pub async fn start_gate(
    dir: &Path,
    thread_id: &ThreadId,
    executable: &Path,
    handler: Arc<dyn HookHandler>,
) -> std::io::Result<HookGate> {
    tervin_core::paths::create_private_dir(dir)?;
    let socket = socket_path(dir, thread_id);
    // A stale socket from a crashed session would make `bind` fail.
    let _ = std::fs::remove_file(&socket);

    let listener = tokio::net::UnixListener::bind(&socket)?;
    set_owner_only(&socket)?;

    let settings = write_hook_settings(dir, thread_id, executable, &socket)?;

    let confirmed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let denials = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

    {
        let confirmed = confirmed.clone();
        let denials = denials.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    result = listener.accept() => result,
                    _ = &mut shutdown_rx => break,
                };
                let Ok((stream, _)) = accepted else { break };

                let handler = handler.clone();
                let confirmed = confirmed.clone();
                let denials = denials.clone();
                // One task per call: a slow decision must not delay the next.
                tokio::spawn(async move {
                    confirmed.store(true, std::sync::atomic::Ordering::SeqCst);
                    if let Some(action) = serve_one(stream, handler).await {
                        denials.lock().push(action);
                    }
                });
            }
        });
    }

    Ok(HookGate {
        socket,
        settings,
        confirmed,
        denials,
        shutdown: Some(shutdown_tx),
    })
}

/// Answer one hook call. Returns the action if it was denied.
async fn serve_one(
    mut stream: tokio::net::UnixStream,
    handler: Arc<dyn HookHandler>,
) -> Option<String> {
    // One request per line, read with an explicit bound rather than `read_line`:
    // anything running as this user can reach the socket, so an unterminated write
    // must not be able to grow this buffer without limit.
    let mut buf: Vec<u8> = Vec::with_capacity(4 * 1024);
    {
        let mut reader = BufReader::new(&mut stream);
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
    }
    if buf.len() > MAX_PAYLOAD_BYTES {
        let decision = HookDecision::Deny {
            reason: format!(
                "Tervin refused a tool call whose input exceeded {} MB.",
                MAX_PAYLOAD_BYTES / (1024 * 1024)
            ),
        };
        let _ = respond(&mut stream, &decision).await;
        return Some("an oversized tool call".to_string());
    }

    let line = String::from_utf8_lossy(&buf).to_string();
    if line.trim().is_empty() {
        return None;
    }

    let request: HookRequest = match serde_json::from_str(line.trim()) {
        Ok(request) => request,
        Err(_) => {
            // Unreadable input must not silently allow. Refuse, and say why.
            let decision = HookDecision::Deny {
                reason: "Tervin could not read this tool call, so it was not allowed.".to_string(),
            };
            let _ = respond(&mut stream, &decision).await;
            return Some("an unreadable tool call".to_string());
        }
    };

    let decision = handler.decide(&request).await;
    let denied = decision.is_deny();
    let _ = respond(&mut stream, &decision).await;

    denied.then(|| describe_action(&request))
}

async fn respond(
    stream: &mut tokio::net::UnixStream,
    decision: &HookDecision,
) -> std::io::Result<()> {
    let mut body = serde_json::to_string(&json!({
        "decision": decision.to_json(),
        "exit_code": decision.exit_code(),
    }))
    .unwrap_or_default();
    body.push('\n');
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await
}

/// How an action reads in the timeline and in a denial list.
pub fn describe_action(request: &HookRequest) -> String {
    if let Some(command) = request.tool_input.get("command").and_then(Value::as_str) {
        return command.to_string();
    }
    for key in ["file_path", "path", "url", "pattern"] {
        if let Some(value) = request.tool_input.get(key).and_then(Value::as_str) {
            return format!("{}({value})", request.tool_name);
        }
    }
    request.tool_name.clone()
}

// --------------------------------------------------------------------- client

/// Run as the hook: read the request, ask Tervin, print the decision, and exit.
///
/// Returns the process exit code. Blocking and dependency-free on purpose — this
/// runs as a short-lived child process on the agent's critical path, so it does no
/// more than a connect, a write, and a read.
pub fn run_hook_client(socket: &Path) -> i32 {
    use std::io::{Read, Write};

    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        // Nothing to decide about. Exiting 1 is a non-blocking error, which leaves
        // the runtime's own flow intact rather than blocking a call Tervin never saw.
        eprintln!("{HOOK_STDERR_PREFIX}could not read the tool call from stdin.");
        return 1;
    }

    let mut stream = match std::os::unix::net::UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(e) => {
            // Tervin is gone. Say so loudly rather than failing silently: the user
            // believes actions are being gated, and right now they are not.
            eprintln!(
                "{HOOK_STDERR_PREFIX}could not reach Tervin at {} ({e}). This tool call \
                 was NOT checked against Tervin Rules.",
                socket.display()
            );
            return 1;
        }
    };

    let timeout = std::time::Duration::from_secs(HOOK_TIMEOUT_SECS);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let mut payload = input.trim().replace('\n', " ");
    payload.push('\n');
    if stream.write_all(payload.as_bytes()).is_err() || stream.flush().is_err() {
        eprintln!("{HOOK_STDERR_PREFIX}could not send the tool call to Tervin.");
        return 1;
    }

    let mut response = String::new();
    if std::io::BufRead::read_line(&mut std::io::BufReader::new(&mut stream), &mut response)
        .is_err()
        || response.trim().is_empty()
    {
        eprintln!("{HOOK_STDERR_PREFIX}Tervin did not answer within {HOOK_TIMEOUT_SECS}s.");
        return 1;
    }

    let Ok(value) = serde_json::from_str::<Value>(response.trim()) else {
        eprintln!("{HOOK_STDERR_PREFIX}Tervin's answer could not be read.");
        return 1;
    };

    let exit_code = value.get("exit_code").and_then(Value::as_i64).unwrap_or(1) as i32;
    let decision = value.get("decision").cloned().unwrap_or(Value::Null);

    if exit_code == 2 {
        // A blocking denial: the runtime reads stderr, not stdout, and feeds it back
        // to the agent so it understands why and can choose differently.
        let reason = decision
            .get("hookSpecificOutput")
            .and_then(|o| o.get("permissionDecisionReason"))
            .and_then(Value::as_str)
            .unwrap_or("Denied by Tervin Rules.");
        eprintln!("{reason}");
        return 2;
    }

    // Silence is how this protocol spells "no opinion", and saying anything else
    // here is not the harmless no-op it looks like.
    //
    // The runtime accepts `allow`, `deny` and `ask`. It does not accept `defer`, and
    // on receiving one it **ends the turn on the spot** and reports success: no tool
    // result, no continuation, the agent simply stops mid-task. Because Tervin gates
    // every tool call, that fired on the *first* one, so every Thread died at its
    // first action while the gate panel said it was protecting the session. Printing
    // a decision that meant "carry on" was the thing stopping everything.
    //
    // Tervin only ever tightens. When Rules do not object there is nothing to say,
    // and the exit code alone says it.
    0
}

/// The flag that turns Tervin's own executable into the hook.
pub const HOOK_FLAG: &str = "--tervin-hook";

/// The prefix every message the hook client prints to stderr carries.
///
/// This is how the normalizer recognises the gate's own failures. The runtime echoes
/// the hook's command line back only when the hook *blocked*; a hook that merely
/// failed carries nothing but its own stderr, so without a marker of its own every
/// gate failure was reported as the user's configuration.
///
/// It is deliberately the same string a reader sees, rather than a hidden token: the
/// text is already user-facing, and a sentinel nobody can read is one nobody
/// maintains. Denials are exempt — their stderr is fed back to the agent verbatim.
pub const HOOK_STDERR_PREFIX: &str = "Tervin hook: ";

/// Recognise a hook invocation before any UI starts.
///
/// The workspace binary doubles as the hook command so the path is exact and no
/// second artefact has to be installed or found on `PATH`.
pub fn hook_socket_from_args<I, S>(args: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg.as_ref() == HOOK_FLAG {
            return iter.next().map(|s| PathBuf::from(s.as_ref()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(tool: &str, input: Value) -> HookRequest {
        HookRequest {
            hook_event_name: "PreToolUse".into(),
            session_id: "s1".into(),
            cwd: "/tmp".into(),
            tool_name: tool.into(),
            tool_input: input,
            tool_use_id: "toolu_1".into(),
            permission_mode: "default".into(),
        }
    }

    /// A handler that denies anything containing a marker.
    struct Marker;
    #[async_trait::async_trait]
    impl HookHandler for Marker {
        async fn decide(&self, request: &HookRequest) -> HookDecision {
            if request.tool_input.to_string().contains("DENY-ME") {
                HookDecision::Deny {
                    reason: "Denied by Tervin Rules: test policy".into(),
                }
            } else {
                HookDecision::Defer {
                    reason: "no objection".into(),
                }
            }
        }
    }

    #[test]
    fn a_deny_exits_two_because_nothing_else_blocks() {
        // Printing a deny and exiting 0 would be read as advisory and the tool
        // would run. This is the single most important line in the file.
        let deny = HookDecision::Deny {
            reason: "no".into(),
        };
        assert_eq!(deny.exit_code(), 2);
        assert_eq!(
            HookDecision::Defer {
                reason: "fine".into()
            }
            .exit_code(),
            0
        );
    }

    #[test]
    fn tervin_never_returns_allow_so_it_can_only_tighten() {
        // `allow` skips the runtime's own permission checks. A gate that quietly
        // disables another gate is not a safety feature.
        for decision in [
            HookDecision::Deny { reason: "x".into() },
            HookDecision::Defer { reason: "y".into() },
        ] {
            let text = decision.to_json().to_string();
            assert!(
                !text.contains("\"allow\""),
                "Tervin must never answer `allow`: {text}"
            );
        }
    }

    #[test]
    fn the_decision_json_matches_the_documented_shape() {
        let json = HookDecision::Deny {
            reason: "Denied by Tervin Rules: rm -rf /".into(),
        }
        .to_json();
        let out = &json["hookSpecificOutput"];
        assert_eq!(out["hookEventName"], json!("PreToolUse"));
        assert_eq!(out["permissionDecision"], json!("deny"));
        assert_eq!(
            out["permissionDecisionReason"],
            json!("Denied by Tervin Rules: rm -rf /")
        );
    }

    #[test]
    fn the_settings_only_add_a_hook_and_nothing_else() {
        // `--settings` merges, so anything else in here would silently override the
        // user's own configuration.
        let settings = hook_settings(Path::new("/Apps/Tervin"), Path::new("/run/h.sock"));
        let keys: Vec<&String> = settings.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["hooks"], "settings must contain only hooks");

        let hook = &settings["hooks"]["PreToolUse"][0];
        // Every tool: a write to a credential file is as much a decision as a
        // command.
        assert_eq!(hook["matcher"], json!("*"));
        let handler = &hook["hooks"][0];
        assert_eq!(handler["type"], json!("command"));
        assert_eq!(handler["command"], json!("/Apps/Tervin"));
        assert_eq!(handler["args"], json!(["--tervin-hook", "/run/h.sock"]));
        // A gate with no timeout could hang the agent indefinitely.
        assert_eq!(handler["timeout"], json!(HOOK_TIMEOUT_SECS));
    }

    #[test]
    fn the_hook_flag_is_recognised_before_anything_else_starts() {
        assert_eq!(
            hook_socket_from_args(["tervin", "--tervin-hook", "/run/h.sock"]),
            Some(PathBuf::from("/run/h.sock"))
        );
        assert_eq!(hook_socket_from_args(["tervin"]), None);
        // A flag with no value must not be treated as a hook run.
        assert_eq!(hook_socket_from_args(["tervin", "--tervin-hook"]), None);
    }

    #[test]
    fn an_action_is_described_by_what_it_does() {
        assert_eq!(
            describe_action(&request("Bash", json!({"command": "rm -rf build"}))),
            "rm -rf build"
        );
        assert_eq!(
            describe_action(&request("Edit", json!({"file_path": "/p/src/lib.rs"}))),
            "Edit(/p/src/lib.rs)"
        );
        // No recognisable argument: the tool name alone, never an empty string.
        assert_eq!(
            describe_action(&request("TodoWrite", json!({}))),
            "TodoWrite"
        );
    }

    #[test]
    fn a_request_with_unknown_fields_still_parses() {
        // The runtime adds fields over time, and a hook that fails to parse is a
        // hole in the gate.
        let parsed: HookRequest = serde_json::from_value(json!({
            "hook_event_name": "PreToolUse",
            "session_id": "abc",
            "cwd": "/proj",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_use_id": "toolu_9",
            "permission_mode": "default",
            "effort": {"level": "high"},
            "something_added_in_a_later_version": 42
        }))
        .expect("unknown fields must not break parsing");
        assert_eq!(parsed.tool_name, "Bash");
    }

    #[test]
    fn a_socket_path_stays_inside_the_platform_limit() {
        // Unix socket paths are capped near 104 bytes on macOS; a long thread id
        // would otherwise make `bind` fail at runtime rather than here.
        let dir = tervin_core::paths::runtime_dir();
        let path = socket_path(&dir, &ThreadId::new());
        assert!(
            path.display().to_string().len() < 100,
            "socket path too long: {}",
            path.display()
        );
    }

    #[tokio::test]
    async fn a_denied_tool_call_is_blocked_end_to_end() {
        // The whole point, exercised over a real socket with the real client.
        let dir = tempfile::tempdir().unwrap();
        let thread_id = ThreadId::new();
        let gate = start_gate(
            dir.path(),
            &thread_id,
            Path::new("/Apps/Tervin"),
            Arc::new(Marker),
        )
        .await
        .expect("gate did not start");

        assert!(!gate.confirmed(), "nothing has called in yet");
        assert!(gate.settings_path().exists());

        let socket = gate.socket_path().to_path_buf();
        let payload = serde_json::to_string(&json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo DENY-ME"},
            "cwd": "/tmp",
        }))
        .unwrap();

        let (code, _out, stderr) = run_client_capturing(&socket, &payload).await;
        assert_eq!(code, 2, "a denial must exit 2 or the tool runs anyway");
        assert!(
            stderr.contains("test policy"),
            "the agent must be told why: {stderr}"
        );

        assert!(gate.confirmed(), "a real call in proves the gate exists");
        assert_eq!(gate.denials(), vec!["echo DENY-ME"]);
    }

    #[tokio::test]
    async fn an_unobjectionable_call_defers_instead_of_approving() {
        let dir = tempfile::tempdir().unwrap();
        let thread_id = ThreadId::new();
        let gate = start_gate(
            dir.path(),
            &thread_id,
            Path::new("/Apps/Tervin"),
            Arc::new(Marker),
        )
        .await
        .unwrap();

        let socket = gate.socket_path().to_path_buf();
        let payload = r#"{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"/p/a.rs"},"cwd":"/tmp"}"#;
        let (code, _out, _err) = run_client_capturing(&socket, payload).await;
        assert_eq!(code, 0);
        assert!(gate.denials().is_empty());
    }

    #[tokio::test]
    async fn an_unreadable_tool_call_is_refused_rather_than_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let thread_id = ThreadId::new();
        let gate = start_gate(
            dir.path(),
            &thread_id,
            Path::new("/Apps/Tervin"),
            Arc::new(Marker),
        )
        .await
        .unwrap();

        let socket = gate.socket_path().to_path_buf();
        let (code, _out, _err) = run_client_capturing(&socket, "this is not json").await;
        assert_eq!(code, 2, "input Tervin cannot read must not be allowed");
    }

    #[tokio::test]
    async fn an_unreachable_tervin_says_the_call_was_not_checked() {
        // The gate fails open by design. It must not fail *quietly*: the user
        // believes actions are being checked.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nothing.sock");
        let (code, _out, stderr) = run_client_capturing(&missing, "{}").await;
        // Not 2: blocking on Tervin being down would make an unrelated crash stop
        // the user's work.
        assert_eq!(code, 1);
        assert!(
            stderr.contains("NOT checked"),
            "a silent failure is the dangerous one: {stderr}"
        );
    }

    #[tokio::test]
    async fn dropping_the_gate_removes_the_socket_and_settings() {
        // A leftover socket would keep answering permission questions for a Thread
        // that no longer exists.
        let dir = tempfile::tempdir().unwrap();
        let thread_id = ThreadId::new();
        let (socket, settings) = {
            let gate = start_gate(
                dir.path(),
                &thread_id,
                Path::new("/Apps/Tervin"),
                Arc::new(Marker),
            )
            .await
            .unwrap();
            (
                gate.socket_path().to_path_buf(),
                gate.settings_path().to_path_buf(),
            )
        };
        assert!(!socket.exists(), "the socket outlived the session");
        assert!(!settings.exists(), "the settings file outlived the session");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_socket_and_settings_are_owner_only() {
        // The socket answers permission questions on Tervin's behalf, so its
        // permissions are the authentication.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let thread_id = ThreadId::new();
        let gate = start_gate(
            dir.path(),
            &thread_id,
            Path::new("/Apps/Tervin"),
            Arc::new(Marker),
        )
        .await
        .unwrap();

        for path in [gate.socket_path(), gate.settings_path()] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode & 0o077,
                0,
                "{} is reachable by others: {mode:o}",
                path.display()
            );
        }
    }

    /// The gate against the real Claude Code CLI.
    ///
    /// Everything else here proves the mechanism in isolation. This proves the thing
    /// that actually matters: that the runtime loads Tervin's settings, runs the
    /// hook, and *honours a refusal*. Nothing about the contract is assumed — the
    /// real binary decides.
    ///
    /// Costs tokens and needs both network and a signed-in account, so it runs only
    /// when asked: `TERVIN_LIVE_CLAUDE=1 cargo test -p agent-runtime -- --nocapture
    /// the_real_cli_honours_a_refusal`.
    #[tokio::test]
    async fn the_real_cli_honours_a_refusal() {
        if std::env::var("TERVIN_LIVE_CLAUDE").is_err() {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let thread_id = ThreadId::new();

        // The test binary stands in for the installed executable: invoked with the
        // hook flag, it answers as the client.
        let exe = std::env::current_exe().expect("no test binary");
        let gate = start_gate(dir.path(), &thread_id, &exe, Arc::new(DenyBash))
            .await
            .expect("gate did not start");

        let output = tokio::process::Command::new("claude")
            .args([
                "-p",
                "Run exactly this shell command and show me its output: echo tervin-gate-probe",
                "--settings",
            ])
            .arg(gate.settings_path())
            .args([
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "auto",
            ])
            .current_dir(project.path())
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .expect("could not run claude");

        let stream = String::from_utf8_lossy(&output.stdout);
        eprintln!("--- claude stream ---\n{stream}\n--- end ---");

        if stream.contains("OAuth access token has expired") {
            panic!("this account is not signed in; the gate could not be exercised");
        }

        assert!(
            gate.confirmed(),
            "the runtime never ran the hook, so Tervin's settings were not loaded"
        );
        assert!(
            !gate.denials().is_empty(),
            "the hook ran but no denial was recorded: {:?}",
            gate.denials()
        );
        // The refusal has to have reached the runtime, not just Tervin's own record.
        assert!(
            stream.contains("Tervin") || stream.contains("permission_denials\":[\"Bash"),
            "the runtime did not report Tervin's refusal"
        );
        // And the command must not have run.
        assert!(
            !stream.contains("tervin-gate-probe\\n"),
            "the blocked command appears to have produced output"
        );
    }

    /// An arbiter that answers across an await point.
    ///
    /// The await matters. `ArbiterHandler` used to bridge async to sync with
    /// `Handle::block_on`, and an arbiter that returned immediately could mask that.
    /// Yielding guarantees the handler is genuinely driven as a future.
    struct AsyncArbiter {
        deny_tool: String,
    }

    #[async_trait::async_trait]
    impl PermissionArbiter for AsyncArbiter {
        async fn decide(
            &self,
            _thread_id: &ThreadId,
            tool_name: &str,
            _input: &Value,
            _cwd: &str,
        ) -> ArbiterDecision {
            tokio::task::yield_now().await;
            if tool_name == self.deny_tool {
                ArbiterDecision::Deny {
                    reason: "the rules say no".into(),
                }
            } else {
                ArbiterDecision::Allow
            }
        }
    }

    fn arbiter_gate_handler(thread_id: &ThreadId) -> Arc<ArbiterHandler> {
        Arc::new(ArbiterHandler::new(
            Arc::new(AsyncArbiter {
                deny_tool: "Bash".into(),
            }),
            thread_id.clone(),
        ))
    }

    /// The handler that actually ships, driven through the real client and socket.
    ///
    /// This test did not exist, and its absence cost a release. `ArbiterHandler` is
    /// the only handler Tervin ever constructs, and it was the only one with no
    /// coverage: every other gate test used a trivial handler that returned a
    /// decision directly. So `Handle::block_on` inside `decide`, called from a task
    /// on a runtime worker, panicked in the real app and never in a test. The socket
    /// closed with no reply, the hook client waited out its full timeout, and the
    /// gate failed open on *every* tool call while the suite stayed green.
    ///
    /// Exit code 1 is the specific symptom: that is the client reporting it got no
    /// answer. Asserting on 2 and 0 rather than merely "not a deny" is what makes
    /// this catch a timeout rather than a wrong decision.
    #[tokio::test]
    async fn the_arbiter_backed_handler_answers_over_the_real_socket() {
        let dir = tempfile::tempdir().unwrap();
        let thread_id = ThreadId::new();
        let gate = start_gate(
            dir.path(),
            &thread_id,
            Path::new("/Apps/Tervin"),
            arbiter_gate_handler(&thread_id),
        )
        .await
        .expect("the gate should start");
        let socket = gate.socket_path().to_path_buf();

        let deny = r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"rm -rf /"},"cwd":"/tmp"}"#;
        let (code, _out, err) = run_client_capturing(&socket, deny).await;
        assert_ne!(
            code, 1,
            "exit 1 means the client never got an answer: {err}"
        );
        assert_eq!(
            code, 2,
            "a refusal must exit 2 or the tool still runs: {err}"
        );

        let allow = r#"{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"/p/a.rs"},"cwd":"/tmp"}"#;
        let (code, out, err) = run_client_capturing(&socket, allow).await;
        assert_ne!(code, 1, "exit 1 means no answer: {err}");
        assert_eq!(code, 0, "a permitted action defers, which exits 0: {err}");

        // Nothing on stdout, and this is the assertion the gate most needs.
        //
        // The runtime accepts `allow`, `deny` and `ask`. Handed anything else — such
        // as Tervin's own word for "no objection" — it ends the turn immediately and
        // reports success, so the agent stops at its first tool call having done
        // nothing. Every Thread died that way while the exit code, the stderr and
        // the audit trail all said the gate was working correctly, which is exactly
        // why stdout is captured here at all.
        assert!(
            out.trim().is_empty(),
            "the gate must say nothing when it does not object, got: {out:?}"
        );

        // The denial is recorded, because a refusal that is not inspectable
        // afterwards is not an audit trail.
        assert_eq!(gate.denials().len(), 1, "the deny should be recorded once");
    }

    /// Several calls at once, because an agent does not wait its turn.
    ///
    /// `serve_one` spawns a task per connection specifically so a slow decision does
    /// not delay the next, and nothing asserted that. It also fails if the handler
    /// blocks a runtime worker, since enough concurrent blocks starve the pool.
    #[tokio::test]
    async fn concurrent_hook_calls_are_all_answered() {
        let dir = tempfile::tempdir().unwrap();
        let thread_id = ThreadId::new();
        let gate = start_gate(
            dir.path(),
            &thread_id,
            Path::new("/Apps/Tervin"),
            arbiter_gate_handler(&thread_id),
        )
        .await
        .unwrap();
        let socket = gate.socket_path().to_path_buf();

        let mut tasks = Vec::new();
        for i in 0..6 {
            let socket = socket.clone();
            let tool = if i % 2 == 0 { "Bash" } else { "Read" };
            let payload = format!(
                r#"{{"hook_event_name":"PreToolUse","tool_name":"{tool}","tool_input":{{"n":{i}}},"cwd":"/tmp"}}"#
            );
            tasks.push(tokio::spawn(async move {
                run_client_capturing(&socket, &payload).await
            }));
        }

        let mut codes = Vec::new();
        for task in tasks {
            let (code, out, err) = task.await.unwrap();
            assert_ne!(code, 1, "a concurrent call went unanswered: {err}");
            assert!(
                out.trim().is_empty(),
                "no call may print a decision the runtime would choke on: {out:?}"
            );
            codes.push(code);
        }
        codes.sort_unstable();
        assert_eq!(codes, vec![0, 0, 0, 2, 2, 2]);
    }

    /// Refuses any shell command, so the live test has something unambiguous to see.
    struct DenyBash;
    #[async_trait::async_trait]
    impl HookHandler for DenyBash {
        async fn decide(&self, request: &HookRequest) -> HookDecision {
            if request.tool_name == "Bash" {
                HookDecision::Deny {
                    reason: "Denied by Tervin Rules: shell commands are refused in this test."
                        .into(),
                }
            } else {
                HookDecision::Defer {
                    reason: "no objection".into(),
                }
            }
        }
    }

    /// Run the real client against a socket, capturing exit code, stdout and stderr.
    ///
    /// The client is deliberately a blocking, process-shaped function, so it is
    /// driven here the same way the runtime drives it: a child process with the
    /// payload on stdin.
    ///
    /// Stdout is captured and returned because it was once discarded here, and the
    /// gate's worst bug lived in it: the client printed a decision the runtime does
    /// not accept, which ended every Thread at its first tool call. Exit codes and
    /// stderr were asserted throughout and all of them were correct.
    async fn run_client_capturing(socket: &Path, payload: &str) -> (i32, String, String) {
        let socket = socket.to_path_buf();
        let payload = payload.to_string();
        tokio::task::spawn_blocking(move || {
            // Re-exec this test binary in client mode: the only faithful way to
            // exercise stdin, stdout, stderr, and the exit code together.
            let exe = std::env::current_exe().expect("no test binary");
            let mut child = std::process::Command::new(exe)
                .arg(HOOK_FLAG)
                .arg(&socket)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("could not run the client");

            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(payload.as_bytes())
                .ok();

            let out = child.wait_with_output().expect("client did not finish");
            (
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout).to_string(),
                String::from_utf8_lossy(&out.stderr).to_string(),
            )
        })
        .await
        .expect("client task panicked")
    }

    /// Turn this test binary into the hook when it is invoked as one.
    ///
    /// Keyed on the same flag production uses, and running before any test does, so
    /// the test binary can stand in for the installed executable — including when
    /// the real `claude` CLI is the one launching it.
    #[ctor::ctor(unsafe)]
    fn client_mode() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if let Some(socket) = hook_socket_from_args(&args) {
            std::process::exit(run_hook_client(&socket));
        }
    }
}
