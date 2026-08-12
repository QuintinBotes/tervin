//! Tervin — the agent-native terminal workspace.

// Windows: keep the console window from appearing behind the app in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// `panic = "abort"` in the release profile means a panic on any thread ends the
// whole window, so a production panic costs the session rather than one feature.
// Each one that remains carries an `#[allow]` whose `reason` is the argument for
// why it cannot fire; a new one has to make that argument or fail the build. What
// this list covers, and the one route it cannot, is written down in tervin-app's
// `tests/production_panics.rs`.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::allow_attributes_without_reason
    )
)]

fn main() {
    // Hook mode, checked before anything else.
    //
    // The same binary doubles as the `PreToolUse` hook that Claude Code runs before
    // each tool call: that way the hook's path is exact and there is no second
    // artefact to install or find on `PATH`. It has to be handled here, ahead of any
    // window or database, because this process is started dozens of times during a
    // session and must do nothing but answer and exit.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(socket) = agent_runtime::claude::hooks::hook_socket_from_args(&args) {
        std::process::exit(agent_runtime::claude::hooks::run_hook_client(&socket));
    }

    if let Err(e) = tervin_app::run() {
        eprintln!("tervin: {e}");
        std::process::exit(1);
    }
}
