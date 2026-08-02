//! End-to-end PTY tests against a real shell.
//!
//! The unit tests cover the escape-sequence tap in isolation. These cover the
//! thing that isolation cannot: that a real shell starts, that keystrokes written
//! to the PTY reach it, that its output comes back through the coalescing pump,
//! and that shell-integration markers survive the round trip.
//!
//! This is the path a screenshot cannot verify. Seeing a prompt only proves output
//! flows; it says nothing about whether input does.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use terminal_core::{PtyConfig, PtyEvent, PtySession, ShellSignal};
use tervin_core::PaneId;

/// How long to wait for a shell to produce expected output.
///
/// Generous: a login shell sources the user's rc files, which on a real machine
/// can mean version managers and completion frameworks.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Collected output plus any signals seen.
struct Collected {
    text: String,
    signals: Vec<ShellSignal>,
}

/// Run `program` in a PTY, write `input`, and collect until `done` is satisfied.
fn run(program: &str, args: &[&str], input: &[&str], done: impl Fn(&str) -> bool) -> Collected {
    let pane = PaneId::new();
    let mut config = PtyConfig::command(
        pane.clone(),
        program,
        args.iter().map(|s| s.to_string()).collect(),
        Some(std::env::temp_dir().display().to_string()),
    );
    config.cols = 80;
    config.rows = 24;

    let (tx, rx) = mpsc::channel::<PtyEvent>();
    let sink = Arc::new(move |event: PtyEvent| {
        let _ = tx.send(event);
    });

    let session = PtySession::spawn(config, sink).expect("could not open a pty");

    // Give the shell a moment to reach its first prompt before typing, so input
    // is not swallowed by a shell still setting up its line editor.
    std::thread::sleep(Duration::from_millis(600));
    for line in input {
        session
            .write(line.as_bytes())
            .expect("could not write to the pty");
    }

    let mut collected = Collected {
        text: String::new(),
        signals: Vec::new(),
    };
    let deadline = Instant::now() + TIMEOUT;

    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(PtyEvent::Chunk(chunk)) => {
                collected
                    .text
                    .push_str(&String::from_utf8_lossy(&chunk.bytes));
                collected
                    .signals
                    .extend(chunk.signals.into_iter().map(|s| s.signal));
                if done(&collected.text) {
                    break;
                }
            }
            Ok(PtyEvent::Exited { .. }) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if done(&collected.text) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = session.kill();
    collected
}

/// Strip escape sequences so assertions match what a user would read.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
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
                b']' | b'P' | b'_' | b'^' => {
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => i += 1,
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[test]
fn a_real_shell_receives_input_and_returns_output() {
    // The input path: keystrokes written to the PTY must reach the shell and its
    // output must come back through the pump. A screenshot of a prompt proves
    // only the output half.
    let collected = run("/bin/sh", &[], &["echo tervin-roundtrip-ok\n"], |text| {
        text.contains("tervin-roundtrip-ok")
    });
    let text = plain(&collected.text);
    assert!(
        text.contains("tervin-roundtrip-ok"),
        "shell did not echo the command's output. Got:\n{text}"
    );
}

#[test]
fn output_arrives_in_order_across_several_commands() {
    // The coalescer batches reads; batching must never reorder them.
    let collected = run(
        "/bin/sh",
        &[],
        &["echo one\n", "echo two\n", "echo three\n"],
        |text| text.contains("three"),
    );
    let text = plain(&collected.text);
    let one = text.find("one");
    let two = text.find("two");
    let three = text.find("three");
    assert!(
        one.is_some() && two.is_some() && three.is_some(),
        "not all output arrived. Got:\n{text}"
    );
    assert!(
        one < two && two < three,
        "output arrived out of order:\n{text}"
    );
}

#[test]
fn a_large_burst_of_output_arrives_intact() {
    // The pump flushes early at a size threshold; nothing may be dropped at the
    // boundary. 5000 lines crosses it many times over.
    let collected = run(
        "/bin/sh",
        &[],
        &["i=0; while [ $i -lt 5000 ]; do echo line-$i; i=$((i+1)); done; echo BURST-DONE\n"],
        |text| text.contains("BURST-DONE"),
    );
    let text = plain(&collected.text);
    assert!(text.contains("BURST-DONE"), "burst never completed");
    for probe in ["line-0", "line-2500", "line-4999"] {
        assert!(
            text.contains(probe),
            "{probe} missing from a 5000-line burst"
        );
    }
}

#[test]
fn shell_integration_markers_survive_the_round_trip() {
    // Emitted by the shell, extracted by the tap, delivered on the chunk. If this
    // breaks, Blocks silently stop forming.
    let script = concat!(
        r#"printf '\033]7373;cmd=ZWNobyBoaQ==\007';"#,
        r#"printf '\033]133;C\007';"#,
        "echo hi;",
        r#"printf '\033]133;D;0\007';"#,
        "echo MARKERS-DONE\n",
    );
    let collected = run("/bin/sh", &[], &[script], |text| {
        text.contains("MARKERS-DONE")
    });

    let has_executed = collected
        .signals
        .iter()
        .any(|s| matches!(s, ShellSignal::CommandExecuted));
    let finished_zero = collected
        .signals
        .iter()
        .any(|s| matches!(s, ShellSignal::CommandFinished { exit_code: Some(0) }));
    let command = collected.signals.iter().find_map(|s| match s {
        ShellSignal::Meta { meta } => meta.command.clone(),
        _ => None,
    });

    assert!(
        has_executed,
        "no CommandExecuted signal: {:?}",
        collected.signals
    );
    assert!(
        finished_zero,
        "no CommandFinished(0): {:?}",
        collected.signals
    );
    assert_eq!(
        command.as_deref(),
        Some("echo hi"),
        "the base64 command did not decode"
    );
}

#[test]
fn a_resize_reaches_the_child() {
    // Resizing must deliver SIGWINCH, or every full-screen program reflows wrong.
    let pane = PaneId::new();
    let mut config = PtyConfig::command(
        pane,
        "/bin/sh",
        vec![],
        Some(std::env::temp_dir().display().to_string()),
    );
    config.cols = 80;
    config.rows = 24;

    let (tx, rx) = mpsc::channel::<PtyEvent>();
    let session = PtySession::spawn(
        config,
        Arc::new(move |e| {
            let _ = tx.send(e);
        }),
    )
    .unwrap();

    std::thread::sleep(Duration::from_millis(500));
    session.resize(120, 40).expect("resize failed");
    assert_eq!(session.size(), (120, 40));

    // Ask the shell what size it thinks it is.
    std::thread::sleep(Duration::from_millis(300));
    session
        .write(b"stty size 2>/dev/null || echo no-stty\n")
        .unwrap();

    let mut text = String::new();
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(PtyEvent::Chunk(chunk)) => {
                text.push_str(&String::from_utf8_lossy(&chunk.bytes));
                if text.contains("40 120") || text.contains("no-stty") {
                    break;
                }
            }
            Ok(PtyEvent::Exited { .. }) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
    let _ = session.kill();

    let plain_text = plain(&text);
    if plain_text.contains("no-stty") {
        // No `stty` here; the resize call itself still succeeded above.
        return;
    }
    assert!(
        plain_text.contains("40 120"),
        "the child did not observe the new size. Got:\n{plain_text}"
    );
}

#[test]
fn a_child_that_exits_is_reported_with_its_status() {
    // A pane must not appear to be running after its process is gone.
    let pane = PaneId::new();
    let config = PtyConfig::command(
        pane,
        "/bin/sh",
        vec!["-c".to_string(), "exit 3".to_string()],
        None,
    );

    let (tx, rx) = mpsc::channel::<PtyEvent>();
    let session = PtySession::spawn(
        config,
        Arc::new(move |e| {
            let _ = tx.send(e);
        }),
    )
    .unwrap();

    let deadline = Instant::now() + TIMEOUT;
    let mut exit = None;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(PtyEvent::Exited { exit_code, .. }) => {
                exit = Some(exit_code);
                break;
            }
            Ok(_) => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }

    assert_eq!(exit, Some(Some(3)), "expected an exit status of 3");
    assert!(!session.is_alive(), "session still reports as alive");
}

#[test]
fn tervin_identifies_itself_to_the_child() {
    // Shell hooks key off TERM_PROGRAM, and truecolor must be advertised or
    // prompts fall back to 256 colours.
    let collected = run(
        "/bin/sh",
        &[],
        &["echo \"[$TERM_PROGRAM|$COLORTERM|$TERM]\"\n"],
        |text| text.contains("Tervin|"),
    );
    let text = plain(&collected.text);
    assert!(
        text.contains("Tervin|truecolor|xterm-256color"),
        "got:\n{text}"
    );
}
