//! Interpreting OSC payloads as shell-integration signals.
//!
//! Tervin speaks the established sequences rather than inventing its own where
//! one exists: OSC 7 for cwd, OSC 8 for hyperlinks, OSC 52 for clipboard, and
//! OSC 133 (semantic prompt) for prompt and command boundaries. A terminal that
//! already emits these gets Blocks with no Tervin-specific setup.
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
}
