//! Interpreting OSC payloads as shell-integration signals.
//!
//! Tervin speaks the established sequences rather than inventing its own where
//! one exists: OSC 7 for cwd, OSC 8 for hyperlinks, OSC 52 for clipboard, and
//! OSC 133 (semantic prompt) for prompt and command boundaries. A terminal that
//! already emits these gets Blocks with no Tervin-specific setup.
//!
//! It also reads OSC 777;notify, which serves two purposes: a program asking for
//! a desktop notification, and — when the title names `warp://cli-agent` — a coding
//! agent reporting its own lifecycle from inside a pane. Tervin reads the sequence
//! agents already emit rather than asking them to emit a second Tervin-shaped one.
//!
//! The one addition is OSC 7373, which carries what OSC 133 has no field for —
//! most importantly the submitted command text. Scraping the command off the
//! screen between the `B` and `C` marks is what other terminals do, and it is
//! unreliable with multi-line prompts, right-hand prompts, and reflow. Having
//! the shell state it explicitly is exact.

use crate::osc::split_params;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Tervin's private OSC number for metadata the standard sequences omit.
pub const TERVIN_OSC: &str = "7373";

/// A recognised shell-integration or terminal signal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum ShellSignal {
    /// OSC 133;A — the shell is about to draw a prompt.
    PromptStart,
    /// OSC 133;B — prompt drawn; the user is now typing.
    PromptEnd,
    /// OSC 133;C — the shell is handing off to the command.
    CommandExecuted,
    /// OSC 133;D[;exit] — the command finished.
    CommandFinished { exit_code: Option<i32> },
    /// OSC 7 — current working directory, and the host that owns it.
    Cwd { host: Option<String>, path: String },
    /// OSC 0 / OSC 2 — window or icon title.
    Title { title: String },
    /// OSC 8 — hyperlink open (`uri` non-empty) or close (`uri` empty).
    Hyperlink { uri: String, id: Option<String> },
    /// OSC 52 — the program asked to write the system clipboard.
    ///
    /// Deliberately surfaced as a request rather than performed here: a remote
    /// host must not silently take the local clipboard.
    ClipboardWriteRequested { selection: String, bytes: Vec<u8> },
    /// OSC 7373 — Tervin metadata for the current command.
    Meta { meta: CommandMeta },
    /// OSC 777;notify — a desktop notification request.
    ///
    /// Surfaced rather than performed, for the same reason as the clipboard: a
    /// process, possibly on a remote host, must not be able to raise a system
    /// notification without the terminal deciding to.
    Notification { title: String, body: String },
    /// OSC 777;notify;`warp://cli-agent` — a coding agent reporting its own
    /// lifecycle from inside a pane.
    AgentActivity { activity: AgentActivity },
}

/// The notification title an agent uses to mark its payload as machine-readable
/// rather than something to show a person.
///
/// The `warp://` scheme is Warp's, and Tervin reads it rather than asking agents
/// to emit a second Tervin-shaped sequence: the point is to work with agents as
/// they already ship. Nothing about handling it is Warp-specific.
pub const AGENT_NOTIFY_TARGET: &str = "warp://cli-agent";

/// What an agent said it was doing.
///
/// Modelled on the envelope Claude Code 2.1.220 actually emits, captured from a
/// real PTY:
///
/// ```text
/// OSC 777 ; notify ; warp://cli-agent ; {"v":1,"agent":"claude",
///   "event":"prompt_submit","session_id":"…","cwd":"…","project":"…",
///   "query":"reply with only the word ok"} BEL
/// ```
///
/// Only the interactive TUI emits it — `claude -p` does not — and it is *not*
/// gated on `TERM_PROGRAM`, which was worth verifying: Tervin sets its own, so a
/// Warp-only check would have made this dead code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentActivity {
    /// Which agent, as it names itself: `claude`, and whatever else adopts this.
    pub agent: String,
    pub event: AgentEvent,
    /// The agent's own session identifier, stable across a whole session. This
    /// is what lets separate notifications be stitched into one Thread.
    pub session_id: String,
    pub cwd: Option<String>,
    pub project: Option<String>,
    /// The prompt text, on `prompt_submit`.
    pub query: Option<String>,
    /// The reply, on `stop`. Claude Code sends this empty and points at the
    /// transcript instead.
    pub response: Option<String>,
    /// Path to the session transcript, on `stop`.
    ///
    /// The useful field: it means a session someone ran themselves in a pane can
    /// be read in full, rather than reconstructed from what happened to be
    /// announced.
    pub transcript_path: Option<String>,
    /// Envelope version. Recorded rather than enforced — refusing an unknown
    /// version would break on the next release for no benefit, since every field
    /// is already optional.
    pub v: Option<u32>,
    pub plugin_version: Option<String>,
}

/// The lifecycle points an agent reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvent {
    /// The agent started and is waiting for a prompt.
    SessionStart,
    /// The user submitted a prompt. Carries the text.
    PromptSubmit,
    /// The agent finished a turn and is idle again.
    Stop,
    /// Something this build does not model.
    ///
    /// Kept rather than discarded, and reported as unrecognised rather than
    /// guessed at — the same rule the runtime adapters follow. An agent adding an
    /// event type must not make Tervin drop the session.
    #[serde(untagged)]
    Other(String),
}

impl AgentEvent {
    /// A stable name for display and storage.
    pub fn as_str(&self) -> &str {
        match self {
            Self::SessionStart => "session_start",
            Self::PromptSubmit => "prompt_submit",
            Self::Stop => "stop",
            Self::Other(other) => other,
        }
    }
}

/// Per-command metadata reported by the shell hook.
///
/// Every field is optional: the hooks degrade gracefully, and a shell without
/// Git installed simply reports no branch rather than failing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandMeta {
    /// The submitted command line, exactly as the user entered it.
    pub command: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: Option<bool>,
    pub git_repo: Option<String>,
    pub host: Option<String>,
    pub user: Option<String>,
    pub shell: Option<String>,
    /// Duration the shell itself measured, in milliseconds.
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
}

impl CommandMeta {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Interpret one OSC payload.
///
/// Returns `None` for sequences Tervin does not consume — which is most of them.
/// Unrecognised sequences are simply not our business; they still reach the
/// renderer untouched.
pub fn parse(payload: &[u8]) -> Option<ShellSignal> {
    let parts = split_params(payload, 2);
    let code = std::str::from_utf8(parts[0]).ok()?;
    let rest: &[u8] = if parts.len() > 1 { parts[1] } else { b"" };

    match code {
        "0" | "2" => Some(ShellSignal::Title {
            title: String::from_utf8_lossy(rest).to_string(),
        }),
        "7" => parse_osc7(rest),
        "8" => parse_osc8(rest),
        "52" => parse_osc52(rest),
        "133" => parse_osc133(rest),
        "777" => parse_osc777(rest),
        TERVIN_OSC => parse_tervin(rest),
        _ => None,
    }
}

/// `OSC 7 ; file://host/percent/encoded/path`
fn parse_osc7(rest: &[u8]) -> Option<ShellSignal> {
    let url = std::str::from_utf8(rest).ok()?;
    let after_scheme = url.strip_prefix("file://").unwrap_or(url);
    let (host, raw_path) = match after_scheme.find('/') {
        Some(i) => (&after_scheme[..i], &after_scheme[i..]),
        None => ("", after_scheme),
    };
    let path = percent_decode(raw_path);
    if path.is_empty() {
        return None;
    }
    Some(ShellSignal::Cwd {
        host: if host.is_empty() {
            None
        } else {
            Some(host.to_string())
        },
        path,
    })
}

/// `OSC 8 ; params ; uri` — params may carry `id=…`.
fn parse_osc8(rest: &[u8]) -> Option<ShellSignal> {
    let parts = split_params(rest, 2);
    if parts.len() < 2 {
        return None;
    }
    let params = std::str::from_utf8(parts[0]).unwrap_or("");
    let uri = std::str::from_utf8(parts[1]).ok()?.to_string();
    let id = params
        .split(':')
        .find_map(|kv| kv.strip_prefix("id="))
        .map(|s| s.to_string());
    Some(ShellSignal::Hyperlink { uri, id })
}

/// `OSC 52 ; selection ; base64`
fn parse_osc52(rest: &[u8]) -> Option<ShellSignal> {
    let parts = split_params(rest, 2);
    if parts.len() < 2 {
        return None;
    }
    let selection = String::from_utf8_lossy(parts[0]).to_string();
    let b64 = std::str::from_utf8(parts[1]).ok()?;
    // `?` is a read request, which Tervin never answers: replying would leak the
    // local clipboard to whatever is running, including a remote host.
    if b64 == "?" {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    Some(ShellSignal::ClipboardWriteRequested { selection, bytes })
}

/// `OSC 133 ; A|B|C|D [; exit] [; k=v …]`
fn parse_osc133(rest: &[u8]) -> Option<ShellSignal> {
    let parts = split_params(rest, 3);
    let kind = parts.first()?;
    match kind.first()? {
        b'A' => Some(ShellSignal::PromptStart),
        b'B' => Some(ShellSignal::PromptEnd),
        b'C' => Some(ShellSignal::CommandExecuted),
        b'D' => {
            let exit_code = parts
                .get(1)
                .and_then(|p| std::str::from_utf8(p).ok())
                .and_then(|s| s.trim().parse::<i32>().ok());
            Some(ShellSignal::CommandFinished { exit_code })
        }
        _ => None,
    }
}

/// `OSC 777 ; notify ; title ; body`
///
/// The sequence is urxvt's desktop-notification request, which agents overload:
/// the title slot names a machine-readable target and the body carries JSON. Both
/// readings are handled, because a real notification from a long build is a thing
/// a terminal should surface too.
fn parse_osc777(rest: &[u8]) -> Option<ShellSignal> {
    // Limit 3, so the body keeps any `;` of its own — a prompt containing one is
    // ordinary, and splitting on it would truncate the JSON into invalid text.
    let parts = split_params(rest, 3);
    if parts.first()? != b"notify" {
        return None;
    }
    let title = String::from_utf8_lossy(parts.get(1).copied().unwrap_or(b"")).to_string();
    let body = String::from_utf8_lossy(parts.get(2).copied().unwrap_or(b"")).to_string();

    if title == AGENT_NOTIFY_TARGET {
        // A malformed envelope is dropped rather than surfaced as a notification:
        // showing a person raw JSON would be worse than showing nothing.
        let activity = parse_agent_activity(&body)?;
        return Some(ShellSignal::AgentActivity { activity });
    }

    if title.is_empty() && body.is_empty() {
        return None;
    }
    Some(ShellSignal::Notification { title, body })
}

/// Parse the agent envelope, treating every field as optional.
///
/// Hand-mapped rather than derived, so that empty strings become `None`. Claude
/// Code sends `"query":""` and `"response":""` on `stop`, and recording those
/// verbatim would put blank rows in prompt history — which is exactly the kind of
/// thing that makes a search feature feel broken.
fn parse_agent_activity(body: &str) -> Option<AgentActivity> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = json.as_object()?;

    let text = |key: &str| -> Option<String> {
        obj.get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };

    // Without a session id there is nothing to attribute activity to, and two
    // unrelated sessions in two panes would merge into one Thread.
    let session_id = text("session_id")?;
    let event = obj.get("event").and_then(|v| v.as_str())?;

    Some(AgentActivity {
        agent: text("agent").unwrap_or_else(|| "agent".to_string()),
        event: match event {
            "session_start" => AgentEvent::SessionStart,
            "prompt_submit" => AgentEvent::PromptSubmit,
            "stop" => AgentEvent::Stop,
            other => AgentEvent::Other(other.to_string()),
        },
        session_id,
        cwd: text("cwd"),
        project: text("project"),
        query: text("query"),
        response: text("response"),
        transcript_path: text("transcript_path"),
        v: obj.get("v").and_then(|v| v.as_u64()).map(|v| v as u32),
        plugin_version: text("plugin_version"),
    })
}

/// `OSC 7373 ; k=v ; k=v …` — values that can contain `;` or newlines are
/// base64-encoded by the shell hook.
fn parse_tervin(rest: &[u8]) -> Option<ShellSignal> {
    let text = std::str::from_utf8(rest).ok()?;
    let mut meta = CommandMeta::default();

    for field in text.split(';') {
        let (key, value) = match field.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        match key {
            "cmd" => meta.command = decode_b64_utf8(value),
            "branch" => meta.git_branch = decode_b64_utf8(value),
            "repo" => meta.git_repo = decode_b64_utf8(value),
            "dirty" => meta.git_dirty = Some(value == "1"),
            "host" => meta.host = Some(value.to_string()),
            "user" => meta.user = Some(value.to_string()),
            "shell" => meta.shell = Some(value.to_string()),
            "dur" => meta.duration_ms = value.parse().ok(),
            "exit" => meta.exit_code = value.parse().ok(),
            _ => {}
        }
    }

    if meta.is_empty() {
        return None;
    }
    Some(ShellSignal::Meta { meta })
}

fn decode_b64_utf8(value: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).to_string())
}

/// Decode `%XX` escapes. Invalid escapes are passed through literally rather
/// than dropped, so an odd path is still openable.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semantic_prompt_marks() {
        assert_eq!(parse(b"133;A"), Some(ShellSignal::PromptStart));
        assert_eq!(parse(b"133;B"), Some(ShellSignal::PromptEnd));
        assert_eq!(parse(b"133;C"), Some(ShellSignal::CommandExecuted));
        assert_eq!(
            parse(b"133;D;130"),
            Some(ShellSignal::CommandFinished {
                exit_code: Some(130)
            })
        );
        // A shell that reports no status still marks the boundary.
        assert_eq!(
            parse(b"133;D"),
            Some(ShellSignal::CommandFinished { exit_code: None })
        );
    }

    #[test]
    fn parses_cwd_with_host_and_percent_escapes() {
        assert_eq!(
            parse(b"7;file://mac.local/Users/dev/my%20project"),
            Some(ShellSignal::Cwd {
                host: Some("mac.local".to_string()),
                path: "/Users/dev/my project".to_string()
            })
        );
    }

    #[test]
    fn parses_hyperlink_id() {
        assert_eq!(
            parse(b"8;id=abc;https://example.com/x"),
            Some(ShellSignal::Hyperlink {
                uri: "https://example.com/x".to_string(),
                id: Some("abc".to_string())
            })
        );
    }

    #[test]
    fn ignores_clipboard_read_requests() {
        // Answering OSC 52 `?` would hand the local clipboard to whatever asked,
        // including a remote host. Tervin never replies.
        assert_eq!(parse(b"52;c;?"), None);
    }

    #[test]
    fn surfaces_clipboard_writes_as_requests() {
        let sig = parse(b"52;c;aGVsbG8=").unwrap();
        assert_eq!(
            sig,
            ShellSignal::ClipboardWriteRequested {
                selection: "c".to_string(),
                bytes: b"hello".to_vec()
            }
        );
    }

    #[test]
    fn parses_tervin_metadata_with_encoded_command() {
        let cmd = base64::engine::general_purpose::STANDARD.encode("git commit -m 'a;b'");
        let branch = base64::engine::general_purpose::STANDARD.encode("main");
        let payload = format!("7373;cmd={cmd};branch={branch};dirty=1;dur=1234;exit=0");
        let sig = parse(payload.as_bytes()).unwrap();
        match sig {
            ShellSignal::Meta { meta } => {
                // The semicolon inside the command survives, which is the whole
                // reason the value is base64-encoded.
                assert_eq!(meta.command.as_deref(), Some("git commit -m 'a;b'"));
                assert_eq!(meta.git_branch.as_deref(), Some("main"));
                assert_eq!(meta.git_dirty, Some(true));
                assert_eq!(meta.duration_ms, Some(1234));
                assert_eq!(meta.exit_code, Some(0));
            }
            other => panic!("expected Meta, got {other:?}"),
        }
    }

    #[test]
    fn ignores_unknown_osc_codes() {
        assert_eq!(parse(b"1337;File=name"), None);
        assert_eq!(parse(b"4;1;rgb:00/00/00"), None);
    }

    // The exact bytes Claude Code 2.1.220 wrote to a PTY, captured rather than
    // written from the documentation — the fields below are what it really sends,
    // including the ones it sends empty.
    const CLAUDE_SESSION_START: &[u8] = br#"777;notify;warp://cli-agent;{"v":1,"agent":"claude","event":"session_start","session_id":"c3a5583d-3934-47a5-9b21-0549acae939e","cwd":"/tmp/scratch","project":"scratch","plugin_version":"2.1.0"}"#;

    const CLAUDE_PROMPT: &[u8] = br#"777;notify;warp://cli-agent;{"v":1,"agent":"claude","event":"prompt_submit","session_id":"c3a5583d","cwd":"/tmp/scratch","project":"scratch","query":"reply with only the word ok"}"#;

    const CLAUDE_STOP: &[u8] = br#"777;notify;warp://cli-agent;{"v":1,"agent":"claude","event":"stop","session_id":"c3a5583d","cwd":"/tmp/scratch","project":"scratch","query":"","response":"","transcript_path":"/Users/dev/.claude/projects/scratch/c3a5583d.jsonl"}"#;

    fn activity(payload: &[u8]) -> AgentActivity {
        match parse(payload) {
            Some(ShellSignal::AgentActivity { activity }) => activity,
            other => panic!("expected agent activity, got {other:?}"),
        }
    }

    #[test]
    fn reads_a_real_session_start_notification() {
        let a = activity(CLAUDE_SESSION_START);
        assert_eq!(a.agent, "claude");
        assert_eq!(a.event, AgentEvent::SessionStart);
        assert_eq!(a.session_id, "c3a5583d-3934-47a5-9b21-0549acae939e");
        assert_eq!(a.cwd.as_deref(), Some("/tmp/scratch"));
        assert_eq!(a.project.as_deref(), Some("scratch"));
        assert_eq!(a.plugin_version.as_deref(), Some("2.1.0"));
        assert_eq!(a.v, Some(1));
    }

    #[test]
    fn reads_a_real_prompt_submission() {
        let a = activity(CLAUDE_PROMPT);
        assert_eq!(a.event, AgentEvent::PromptSubmit);
        assert_eq!(a.query.as_deref(), Some("reply with only the word ok"));
    }

    #[test]
    fn a_stop_gives_a_transcript_path_and_no_empty_prompt() {
        let a = activity(CLAUDE_STOP);
        assert_eq!(a.event, AgentEvent::Stop);
        // The whole point of the stop event: the transcript is on disk, so the
        // session can be read in full rather than pieced together.
        assert!(a.transcript_path.as_deref().unwrap().ends_with(".jsonl"));
        // Sent as "", and must not become a blank row in prompt history.
        assert_eq!(a.query, None);
        assert_eq!(a.response, None);
    }

    #[test]
    fn a_prompt_containing_a_semicolon_survives_parameter_splitting() {
        // OSC parameters are `;`-separated, and a prompt may legitimately contain
        // one. Splitting past the body would truncate the JSON into garbage.
        let a = activity(
            br#"777;notify;warp://cli-agent;{"agent":"claude","event":"prompt_submit","session_id":"s1","query":"run: a; then b"}"#,
        );
        assert_eq!(a.query.as_deref(), Some("run: a; then b"));
    }

    #[test]
    fn an_unmodelled_event_type_is_kept_rather_than_dropped() {
        // An agent adding a lifecycle event must not make Tervin lose the session.
        let a = activity(
            br#"777;notify;warp://cli-agent;{"agent":"claude","event":"tool_use_start","session_id":"s1"}"#,
        );
        assert_eq!(a.event, AgentEvent::Other("tool_use_start".to_string()));
        assert_eq!(a.event.as_str(), "tool_use_start");
    }

    #[test]
    fn an_agent_that_is_not_claude_is_read_the_same_way() {
        // Nothing here is Claude-specific; any agent adopting the envelope works.
        let a = activity(
            br#"777;notify;warp://cli-agent;{"agent":"codex","event":"session_start","session_id":"abc"}"#,
        );
        assert_eq!(a.agent, "codex");
    }

    #[test]
    fn a_plain_desktop_notification_is_not_mistaken_for_an_agent() {
        assert_eq!(
            parse(b"777;notify;Build finished;42 tests passed"),
            Some(ShellSignal::Notification {
                title: "Build finished".to_string(),
                body: "42 tests passed".to_string(),
            })
        );
    }

    #[test]
    fn a_malformed_agent_envelope_is_dropped_not_shown_as_a_notification() {
        // Raw JSON in a notification banner is worse than no notification.
        assert_eq!(parse(b"777;notify;warp://cli-agent;not json at all"), None);
        // No session id: there would be nothing to attribute the activity to, and
        // two panes' sessions would merge into one Thread.
        assert_eq!(
            parse(br#"777;notify;warp://cli-agent;{"agent":"claude","event":"stop"}"#),
            None
        );
    }

    #[test]
    fn osc_777_that_is_not_a_notify_request_is_ignored() {
        assert_eq!(parse(b"777;precmd;something"), None);
        assert_eq!(parse(b"777;notify"), None);
    }
}
