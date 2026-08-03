//! Asking the user's own shell what completes.
//!
//! The alternative approaches were weighed in COMPETITIVE-SPEC §3.2 and rejected:
//! shipping a corpus of specs goes stale and knows nothing about internal tools,
//! and running `--help` executes an arbitrary program out of `PATH` to find out
//! what its flags are. fish parses man pages specifically to avoid that.
//!
//! zsh's compsys already knows every spec the user has installed and is already
//! correct about them. So this drives a real zsh and reads what it offers. Nothing
//! is guessed, nothing the user has not already installed is consulted, and a
//! private tool with its own completion works for free.
//!
//! ## The parts that are not obvious
//!
//! Established against zsh 5.9 before any of this was written, and each one of
//! these failed silently rather than loudly when it was wrong:
//!
//! - Completion functions only exist inside a widget context, so there has to be an
//!   **interactive zsh on a pty** to send keystrokes to. A plain subshell has no
//!   completion system to ask.
//! - **`-f`** skips the user's rc files. Their plugins and prompt must not affect
//!   the answer, and their startup code should not run to service a keystroke.
//!   The cost is that `compinit` must be loaded explicitly.
//! - **`compinit -u -D`**: without `-u` it prompts about insecure directories and
//!   hangs; without `-D` it rebuilds the dump every time.
//! - **`LISTMAX` large and an empty `list-prompt`**, or zsh asks "do you wish to
//!   see all 114 possibilities" and lists nothing. That reads exactly like the
//!   technique not working.
//! - **`COLUMNS=1`** for one candidate per line. Packed columns cannot be split on
//!   whitespace, because candidates and descriptions contain spaces.
//! - At `COLUMNS=1` zsh writes **each character followed by a space and a
//!   backspace**, so `--all` arrives as `-·-·a·l·l·`. That is undone before
//!   anything else can be read.
//!
//! ## Synchronisation
//!
//! The genuinely hard part, and what makes this its own piece of work. Knowing when
//! a listing has finished needs a marker the child prints and a read loop that
//! waits for it; without one, the setup echo is read back as candidates. The prompt
//! is set to a marker that cannot occur in real output, and every step waits for it.
//!
//! Everything is bounded by a timeout. A shell that does not answer promptly is one
//! that offers no completions, never one that hangs the caller — this runs while
//! someone is typing.
//!
//! ## Status: not finished, and not wired up
//!
//! The decoding and parsing are done and tested. Driving zsh is not: it currently
//! returns no candidates, and two causes have been found and fixed on the way to
//! that, both of which the next attempt would otherwise rediscover.
//!
//! - **`TERM=dumb` disables ZLE**, and without ZLE there is no completion system at
//!   all — Tab is inserted as a literal tab and the reply is empty. It looks exactly
//!   like a shell with nothing installed.
//! - **The setup line contained the marker literally**, so the *echo* of it matched
//!   and the reader carried on before setup had run. Fixed by splitting the marker
//!   with an empty quote, the same trick the PTY tests use.
//!
//! What remains: after setup, two markers arrive together — the explicit `print -n`
//! and the redrawn prompt — so the read that should wait for the listing matches
//! the second of those immediately. Counting markers is the wrong primitive; the
//! next attempt should distinguish the prompt marker from a completion-finished
//! marker, most likely by binding a widget that prints a *different* marker after
//! `expand-or-complete` returns, so the two events are never confused.
//!
//! Nothing calls this yet. It compiles and its unit tests pass, and it is committed
//! rather than deleted because the two findings above cost real time to establish.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// One thing the shell offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCompletion {
    /// The candidate itself, e.g. `--all`.
    pub value: String,
    /// zsh's own description, where it gave one.
    pub description: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CompletionError {
    #[error("no usable shell for completion")]
    Unsupported,
    #[error("the shell did not answer in time")]
    TimedOut,
    #[error("could not drive the shell: {0}")]
    Io(String),
}

/// Printed by the child so the reader knows a step finished.
///
/// Deliberately not a word: it has to be impossible in real completion output, and
/// a plausible-looking marker is how a reader ends up treating a candidate as the
/// end of the listing.
const MARKER: &str = "@@TERVIN-READY@@";

/// The same marker, written so the shell's echo of the command cannot match it.
///
/// zsh echoes what is typed. A setup line containing the marker literally is a
/// setup line whose *echo* contains the marker, so the reader sees it before the
/// command has run and carries on to read the echo as candidates — precisely the
/// mistake the spike made and the spec warned about. Split by an empty quote, the
/// echo reads `@@TERVIN''-READY@@` while the value is unchanged.
const MARKER_LITERAL: &str = "@@TERVIN''-READY@@";

/// Ask zsh what completes `line`.
///
/// `line` is the command line as typed, e.g. `git commit -`. The returned
/// candidates are whatever the user's own compsys offers for it.
pub fn zsh_completions(
    line: &str,
    timeout: Duration,
) -> Result<Vec<ShellCompletion>, CompletionError> {
    let zsh = which_zsh().ok_or(CompletionError::Unsupported)?;

    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(portable_pty::PtySize {
            rows: 200,
            // One candidate per line. See the module comment: packed columns
            // cannot be split on whitespace without breaking on descriptions.
            cols: 1,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| CompletionError::Io(e.to_string()))?;

    let mut cmd = portable_pty::CommandBuilder::new(&zsh);
    cmd.arg("-f");
    cmd.arg("-i");
    // Not `dumb`. zsh disables ZLE on a terminal it cannot drive, and without ZLE
    // there is no completion system to ask: Tab is inserted as a literal tab and
    // the reply is empty. That failure looks exactly like a shell with no
    // completions installed, which is the wrong conclusion to draw from it.
    cmd.env("TERM", "xterm-256color");
    // The child must not inherit whatever started Tervin, for the same reason a
    // pane must not: it changes the answer for reasons the user cannot see.
    for key in ["ZDOTDIR", "CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION"] {
        cmd.env_remove(key);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| CompletionError::Io(e.to_string()))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| CompletionError::Io(e.to_string()))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| CompletionError::Io(e.to_string()))?;

    let deadline = Instant::now() + timeout;
    let result = (|| -> Result<Vec<ShellCompletion>, CompletionError> {
        // Setup, then a marker so the listing that follows cannot be confused with
        // the echo of this.
        let setup = format!(
            "PS1='{MARKER_LITERAL}'; PS2=''; RPS1=''; \
             LISTMAX=9999; \
             autoload -Uz compinit && compinit -u -D; \
             zstyle ':completion:*' list-prompt ''; \
             zstyle ':completion:*' menu no; \
             print -n '{MARKER_LITERAL}'\n"
        );
        write_all(&mut writer, setup.as_bytes())?;
        // Once: the echo can no longer contain it, so the first occurrence is the
        // real one, printed after everything above has actually run.
        read_until(&mut reader, MARKER, deadline, 1)?;

        // The line, then Tab. Tab is what asks compsys; nothing else does.
        write_all(&mut writer, line.as_bytes())?;
        write_all(&mut writer, b"\t")?;

        // zsh prints the listing and redraws the prompt, so the marker arrives
        // again on the far side of the candidates.
        let listing = read_until(&mut reader, MARKER, deadline, 1)?;
        Ok(parse_listing(&listing, line))
    })();

    let _ = writer.write_all(b"\x03\x04");
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn which_zsh() -> Option<String> {
    // The user's own shell when it is zsh, so their compsys version is the one
    // asked. Otherwise any zsh on PATH, because a bash user may still have one.
    if let Ok(shell) = std::env::var("SHELL") {
        if shell.ends_with("/zsh") && std::path::Path::new(&shell).exists() {
            return Some(shell);
        }
    }
    for candidate in ["/bin/zsh", "/usr/bin/zsh", "/usr/local/bin/zsh"] {
        if std::path::Path::new(candidate).exists() {
            return Some(candidate.to_string());
        }
    }
    None
}

fn write_all(w: &mut Box<dyn Write + Send>, bytes: &[u8]) -> Result<(), CompletionError> {
    w.write_all(bytes)
        .and_then(|()| w.flush())
        .map_err(|e| CompletionError::Io(e.to_string()))
}

/// Read until the marker has been seen `count` times, or time runs out.
///
/// Returns everything read. The caller decides which part of it matters, because
/// the marker appears around the interesting section rather than only after it.
fn read_until(
    reader: &mut Box<dyn Read + Send>,
    marker: &str,
    deadline: Instant,
    count: usize,
) -> Result<String, CompletionError> {
    let mut acc = String::new();
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                acc.push_str(&String::from_utf8_lossy(&buf[..n]));
                if decode(&acc).matches(marker).count() >= count {
                    return Ok(acc);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(CompletionError::Io(e.to_string())),
        }
    }
    Err(CompletionError::TimedOut)
}

/// Undo the terminal's own rendering: `X \b` pairs first, then escape sequences.
///
/// The order matters. At `COLUMNS=1` zsh writes every character followed by a
/// space and a backspace, so the text is interleaved with padding before any
/// escape sequence is even considered.
pub fn decode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // A character, a space, and a backspace: keep the character.
        if i + 2 < bytes.len() && bytes[i + 1] == b' ' && bytes[i + 2] == 0x08 {
            out.push(bytes[i] as char);
            i += 3;
            continue;
        }
        if bytes[i] == 0x1B {
            i += 1;
            if i >= bytes.len() {
                break;
            }
            match bytes[i] {
                b'[' => {
                    i += 1;
                    while i < bytes.len() && !(0x40..=0x7E).contains(&bytes[i]) {
                        i += 1;
                    }
                    i += 1;
                }
                b']' => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != 0x07 {
                        i += 1;
                    }
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        if bytes[i] == 0x08 || bytes[i] == b'\r' {
            i += 1;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Pull candidates out of a decoded listing.
///
/// Everything before the first marker is the echo of the setup and the line the
/// caller typed; treating it as candidates is the mistake the spike made.
fn parse_listing(raw: &str, line: &str) -> Vec<ShellCompletion> {
    let decoded = decode(raw);
    // Behind an env var because the next person to work on this will need it: the
    // failures here are silent and look identical to "this shell has no
    // completions", so seeing the actual bytes is the difference between an hour
    // and a day.
    if std::env::var_os("TERVIN_COMP_DEBUG").is_some() {
        eprintln!("--- RAW ---\n{raw:?}\n--- DECODED ---\n{decoded}\n--- END ---");
    }
    let body = match decoded.split_once(MARKER) {
        Some((_, rest)) => rest,
        None => &decoded,
    };
    // The typed line is echoed back before the listing; drop it so a prefix is
    // never offered as a completion of itself.
    let typed = line.trim();

    let mut out = Vec::new();
    let mut seen = Vec::new();
    for candidate in body.lines() {
        let text = candidate.trim();
        if text.is_empty() || text.contains(MARKER) || text == typed {
            continue;
        }
        // zsh separates a candidate from its description with whitespace once the
        // list is one per line. Only the first run splits: a description contains
        // spaces of its own.
        let (value, description) = match text.split_once("  ") {
            Some((v, d)) => (v.trim(), Some(d.trim().to_string())),
            None => (text, None),
        };
        if value.is_empty() || seen.iter().any(|s| s == value) {
            continue;
        }
        seen.push(value.to_string());
        out.push(ShellCompletion {
            value: value.to_string(),
            description: description.filter(|d| !d.is_empty()),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_padding_is_undone_before_anything_else() {
        // What zsh actually writes at COLUMNS=1: every character followed by a
        // space and a backspace. Read without undoing this, `--all` is unreadable
        // and every candidate looks like nonsense.
        let raw = "- \u{8}- \u{8}a \u{8}l \u{8}l \u{8}";
        assert_eq!(decode(raw), "--all");
    }

    #[test]
    fn escape_sequences_come_off_too() {
        assert_eq!(decode("\u{1b}[1m--force\u{1b}[0m"), "--force");
        assert_eq!(decode("\u{1b}]0;title\u{7}ok"), "ok");
    }

    #[test]
    fn the_echo_before_the_marker_is_not_a_candidate() {
        // The mistake the spike made: reading the setup echo back as completions.
        let raw = format!("compinit -u -D\n{MARKER}\n--all\n--amend  amend the commit\n");
        let got = parse_listing(&raw, "git commit -");
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].value, "--all");
        assert_eq!(got[1].value, "--amend");
        assert_eq!(got[1].description.as_deref(), Some("amend the commit"));
    }

    #[test]
    fn the_typed_line_is_not_offered_as_its_own_completion() {
        let raw = format!("{MARKER}\ngit commit -\n--all\n");
        let got = parse_listing(&raw, "git commit -");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].value, "--all");
    }

    #[test]
    fn a_shell_that_cannot_be_driven_is_not_an_error_worth_raising() {
        // The contract the caller depends on: completion degrades to whatever
        // path and history completion already do, and never blocks typing.
        let got = zsh_completions("git commit -", Duration::from_millis(1));
        assert!(
            matches!(
                got,
                Err(CompletionError::TimedOut) | Err(CompletionError::Unsupported) | Ok(_)
            ),
            "unexpected: {got:?}"
        );
    }
}
