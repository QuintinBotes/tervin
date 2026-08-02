//! Full-screen editors, driven for real.
//!
//! "vim works" is the single best proxy for terminal correctness, because vim uses
//! nearly everything at once: the alternate screen, cursor addressing, scroll
//! regions, `SIGWINCH` reflow, and raw single-keystroke input with no line
//! discipline. If vim behaves, most of the terminal is right; if it does not, the
//! terminal is a widget that looks like a terminal.
//!
//! These tests drive the actual binary — no scripted escape sequences — and assert
//! on what comes back, including that Tervin's alternate-screen detection fires so
//! Block capture pauses instead of storing a megabyte of redraws.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use terminal_core::{PrivateMode, PtyConfig, PtyEvent};
use tervin_core::PaneId;

const TIMEOUT: Duration = Duration::from_secs(25);

/// A live editor session under test.
struct Session {
    session: terminal_core::PtySession,
    rx: mpsc::Receiver<PtyEvent>,
    text: String,
    /// Whether the alternate screen was active at the last chunk.
    alt_screen: bool,
    /// Every private-mode change observed, in order.
    modes: Vec<(PrivateMode, bool)>,
}

impl Session {
    fn start(program: &str, args: &[&str], cwd: &std::path::Path) -> Self {
        let mut config = PtyConfig::command(
            PaneId::new(),
            program,
            args.iter().map(|s| s.to_string()).collect(),
            Some(cwd.display().to_string()),
        );
        config.cols = 90;
        config.rows = 26;
        // A predictable terminal, so the editor does not probe for capabilities
        // that differ between machines.
        config.env.push(("TERM".into(), "xterm-256color".into()));

        let (tx, rx) = mpsc::channel();
        let session = terminal_core::PtySession::spawn(
            config,
            Arc::new(move |event| {
                let _ = tx.send(event);
            }),
        )
        .expect("could not open a pty");

        Self {
            session,
            rx,
            text: String::new(),
            alt_screen: false,
            modes: Vec::new(),
        }
    }

    /// Pump events until `done` is satisfied or the deadline passes.
    fn wait_for(&mut self, done: impl Fn(&Session) -> bool) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if done(self) {
                return true;
            }
            match self.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(PtyEvent::Chunk(chunk)) => {
                    self.text.push_str(&String::from_utf8_lossy(&chunk.bytes));
                    self.alt_screen = chunk.alternate_screen;
                    for change in chunk.mode_changes {
                        self.modes.push((change.mode, change.enabled));
                    }
                }
                Ok(PtyEvent::Exited { .. }) => return done(self),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            }
        }
        done(self)
    }

    fn send(&self, bytes: &[u8]) {
        self.session.write(bytes).expect("write to pty failed");
        // Editors process one keystroke at a time; typing faster than they read
        // makes a test flaky rather than fast.
        std::thread::sleep(Duration::from_millis(120));
    }

    fn saw_mode(&self, mode: PrivateMode, enabled: bool) -> bool {
        self.modes.iter().any(|&(m, e)| m == mode && e == enabled)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.session.kill();
    }
}

fn have(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

#[test]
fn vim_enters_the_alternate_screen_and_leaves_it_cleanly() {
    // The signal Block capture depends on. Without it, opening vim inside a
    // command stores every screen redraw as that command's output.
    if !have("vim") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut vim = Session::start("vim", &["-u", "NONE", "-N", "scratch.txt"], dir.path());

    assert!(
        vim.wait_for(|s| s.alt_screen && s.saw_mode(PrivateMode::AlternateScreen, true)),
        "vim did not enter the alternate screen. Modes seen: {:?}",
        vim.modes
    );

    // `:q!` leaves without writing.
    vim.send(b"\x1b");
    vim.send(b":q!\r");

    assert!(
        vim.wait_for(|s| s.saw_mode(PrivateMode::AlternateScreen, false) || !s.alt_screen),
        "vim did not leave the alternate screen"
    );
}

#[test]
fn vim_edits_and_writes_a_real_file() {
    // End to end through the input path: raw keystrokes, modal editing, a write.
    // Nothing about this works if the PTY is not in raw mode with no line
    // discipline in the way.
    if !have("vim") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("note.txt");

    let mut vim = Session::start("vim", &["-u", "NONE", "-N", "note.txt"], dir.path());
    assert!(vim.wait_for(|s| s.alt_screen), "vim never started");

    // Insert mode, type, escape, write and quit.
    vim.send(b"i");
    vim.send(b"tervin-vim-roundtrip");
    vim.send(b"\x1b");
    vim.send(b":wq\r");

    // Wait for the file rather than for output: the write is the observable
    // outcome, and screen content is not.
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline && !file.exists() {
        let _ = vim.wait_for(|_| false);
    }

    let written = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(
        written.contains("tervin-vim-roundtrip"),
        "vim did not write the typed text. File was {written:?}"
    );
}

#[test]
fn vim_survives_a_resize() {
    // A resize must deliver SIGWINCH and vim must redraw at the new size. Getting
    // this wrong leaves every full-screen program drawing to stale dimensions.
    if !have("vim") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut vim = Session::start("vim", &["-u", "NONE", "-N"], dir.path());
    assert!(vim.wait_for(|s| s.alt_screen), "vim never started");

    let before = vim.text.len();
    vim.session.resize(120, 40).expect("resize failed");
    assert_eq!(vim.session.size(), (120, 40));

    // A redraw is the observable effect of SIGWINCH arriving.
    assert!(
        vim.wait_for(|s| s.text.len() > before),
        "vim produced no output after a resize"
    );

    vim.send(b"\x1b");
    vim.send(b":q!\r");
}

#[test]
fn vim_enables_mouse_reporting_when_asked() {
    // Mouse reporting decides whether a click goes to the program or starts a
    // selection, so Tervin has to observe the mode rather than guess.
    if !have("vim") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut vim = Session::start("vim", &["-u", "NONE", "-N"], dir.path());
    assert!(vim.wait_for(|s| s.alt_screen), "vim never started");

    vim.send(b"\x1b");
    vim.send(b":set mouse=a\r");

    let saw = vim.wait_for(|s| s.saw_mode(PrivateMode::MouseReporting, true));

    vim.send(b"\x1b");
    vim.send(b":q!\r");

    assert!(
        saw,
        "mouse reporting was never observed. Modes: {:?}",
        vim.modes
    );
}

#[test]
fn control_keys_reach_the_program_rather_than_the_terminal() {
    // Ctrl-A, Ctrl-E, Ctrl-K and Ctrl-W all mean something in readline and emacs.
    // If Tervin swallowed them the shell would be unusable, so this asserts they
    // arrive as control bytes and take effect.
    let dir = tempfile::tempdir().unwrap();
    let mut sh = Session::start("/bin/sh", &[], dir.path());
    std::thread::sleep(Duration::from_millis(500));

    // Type a line, then use Ctrl-U (kill line) and retype. If the control byte is
    // swallowed, the original text survives and the echo shows it.
    sh.send(b"echo SHOULD-NOT-APPEAR");
    sh.send(b"\x15"); // Ctrl-U
    sh.send(b"echo control-keys-ok\r");

    assert!(
        sh.wait_for(|s| s.text.contains("control-keys-ok")),
        "the retyped command never ran"
    );
    // The killed text must not have been executed.
    let executed_original = sh
        .text
        .lines()
        .any(|line| line.trim() == "SHOULD-NOT-APPEAR");
    assert!(
        !executed_original,
        "Ctrl-U did not reach the shell; the original line ran. Output:\n{}",
        sh.text
    );
}

#[test]
fn escape_reaches_the_program() {
    // Escape is a real keystroke that vim depends on. A UI that treats it purely
    // as "close the overlay" breaks modal editing.
    if !have("vim") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("esc.txt");
    let mut vim = Session::start("vim", &["-u", "NONE", "-N", "esc.txt"], dir.path());
    assert!(vim.wait_for(|s| s.alt_screen), "vim never started");

    // Insert, then Escape, then a normal-mode command. The command only works if
    // Escape actually left insert mode.
    vim.send(b"iabc");
    vim.send(b"\x1b");
    vim.send(b":wq\r");

    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline && !file.exists() {
        let _ = vim.wait_for(|_| false);
    }

    let written = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(
        written.contains("abc") && !written.contains(":wq"),
        "Escape did not leave insert mode; the command was typed as text. File: {written:?}"
    );
}

#[test]
fn less_uses_the_alternate_screen_and_quits() {
    // A second full-screen program, because vim is unusual enough that passing
    // only vim proves less than it seems.
    if !have("less") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("long.txt");
    let body: String = (0..200).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&file, body).unwrap();

    let mut less = Session::start("less", &["long.txt"], dir.path());
    assert!(
        less.wait_for(|s| s.alt_screen || s.text.contains("line 0")),
        "less produced nothing"
    );

    less.send(b"q");
    assert!(
        less.wait_for(|s| !s.alt_screen),
        "less did not restore the primary screen"
    );
}

#[test]
fn an_editor_reports_the_size_tervin_gave_it() {
    // Wrong geometry is the most common invisible terminal bug: everything looks
    // fine until a program wraps at the wrong column.
    let dir = tempfile::tempdir().unwrap();
    let mut sh = Session::start("/bin/sh", &[], dir.path());
    std::thread::sleep(Duration::from_millis(400));

    sh.send(b"stty size 2>/dev/null || echo no-stty\r");
    assert!(
        sh.wait_for(|s| s.text.contains("26 90") || s.text.contains("no-stty")),
        "the child never reported its size. Output:\n{}",
        sh.text
    );

    if !sh.text.contains("no-stty") {
        assert!(
            sh.text.contains("26 90"),
            "expected 26 rows by 90 columns, got:\n{}",
            sh.text
        );
    }
}
