//! The Tervin desktop host.
//!
//! Assembles the subsystems, owns application state, and exposes the IPC surface
//! the workspace UI calls. Everything domain-specific lives in the other crates;
//! this one is wiring.

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

pub mod agent_blocks;
pub mod commands;
pub mod pane_agents;
pub mod state;

use parking_lot::RwLock;
use shell_integration::ShellAliases;
use std::sync::{Arc, LazyLock};

/// Cached shell aliases.
///
/// Enumerating aliases means starting the user's shell and sourcing their rc
/// files, which can take hundreds of milliseconds. It is done once at startup and
/// on explicit reload, never on the path of anything interactive.
static ALIASES: LazyLock<RwLock<ShellAliases>> =
    LazyLock::new(|| RwLock::new(ShellAliases::default()));

/// The current alias snapshot.
pub fn aliases_snapshot() -> ShellAliases {
    ALIASES.read().clone()
}

/// Re-read aliases from the shell and replace the snapshot.
pub fn reload_aliases() -> ShellAliases {
    let loaded = ShellAliases::load();
    *ALIASES.write() = loaded.clone();
    loaded
}

/// Build and run the desktop application.
pub fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TERVIN_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let state = state::AppState::new()?;

    // Warm the alias cache off the startup path so the first window paint is not
    // waiting on the user's rc files.
    std::thread::spawn(|| {
        reload_aliases();
    });

    // Keep the bundled shell scripts current with this build.
    if let Err(e) = shell_integration::write_all_scripts(&tervin_core::paths::shell_dir()) {
        state.notice(format!("Could not write shell integration scripts: {e}"));
    }

    let builder = tauri::Builder::default();

    // E2E only. The plugin opens a WebDriver HTTP server on TAURI_WEBDRIVER_PORT
    // that can click, type, and read the window, so it is gated on a feature that
    // is off by default rather than on `debug_assertions` — a developer running
    // `tauri dev` gets no automation socket either, only `pnpm e2e:build` does.
    #[cfg(feature = "e2e")]
    let builder = builder.plugin(tauri_plugin_wdio_webdriver::init());

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::clone(&state))
        .invoke_handler(tauri::generate_handler![
            // terminal
            commands::pty_spawn,
            commands::pty_write,
            commands::pty_resize,
            commands::pty_close,
            // blocks
            commands::blocks_query,
            commands::block_get,
            commands::block_output,
            commands::block_set_bookmark,
            commands::block_set_tags,
            commands::block_set_note,
            commands::block_tags_all,
            // git
            commands::git_status,
            commands::git_diff,
            commands::git_branches,
            commands::git_log,
            commands::git_stage,
            commands::git_unstage,
            commands::git_apply_hunks,
            // rules
            commands::rules_list,
            commands::rules_pending,
            commands::rules_evaluate,
            commands::rules_resolve,
            commands::rules_add,
            commands::rules_remove,
            commands::audit_recent,
            // agents
            commands::agents_overview,
            commands::agents_discovery,
            commands::project_instructions,
            commands::agents_add_acp,
            commands::agents_add_local_model,
            commands::agents_save_profiles,
            commands::thread_start,
            commands::thread_send,
            commands::thread_interrupt,
            commands::thread_set_permission_mode,
            commands::thread_info,
            commands::thread_events,
            commands::thread_handoff,
            commands::prompts_search,
            commands::history_retention,
            commands::history_set_retention,
            commands::threads_list,
            commands::thread_stop,
            // environment and settings
            commands::path_complete,
            commands::fs_list_dir,
            commands::path_index_status,
            commands::path_index_rebuild,
            commands::ui_log,
            commands::environment,
            commands::shell_integration_install,
            commands::shell_integration_uninstall,
            commands::aliases_reload,
            commands::alias_expand,
            commands::settings_get,
            commands::settings_set,
            commands::color_scheme_set,
            commands::scrollback_save,
            commands::scrollback_load,
            commands::scrollback_retain,
            commands::workspace_save,
            commands::workspace_load,
            commands::set_project_root,
            commands::open_path,
            // history and workflows
            commands::saved_commands,
            commands::saved_command_upsert,
            commands::saved_command_delete,
            commands::saved_command_render,
            commands::command_history,
            commands::recent_directories,
            commands::forget_directory,
            // connections
            commands::connections,
            commands::connection_launch_spec,
            commands::ssh_probe,
            commands::ssh_key_status,
        ])
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("failed to start Tervin: {e}"))
}
