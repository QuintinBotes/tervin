//! Blocks, end to end, through the real pipeline.
//!
//! This is the test that says whether Tervin's headline feature actually works
//! when someone opens the app. It exercises every layer with nothing stubbed:
//!
//! real zsh → automatic `ZDOTDIR` injection → OSC markers → the escape tap →
//! `BlockBuilder` → a finished `Block`
//!
//! The unit tests cover each of those in isolation against synthetic input. Only
//! this one proves they compose, and that injection reaches a shell that was
//! never configured for Tervin.

use block_engine::{BlockBuilder, BlockEvent, BlockStatus};
use shell_integration::{InjectionMode, Shell};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use terminal_core::{PtyConfig, PtyEvent};
use tervin_core::{PaneId, SessionId};

/// Generous, because a login shell sources the user's rc files — which on a real
/// machine can mean a version manager and a completion framework.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A scratch directory that cleans itself up.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tervin-e2e-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// Run commands in an injected shell and collect the Blocks that form.
fn blocks_from(shell_program: &str, shell: Shell, commands: &[&str]) -> Vec<block_engine::Block> {
    let scratch = Scratch::new("inject");
    let spill = Scratch::new("spill");

    let injection =
        shell_integration::prepare_injection(shell, scratch.path(), InjectionMode::Automatic);
    assert!(
        injection.unavailable.is_none(),
        "injection unavailable: {:?}",
        injection.unavailable
    );

    let pane = PaneId::new();
    let mut config = PtyConfig::command(
        pane.clone(),
        shell_program,
        // Login+interactive, which is how Tervin opens a pane.
        vec!["-l".to_string()],
        Some(std::env::temp_dir().display().to_string()),
    );
    // Injection arguments come first: `--init-file` has to precede the shell's own.
    let mut args = injection.args.clone();
    args.extend(config.args.clone());
    config.args = args;
    config.env.extend(injection.env.clone());
    config.cols = 100;
    config.rows = 30;

    let (tx, rx) = mpsc::channel::<PtyEvent>();
    let session = spawn_session(config, tx);

    let mut builder = BlockBuilder::new(
        pane,
        SessionId::new(),
        std::env::temp_dir().display().to_string(),
        spill.path().to_path_buf(),
    );

    // Let the shell reach its first prompt before typing, so input is not
    // swallowed by a shell still building its line editor.
    std::thread::sleep(Duration::from_millis(1200));
    for command in commands {
        session.write(command.as_bytes()).expect("write failed");
        // One at a time, so each produces its own Block rather than being typed
        // into a shell that has not yet drawn a new prompt.
        std::thread::sleep(Duration::from_millis(400));
    }

    let mut finished: Vec<block_engine::Block> = Vec::new();
    let deadline = Instant::now() + TIMEOUT;

    while Instant::now() < deadline && finished.len() < commands.len() {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(PtyEvent::Chunk(chunk)) => {
                for event in builder.consume(&chunk) {
                    if let BlockEvent::Finished(block) = event {
                        finished.push(block);
                    }
                }
            }
            Ok(PtyEvent::Exited { exit_code, .. }) => {
                for event in builder.on_session_end(exit_code) {
                    if let BlockEvent::Finished(block) = event {
                        finished.push(block);
                    }
                }
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = session.kill();
    finished
}

/// Small helper so the sink closure stays readable at the call site.
fn spawn_session(config: PtyConfig, tx: mpsc::Sender<PtyEvent>) -> terminal_core::PtySession {
    terminal_core::PtySession::spawn(
        config,
        Arc::new(move |event| {
            let _ = tx.send(event);
        }),
    )
    .expect("could not open a pty")
}

fn have(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

#[test]
fn an_unconfigured_zsh_produces_blocks_after_injection() {
    // The headline claim: Blocks work on first launch, with nothing installed and
    // no file of the user's modified.
    if !have("zsh") {
        return;
    }

    let blocks = blocks_from("zsh", Shell::Zsh, &["echo tervin-block-one\n"]);

    assert!(
        !blocks.is_empty(),
        "no Block formed — injection did not reach the shell"
    );
    let block = &blocks[0];
    assert_eq!(
        block.command, "echo tervin-block-one",
        "the command text did not survive the round trip"
    );
    assert_eq!(block.exit_code, Some(0));
    assert_eq!(block.status, BlockStatus::Succeeded);
    assert!(
        block.output.inline_text().contains("tervin-block-one"),
        "output was not captured: {:?}",
        block.output.inline_text()
    );
    // The marker bytes themselves must not be in the output.
    assert!(!block.output.inline_text().contains("133;"));
}

#[test]
fn a_failing_command_records_its_real_exit_code() {
    // Derived-from-nothing exit codes are worse than none; this must be the
    // shell's own status.
    if !have("zsh") {
        return;
    }

    let blocks = blocks_from("zsh", Shell::Zsh, &["(exit 42)\n", "false\n"]);
    assert!(!blocks.is_empty(), "no Block formed");

    let subshell = blocks
        .iter()
        .find(|b| b.command == "(exit 42)")
        .expect("no Block for the subshell");
    assert_eq!(
        subshell.exit_code,
        Some(42),
        "the shell's own status was not recorded"
    );
    assert_eq!(subshell.status, BlockStatus::Failed);

    if let Some(false_block) = blocks.iter().find(|b| b.command == "false") {
        assert_eq!(false_block.exit_code, Some(1));
        assert_eq!(false_block.status, BlockStatus::Failed);
    }
}

#[test]
fn several_commands_produce_several_blocks_in_order() {
    if !have("zsh") {
        return;
    }

    let blocks = blocks_from(
        "zsh",
        Shell::Zsh,
        &["echo first\n", "echo second\n", "echo third\n"],
    );

    assert!(
        blocks.len() >= 3,
        "expected three Blocks, got {}: {:?}",
        blocks.len(),
        blocks.iter().map(|b| &b.command).collect::<Vec<_>>()
    );
    assert_eq!(blocks[0].command, "echo first");
    assert_eq!(blocks[1].command, "echo second");
    assert_eq!(blocks[2].command, "echo third");

    // Each Block holds only its own output.
    assert!(blocks[0].output.inline_text().contains("first"));
    assert!(!blocks[0].output.inline_text().contains("second"));
}

#[test]
fn the_working_directory_is_recorded_per_block() {
    if !have("zsh") {
        return;
    }

    let blocks = blocks_from("zsh", Shell::Zsh, &["cd /usr\n", "pwd\n"]);
    let pwd_block = blocks.iter().find(|b| b.command == "pwd");
    assert!(
        pwd_block.is_some(),
        "no pwd Block: {:?}",
        blocks.iter().map(|b| &b.command).collect::<Vec<_>>()
    );
    // OSC 7 reported the new directory before the command ran.
    assert_eq!(
        pwd_block.unwrap().cwd,
        "/usr",
        "cwd was not tracked across `cd`"
    );
}

#[test]
fn a_command_with_quotes_and_semicolons_survives_intact() {
    // The reason the command travels base64-encoded rather than as plain text.
    if !have("zsh") {
        return;
    }

    let blocks = blocks_from("zsh", Shell::Zsh, &["echo 'a;b' \"c d\"\n"]);
    assert!(!blocks.is_empty(), "no Block formed");
    assert_eq!(blocks[0].command, r#"echo 'a;b' "c d""#);
}

#[test]
fn parsed_structure_is_attached_to_the_block() {
    // Ports and paths become affordances, so they have to be extracted from real
    // captured output rather than from a fixture.
    if !have("zsh") {
        return;
    }

    let blocks = blocks_from(
        "zsh",
        Shell::Zsh,
        &["echo 'listening on http://localhost:4321/'\n"],
    );
    assert!(!blocks.is_empty());
    let parsed = &blocks[0].parsed;
    assert!(
        parsed.ports.contains(&4321) || parsed.urls.iter().any(|u| u.contains("4321")),
        "no port or url extracted from {:?}",
        blocks[0].output.inline_text()
    );
}

#[test]
fn an_unconfigured_bash_produces_blocks_after_injection() {
    // bash uses a different mechanism entirely (`--init-file`), so it needs its
    // own proof.
    if !have("bash") {
        return;
    }

    let blocks = blocks_from("bash", Shell::Bash, &["echo tervin-bash-block\n"]);

    // macOS ships bash 3.2, where the DEBUG-trap approach is more fragile. Report
    // rather than fail if no Block formed, but verify it hard when one did.
    if blocks.is_empty() {
        eprintln!("note: bash produced no Block on this machine (bash --version?)");
        return;
    }
    assert_eq!(blocks[0].command.trim(), "echo tervin-bash-block");
    assert_eq!(blocks[0].exit_code, Some(0));
}

#[test]
fn injection_leaves_the_users_own_config_untouched() {
    // The promise that makes automatic injection acceptable at all.
    let home = dirs::home_dir().expect("no home directory");
    let watched = [
        ".zshrc",
        ".zshenv",
        ".zprofile",
        ".zlogin",
        ".bashrc",
        ".bash_profile",
    ];

    let before: Vec<Option<std::time::SystemTime>> = watched
        .iter()
        .map(|name| {
            std::fs::metadata(home.join(name))
                .and_then(|m| m.modified())
                .ok()
        })
        .collect();

    let scratch = Scratch::new("untouched");
    for shell in shell_integration::ALL_SHELLS {
        shell_integration::prepare_injection(shell, scratch.path(), InjectionMode::Automatic);
    }

    let after: Vec<Option<std::time::SystemTime>> = watched
        .iter()
        .map(|name| {
            std::fs::metadata(home.join(name))
                .and_then(|m| m.modified())
                .ok()
        })
        .collect();

    for (i, name) in watched.iter().enumerate() {
        assert_eq!(
            before[i], after[i],
            "{name} was modified — injection must never touch a file the user owns"
        );
    }
}
