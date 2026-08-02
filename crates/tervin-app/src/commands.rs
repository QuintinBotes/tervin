//! The IPC surface between the Rust core and the workspace UI.
//!
//! Two rules shape this module.
//!
//! **Nothing blocking runs on the async runtime.** Git and database calls are
//! blocking, and a `git status` on a cold repository is slow enough to drop
//! frames, so they are moved to a blocking pool. The terminal write path is the
//! exception: it is a lock and a `write`, and pushing it through a task queue
//! would add latency to every keystroke.
//!
//! **Terminal bytes never become JSON.** Output is delivered over an IPC channel
//! as raw binary, arriving in the UI as an `ArrayBuffer`. Encoding a build log as
//! a JSON string array would cost several times the bytes and a parse per frame.

use crate::state::{AppState, PaneState, ThreadRuntime};
use agent_runtime::runtime::{Attachment, LaunchConfig};
use agent_runtime::{AgentProfile, ImportCandidate, ProfileConfig};
use block_engine::{BlockBuilder, BlockEvent, BlockFilter, BlockSummary};
use git_service::{DiffMode, FileDiff, RepoStatus};
use rules_engine::{ActionContext, ActionKind, ApprovalOutcome, ApprovalRequest, Decision};
use serde::{Deserialize, Serialize};
use shell_integration::{IntegrationStatus, Shell, ShellAliases};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, State};
use terminal_core::{PtyConfig, PtyEvent};
use tervin_core::{BlockId, PaneId, SessionId, ThreadId};

/// Errors crossing the IPC boundary, rendered as a plain message for the UI.
#[derive(Debug, Serialize)]
pub struct CommandError {
    pub message: String,
    /// A short, stable code the UI can branch on.
    pub code: String,
}

impl CommandError {
    fn new(code: &str, message: impl std::fmt::Display) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

impl<E: std::fmt::Display> From<E> for CommandError {
    fn from(e: E) -> Self {
        Self::new("error", e)
    }
}

type Result<T> = std::result::Result<T, CommandError>;

/// Run blocking work off the async runtime.
async fn blocking<T, F>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| CommandError::new("panic", e))?
}

// ============================================================ terminal

/// Everything needed to open a pane.
#[derive(Debug, Deserialize)]
pub struct SpawnRequest {
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// Run this instead of the user's shell — the Tier 3 agent path.
    pub program: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Attribute this pane's Blocks to a Thread.
    pub thread_id: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SpawnResponse {
    pub pane_id: String,
    pub shell: Option<Shell>,
    /// Whether the hook will load in this pane, whether by injection or because
    /// the user sourced it themselves.
    pub integration_installed: bool,
    /// Why integration is unavailable, when it is. Surfaced rather than leaving
    /// an empty Blocks list unexplained.
    pub integration_note: Option<String>,
}

/// Open a terminal pane and start streaming its output.
#[tauri::command]
pub async fn pty_spawn(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    request: SpawnRequest,
    on_output: Channel<InvokeResponseBody>,
) -> Result<SpawnResponse> {
    let pane_id = PaneId::new();
    let cwd = request
        .cwd
        .clone()
        .unwrap_or_else(|| state.project_root().display().to_string());

    let mut config = match &request.program {
        Some(program) => PtyConfig::command(
            pane_id.clone(),
            program.clone(),
            request.args.clone(),
            Some(cwd.clone()),
        ),
        None => PtyConfig::login_shell(pane_id.clone(), Some(cwd.clone())),
    };
    config.cols = request.cols.max(1);
    config.rows = request.rows.max(1);
    config.env = request.env.clone();

    // Shell integration, injected rather than requested.
    //
    // Blocks need the shell to report prompt boundaries. Asking the user to edit
    // their rc file first would mean the product does not work when they open it,
    // so Tervin injects the hook itself — via ZDOTDIR for zsh, --init-file for
    // bash, vendor_conf.d for fish — without modifying anything they own.
    let shell = Shell::detect(&config.program);
    let shell_dir = tervin_core::paths::shell_dir();

    let mut integration_active = false;
    let mut integration_note: Option<String> = None;

    if let Some(shell) = shell {
        // A hook the user sourced themselves already works; injecting on top
        // would load it twice.
        let already_sourced = shell_integration::status(shell, &shell_dir).installed;
        if already_sourced {
            integration_active = true;
        } else if request.program.is_none() {
            // Only for a shell Tervin launched as a shell. A managed command or an
            // SSH session is not ours to reconfigure.
            let injection =
                shell_integration::prepare_injection(shell, &shell_dir, state.injection_mode());
            if let Some(reason) = &injection.unavailable {
                integration_note = Some(reason.clone());
            } else if injection.is_active() {
                integration_active = true;
                // Injection args come first: `--init-file` must precede the
                // shell's own arguments.
                let mut args = injection.args;
                args.extend(config.args.clone());
                config.args = args;
                config.env.extend(injection.env);
            }
        }
    }

    // Per-pane Block assembly.
    let mut builder = BlockBuilder::new(
        pane_id.clone(),
        SessionId::new(),
        cwd.clone(),
        state.spill_dir.clone(),
    );
    builder.set_thread(request.thread_id.clone().map(ThreadId::from_external));
    builder.set_project(
        std::path::Path::new(&cwd)
            .file_name()
            .and_then(|s| s.to_str())
            .map(String::from),
    );

    state.panes.write().insert(
        pane_id.clone(),
        PaneState {
            builder,
            thread_id: request.thread_id.clone().map(ThreadId::from_external),
            title: request.title.clone().unwrap_or_else(|| "Shell".to_string()),
        },
    );

    let sink_state = state.inner().clone();
    let sink_app = app.clone();
    let sink = Arc::new(move |event: PtyEvent| match event {
        PtyEvent::Chunk(chunk) => {
            // Bytes to the renderer first: nothing else may delay drawing.
            let _ = on_output.send(InvokeResponseBody::Raw(chunk.bytes.clone()));

            // Colour-scheme handling before the Block engine, because it is not a Block
            // concern: it is the terminal answering a question the program asked.
            answer_color_scheme(&sink_state, &chunk);

            let events = match sink_state.panes.write().get_mut(&chunk.pane_id) {
                Some(pane) => pane.builder.consume(&chunk),
                None => Vec::new(),
            };
            handle_block_events(&sink_state, &sink_app, events);
        }
        PtyEvent::Exited { pane_id, exit_code } => {
            let events = match sink_state.panes.write().get_mut(&pane_id) {
                Some(pane) => pane.builder.on_session_end(exit_code),
                None => Vec::new(),
            };
            handle_block_events(&sink_state, &sink_app, events);
            // The Threads stay on disk — the conversation happened. Only the live
            // session mapping goes, so a new agent in a reused pane id starts clean.
            sink_state.pane_agents.forget_pane(&pane_id);
            let _ = sink_app.emit(
                "pane://exited",
                serde_json::json!({ "paneId": pane_id.as_str(), "exitCode": exit_code }),
            );
        }
    });

    state
        .terminals
        .spawn(config, sink)
        .map_err(|e| CommandError::new("pty_spawn", e))?;

    Ok(SpawnResponse {
        pane_id: pane_id.to_string(),
        shell,
        integration_installed: integration_active,
        integration_note,
    })
}

/// Answer a program's light/dark question, and remember whether it wants updates.
///
/// The reply is written back as *input* to the program, which is how terminal status
/// reports work — so it goes through the same path as a keystroke.
fn answer_color_scheme(state: &Arc<AppState>, chunk: &terminal_core::PtyChunk) {
    let has_query = chunk
        .queries
        .iter()
        .any(|q| matches!(q, terminal_core::TerminalQuery::ColorScheme { .. }));

    let scheme = {
        let mut cs = state.color_scheme.lock();
        // Subscription is carried on every chunk rather than inferred from a change, so
        // a pane that enabled the mode before Tervin started watching is still known.
        if chunk.color_scheme_updates {
            cs.subscribers.insert(chunk.pane_id.clone());
        } else {
            cs.subscribers.remove(&chunk.pane_id);
        }
        cs.scheme
    };

    if has_query {
        if let Err(e) = state.terminals.write(&chunk.pane_id, scheme.report()) {
            tracing::debug!("could not answer a colour-scheme query: {e}");
        }
    }
}

/// Tell every subscribed pane that the theme changed.
///
/// Called from the UI when the theme changes, because the UI owns the themes and the
/// backend only learns the resulting background colour.
#[tauri::command]
pub fn color_scheme_set(state: State<'_, Arc<AppState>>, dark: bool) -> Result<usize> {
    let scheme = if dark {
        terminal_core::ColorScheme::Dark
    } else {
        terminal_core::ColorScheme::Light
    };

    let subscribers = {
        let mut cs = state.color_scheme.lock();
        // Nothing to report when it has not actually changed. Programs redraw on this,
        // so a spurious report is a visible flicker.
        if cs.scheme == scheme {
            return Ok(0);
        }
        cs.scheme = scheme;
        cs.subscribers.iter().cloned().collect::<Vec<_>>()
    };

    let mut told = 0;
    for pane in subscribers {
        // A pane that has since exited simply fails to write; that is not an error worth
        // reporting, and the next chunk would have dropped it anyway.
        if state.terminals.write(&pane, scheme.report()).is_ok() {
            told += 1;
        }
    }
    Ok(told)
}

/// Persist finished Blocks and forward Block activity to the UI.
fn handle_block_events(state: &Arc<AppState>, app: &AppHandle, events: Vec<BlockEvent>) {
    for event in events {
        match event {
            BlockEvent::Started(block) => {
                let _ = app.emit("block://started", &block);
            }
            BlockEvent::Progress {
                block_id,
                total_bytes,
            } => {
                // Progress is high-frequency; the UI throttles its own redraw.
                let _ = app.emit(
                    "block://progress",
                    serde_json::json!({ "blockId": block_id.as_str(), "totalBytes": total_bytes }),
                );
            }
            BlockEvent::Finished(block) => {
                // Writing is blocking; keep it off the PTY pump thread's caller.
                let store = state.store.clone();
                let app = app.clone();
                let to_emit = block.clone();
                std::thread::spawn(move || {
                    if let Err(e) = store.upsert_block(&block) {
                        tracing::warn!("could not persist block {}: {e}", block.id);
                    }
                    let _ = app.emit("block://finished", &to_emit);
                });
            }
            BlockEvent::CwdChanged { pane_id, cwd, host } => {
                // Recorded before it is announced, so a `cd` is in the recent list even
                // if the UI is not listening yet.
                //
                // Only local directories: a path on a remote host does not exist here, and
                // offering to `cd` into it from a local pane would fail.
                if host.is_none() {
                    let store = state.store.clone();
                    let path = cwd.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = store.record_directory(&path) {
                            tracing::debug!("could not record {path}: {e}");
                        }
                    });
                }
                let _ = app.emit(
                    "pane://cwd",
                    serde_json::json!({ "paneId": pane_id.as_str(), "cwd": cwd, "host": host }),
                );
            }
            BlockEvent::AgentActivity { pane_id, activity } => {
                // Reading a transcript is blocking file I/O and this runs on the PTY
                // pump, where a stall shows up as the terminal freezing. Off the
                // thread it goes.
                let state = state.clone();
                let app = app.clone();
                std::thread::spawn(move || {
                    let observation = state.pane_agents.observe(&activity, &pane_id, &state.store);

                    if let Some(thread) = &observation.thread {
                        if let Err(e) = state.store.upsert_thread(thread) {
                            tracing::warn!("could not persist observed thread: {e}");
                        }
                        let _ = app.emit("thread://observed", thread);
                    }
                    for event in &observation.events {
                        if let Err(e) = state.store.append_event(event, None) {
                            tracing::warn!("could not persist observed event: {e}");
                        }
                        let _ = app.emit("thread://event", event);

                        // A session someone ran themselves gets Blocks too. Its commands
                        // come from the transcript, so they arrive as `tool.requested`
                        // rather than `command.started` — the bridge ignores what it does
                        // not recognise, which keeps this a single code path.
                        if let Some(thread_id) = &event.thread_id {
                            let update = state.agent_blocks.observe(thread_id, event);
                            if update.started.is_some() || update.finished.is_some() {
                                crate::agent_blocks::apply(&state.store, &app, update);
                            }
                        }
                    }
                    if let Some((thread_id, thread_state)) = &observation.state {
                        let _ = app.emit(
                            "thread://state",
                            serde_json::json!({
                                "threadId": thread_id.as_str(),
                                "state": thread_state,
                                "label": thread_state.label(),
                            }),
                        );
                    }
                });
            }

            BlockEvent::NotificationRequested {
                pane_id,
                title,
                body,
            } => {
                // Forwarded for the UI to show in its own notice rail. Not raised as a
                // system notification: a process asking for one is not the same as the
                // person wanting one, and OSC 777 can arrive from a remote host.
                let _ = app.emit(
                    "pane://notification",
                    serde_json::json!({
                        "paneId": pane_id.as_str(),
                        "title": title,
                        "body": body,
                    }),
                );
            }

            BlockEvent::ClipboardRequested { selection, bytes } => {
                // Surfaced, never performed here: a remote host must not be able
                // to take the local clipboard without the user seeing it.
                let _ = app.emit(
                    "clipboard://requested",
                    serde_json::json!({
                        "selection": selection,
                        "text": String::from_utf8_lossy(&bytes),
                        "bytes": bytes.len(),
                    }),
                );
            }
        }
    }
}

/// Send user input to a pane.
///
/// Deliberately synchronous: this is on the keystroke path.
#[tauri::command]
pub fn pty_write(state: State<'_, Arc<AppState>>, pane_id: String, data: Vec<u8>) -> Result<()> {
    state
        .terminals
        .write(&PaneId::from_external(pane_id), &data)
        .map_err(|e| CommandError::new("pty_write", e))
}

#[tauri::command]
pub fn pty_resize(
    state: State<'_, Arc<AppState>>,
    pane_id: String,
    cols: u16,
    rows: u16,
) -> Result<()> {
    state
        .terminals
        .resize(&PaneId::from_external(pane_id), cols, rows)
        .map_err(|e| CommandError::new("pty_resize", e))
}

#[tauri::command]
pub fn pty_close(state: State<'_, Arc<AppState>>, pane_id: String) -> Result<()> {
    let id = PaneId::from_external(pane_id);
    state.panes.write().remove(&id);
    state
        .terminals
        .close(&id)
        .map_err(|e| CommandError::new("pty_close", e))
}

// ============================================================ blocks

#[tauri::command]
pub async fn blocks_query(
    state: State<'_, Arc<AppState>>,
    filter: BlockFilter,
) -> Result<Vec<BlockSummary>> {
    let store = state.store.clone();
    blocking(move || store.query_blocks(&filter).map_err(CommandError::from)).await
}

#[tauri::command]
pub async fn block_get(
    state: State<'_, Arc<AppState>>,
    block_id: String,
) -> Result<Option<block_engine::Block>> {
    let store = state.store.clone();
    let id = BlockId::from_external(block_id);
    blocking(move || store.get_block(&id).map_err(CommandError::from)).await
}

/// Full raw output, including anything that spilled to disk.
#[tauri::command]
pub async fn block_output(state: State<'_, Arc<AppState>>, block_id: String) -> Result<Vec<u8>> {
    let store = state.store.clone();
    let id = BlockId::from_external(block_id);
    blocking(move || store.read_full_output(&id).map_err(CommandError::from)).await
}

#[tauri::command]
pub async fn block_set_bookmark(
    state: State<'_, Arc<AppState>>,
    block_id: String,
    bookmarked: bool,
) -> Result<()> {
    let store = state.store.clone();
    let id = BlockId::from_external(block_id);
    blocking(move || {
        store
            .set_bookmark(&id, bookmarked)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn block_set_tags(
    state: State<'_, Arc<AppState>>,
    block_id: String,
    tags: Vec<String>,
) -> Result<()> {
    let store = state.store.clone();
    let id = BlockId::from_external(block_id);
    blocking(move || store.set_tags(&id, &tags).map_err(CommandError::from)).await
}

#[tauri::command]
pub async fn block_set_note(
    state: State<'_, Arc<AppState>>,
    block_id: String,
    note: Option<String>,
) -> Result<()> {
    let store = state.store.clone();
    let id = BlockId::from_external(block_id);
    blocking(move || {
        store
            .set_note(&id, note.as_deref())
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn block_tags_all(state: State<'_, Arc<AppState>>) -> Result<Vec<String>> {
    let store = state.store.clone();
    blocking(move || store.all_tags().map_err(CommandError::from)).await
}

// ============================================================ git

#[tauri::command]
pub async fn git_status(
    state: State<'_, Arc<AppState>>,
    path: Option<String>,
) -> Result<Option<RepoStatus>> {
    let git = state.git.clone();
    let root = path
        .map(PathBuf::from)
        .unwrap_or_else(|| state.project_root());
    blocking(move || {
        let Some(repo) = git.repo_root(&root) else {
            return Ok(None);
        };
        git.status(&repo).map(Some).map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn git_diff(
    state: State<'_, Arc<AppState>>,
    mode: DiffMode,
    path: Option<String>,
) -> Result<Vec<FileDiff>> {
    let git = state.git.clone();
    let root = state.project_root();
    blocking(move || {
        let Some(repo) = git.repo_root(&root) else {
            return Ok(Vec::new());
        };
        match path {
            Some(p) => Ok(git
                .diff_file(&repo, mode, &p)
                .map_err(CommandError::from)?
                .into_iter()
                .collect()),
            None => git.diff(&repo, mode).map_err(CommandError::from),
        }
    })
    .await
}

#[tauri::command]
pub async fn git_branches(state: State<'_, Arc<AppState>>) -> Result<Vec<git_service::Branch>> {
    let git = state.git.clone();
    let root = state.project_root();
    blocking(move || match git.repo_root(&root) {
        Some(repo) => git.branches(&repo).map_err(CommandError::from),
        None => Ok(Vec::new()),
    })
    .await
}

#[tauri::command]
pub async fn git_log(
    state: State<'_, Arc<AppState>>,
    limit: usize,
) -> Result<Vec<git_service::Commit>> {
    let git = state.git.clone();
    let root = state.project_root();
    blocking(move || match git.repo_root(&root) {
        // Commits Tervin did not make are marked so agent and external work is
        // never presented as the user's own.
        Some(repo) => git
            .log(&repo, limit.min(500), Some("Tervin-Session:"))
            .map_err(CommandError::from),
        None => Ok(Vec::new()),
    })
    .await
}

#[tauri::command]
pub async fn git_stage(state: State<'_, Arc<AppState>>, paths: Vec<String>) -> Result<()> {
    let git = state.git.clone();
    let root = state.project_root();
    blocking(move || match git.repo_root(&root) {
        Some(repo) => git.stage(&repo, &paths).map_err(CommandError::from),
        None => Err(CommandError::new("no_repo", "Not a git repository.")),
    })
    .await
}

#[tauri::command]
pub async fn git_unstage(state: State<'_, Arc<AppState>>, paths: Vec<String>) -> Result<()> {
    let git = state.git.clone();
    let root = state.project_root();
    blocking(move || match git.repo_root(&root) {
        Some(repo) => git.unstage(&repo, &paths).map_err(CommandError::from),
        None => Err(CommandError::new("no_repo", "Not a git repository.")),
    })
    .await
}

/// Apply or revert selected hunks.
///
/// Reverting is a destructive edit to the working tree, so it goes through
/// Tervin Rules like any other destructive action.
#[tauri::command]
pub async fn git_apply_hunks(
    state: State<'_, Arc<AppState>>,
    path: String,
    mode: DiffMode,
    hunks: Vec<usize>,
    reverse: bool,
    cached: bool,
) -> Result<()> {
    let git = state.git.clone();
    let root = state.project_root();
    blocking(move || {
        let Some(repo) = git.repo_root(&root) else {
            return Err(CommandError::new("no_repo", "Not a git repository."));
        };
        let diff = git
            .diff_file(&repo, mode, &path)
            .map_err(CommandError::from)?
            .ok_or_else(|| CommandError::new("no_diff", "That file has no diff any more."))?;
        let patch = diff
            .patch_for_hunks(&hunks)
            .ok_or_else(|| CommandError::new("no_hunks", "No applicable hunks were selected."))?;
        git.apply_patch(&repo, &patch, reverse, cached)
            .map_err(CommandError::from)
    })
    .await
}

// ============================================================ rules

#[tauri::command]
pub fn rules_list(state: State<'_, Arc<AppState>>) -> Vec<rules_engine::PolicyRule> {
    state.rules.rules()
}

#[tauri::command]
pub fn rules_pending(state: State<'_, Arc<AppState>>) -> Vec<ApprovalRequest> {
    state.rules.pending_requests()
}

/// What Tervin would do with a command, without running it.
///
/// Aliases are expanded first: the risk of `deploy` depends entirely on what
/// `deploy` is aliased to.
#[derive(Debug, Serialize)]
pub struct EvaluationResult {
    pub decision: Decision,
    /// Set when an alias changed what will actually run.
    pub expansion: Option<shell_integration::Expansion>,
}

#[tauri::command]
pub async fn rules_evaluate(
    state: State<'_, Arc<AppState>>,
    command: String,
    cwd: Option<String>,
) -> Result<EvaluationResult> {
    let cwd = cwd.unwrap_or_else(|| state.project_root().display().to_string());
    let aliases = crate::aliases_snapshot();
    let expansion = aliases.expand_command_line(&command);
    let effective = if expansion.changed() {
        expansion.expanded.clone()
    } else {
        command.clone()
    };

    let ctx = ActionContext::user(cwd);
    let decision = state.rules.evaluate(&effective, ActionKind::Command, &ctx);

    Ok(EvaluationResult {
        decision,
        expansion: expansion.changed().then_some(expansion),
    })
}

#[tauri::command]
pub fn rules_resolve(
    state: State<'_, Arc<AppState>>,
    request_id: String,
    outcome: ApprovalOutcome,
) -> Result<serde_json::Value> {
    let id = tervin_core::RequestId::from_external(request_id);
    let result = state.rules.resolve(&id, outcome);

    let (kind, command) = match &result {
        rules_engine::ResolveResult::Approved {
            request,
            command,
            scope,
        } => {
            let _ = state.store.append_audit(
                request.thread_id.as_ref(),
                "user",
                &request.action,
                "decided",
                Some("approved"),
                Some("tervin"),
                Some(scope.label()),
                serde_json::to_string(&request.risk).ok().as_deref(),
                None,
            );
            ("approved", Some(command.clone()))
        }
        rules_engine::ResolveResult::Denied { request, reason } => {
            let _ = state.store.append_audit(
                request.thread_id.as_ref(),
                "user",
                &request.action,
                "decided",
                Some("denied"),
                Some("tervin"),
                None,
                None,
                Some(reason),
            );
            ("denied", None)
        }
        rules_engine::ResolveResult::ReEvaluate { command, .. } => {
            ("re_evaluate", Some(command.clone()))
        }
        rules_engine::ResolveResult::Unknown => ("unknown", None),
    };

    Ok(serde_json::json!({ "result": kind, "command": command }))
}

#[tauri::command]
pub fn rules_add(state: State<'_, Arc<AppState>>, rule: rules_engine::PolicyRule) {
    state.rules.add_rule(rule);
}

#[tauri::command]
pub fn rules_remove(state: State<'_, Arc<AppState>>, id: String) -> bool {
    state.rules.remove_rule(&id)
}

#[tauri::command]
pub async fn audit_recent(
    state: State<'_, Arc<AppState>>,
    limit: usize,
) -> Result<Vec<block_engine::AuditRecord>> {
    let store = state.store.clone();
    blocking(move || {
        store
            .recent_audit(limit.min(1000))
            .map_err(CommandError::from)
    })
    .await
}

// ============================================================ agents

/// What the user configured, read from disk and nothing else.
///
/// Deliberately separate from [`AgentsDiscovery`]. These two answer different
/// questions — "what did I set up?" and "what is on this machine?" — and only the
/// second one has to leave the process to find out. Serving them together meant a
/// profile the user had written by hand could not be shown until every agent binary
/// on the machine had been probed, and could not be shown *at all* if one of those
/// probes failed: the whole command returned an error, the UI had no profiles, and
/// it said "No agent profile configured" to a user with five of them.
#[derive(Debug, Serialize)]
pub struct AgentsOverview {
    pub profiles: Vec<AgentProfile>,
    pub default_profile: Option<String>,
    /// Where the files the UI mentions actually are.
    ///
    /// Resolved rather than written into the interface, because the location differs by
    /// platform: an interface that names `~/.config/tervin` on macOS is telling the
    /// user to look somewhere that does not exist.
    pub profiles_path: String,
    pub mcp_path: String,
}

/// What Tervin found on the machine. Every field here costs a subprocess.
#[derive(Debug, Serialize)]
pub struct AgentsDiscovery {
    pub discovered: Vec<agent_runtime::Discovery>,
    /// Profiles Tervin found but has not adopted.
    pub import_candidates: Vec<ImportCandidate>,
}

/// The user's own configuration. Cheap, local, and cannot fail on a missing binary.
#[tauri::command]
pub async fn agents_overview(state: State<'_, Arc<AppState>>) -> Result<AgentsOverview> {
    let profiles = state.profiles.read().clone();
    Ok(AgentsOverview {
        default_profile: profiles.default_profile,
        profiles: profiles.profiles,
        profiles_path: tervin_core::paths::abbreviate(&ProfileConfig::path()),
        mcp_path: tervin_core::paths::abbreviate(&agent_runtime::McpConfig::path()),
    })
}

/// What is installed. Slow and failure-prone by nature, which is why it stands alone.
#[tauri::command]
pub async fn agents_discovery(state: State<'_, Arc<AppState>>) -> Result<AgentsDiscovery> {
    // Snapshot under the lock, then release it: discovery spawns processes and
    // must not hold the registry lock while it awaits them.
    let adapters = state.agents.read().snapshot();
    let mut discovered = Vec::new();
    for adapter in adapters {
        discovered.push(adapter.discover().await);
    }
    for agent in agent_runtime::GENERIC_AGENTS {
        discovered.push(agent_runtime::registry::discover_generic(agent).await);
    }

    let existing: Vec<String> = state
        .profiles
        .read()
        .profiles
        .iter()
        .map(|p| p.id.clone())
        .collect();

    let import_candidates = blocking(move || {
        Ok(agent_runtime::profile::import_candidates()
            .into_iter()
            .filter(|c| !existing.contains(&c.profile.id))
            .collect::<Vec<_>>())
    })
    .await?;

    Ok(AgentsDiscovery {
        discovered,
        import_candidates,
    })
}

/// What other tools have already written into this project.
#[derive(Debug, Serialize)]
pub struct ProjectInstructions {
    pub discovered: agent_runtime::instructions::Discovered,
    /// MCP servers found in another tool's configuration that Tervin could adopt.
    ///
    /// Offered, never applied: Tervin supplies MCP servers to ACP agents, so copying
    /// one here genuinely adds tools to an agent, which is the user's call.
    pub adoptable: Vec<agent_runtime::McpAdoption>,
    /// The project this was read from, so the panel can never be ambiguous about
    /// which directory it is describing.
    pub project_root: String,
}

/// Report instruction files and MCP configuration already present in the project.
///
/// Read-only, and reads names rather than contents. Which of these a given runtime
/// will actually obey is decided in [`agent_runtime::instructions::readership`], and
/// the UI asks per runtime rather than presenting one list as universal, because the
/// same `CLAUDE.md` is in force for Claude Code and ignored by Codex.
#[tauri::command]
pub async fn project_instructions(state: State<'_, Arc<AppState>>) -> Result<ProjectInstructions> {
    let root = state.project_root();
    // Tervin's own configured servers, so an offer to adopt can say whether it would
    // overwrite one that is already there.
    let existing: Vec<String> = agent_runtime::McpConfig::load()
        .0
        .enabled()
        .map(|(name, _)| name.clone())
        .collect();

    blocking(move || {
        // A filesystem walk, so off the UI thread. `home_dir` is resolved here rather
        // than inside discovery so the walk itself stays testable against temporary
        // directories.
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let discovered = agent_runtime::instructions::discover(&root, &home);
        let adoptable = agent_runtime::adoption_candidates(&discovered.mcp, &existing);
        Ok(ProjectInstructions {
            discovered,
            adoptable,
            project_root: tervin_core::paths::abbreviate(&root),
        })
    })
    .await
}

/// Register a user-configured agent that speaks the Agent Client Protocol.
///
/// This is the payoff of integrating with a protocol rather than with vendors: an
/// agent Tervin has never heard of becomes a full structured integration — plans,
/// tool events, and a real permission gate — from a command line typed in Settings.
///
/// The profile is saved too, so the agent is selectable immediately rather than
/// registered but invisible.
#[tauri::command]
pub async fn agents_add_acp(
    state: State<'_, Arc<AppState>>,
    display_name: String,
    binary: String,
    args: Vec<String>,
) -> Result<agent_runtime::Discovery> {
    let display_name = display_name.trim().to_string();
    let binary = binary.trim().to_string();
    if display_name.is_empty() || binary.is_empty() {
        return Err(CommandError::new(
            "invalid_agent",
            "An ACP agent needs a name and a command.",
        ));
    }

    // Suffixed so `tier_for` treats it as structured, and so it cannot collide with
    // a generic entry for the same tool.
    let runtime_id = format!("{}-acp", slug(&display_name));
    let spec = agent_runtime::AcpAgentSpec {
        runtime_id: runtime_id.clone(),
        display_name: display_name.clone(),
        binary: binary.clone(),
        args: args.clone(),
        note: "A user-configured agent speaking the Agent Client Protocol. Tervin Rules \
               gate every action it asks about."
            .to_string(),
        install_hint: String::new(),
    };

    {
        let arbiter = state.arbiter();
        state.agents.write().add_acp_agent(spec, arbiter);
    }

    let mut config = state.profiles.read().clone();
    let profile = AgentProfile {
        id: runtime_id.clone(),
        name: display_name,
        runtime_id: runtime_id.clone(),
        binary,
        // The ACP flags live on the profile here because Tervin has no built-in spec
        // for this agent — the user's command line is the whole definition.
        args,
        env: Default::default(),
        model: None,
        permission_mode: None,
        badge: None,
        sensitive: false,
    };
    config.profiles.retain(|p| p.id != profile.id);
    config.profiles.push(profile);
    config
        .save()
        .map_err(|e| CommandError::new("save_profiles", e))?;
    *state.profiles.write() = config;

    let discovery = state
        .agents
        .read()
        .get(&runtime_id)
        .ok_or_else(|| CommandError::new("no_adapter", "The agent was not registered."))?;
    Ok(discovery.discover().await)
}

/// Register a user-configured model endpoint.
///
/// Deliberately does not create an agent profile: a model endpoint is selected as a
/// runtime, and giving it a profile alongside the agents would suggest it can be used
/// interchangeably with them.
#[tauri::command]
pub async fn agents_add_local_model(
    state: State<'_, Arc<AppState>>,
    display_name: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<agent_runtime::Discovery> {
    let display_name = display_name.trim().to_string();
    let base_url = base_url.trim().to_string();
    if display_name.is_empty() || base_url.is_empty() {
        return Err(CommandError::new(
            "invalid_endpoint",
            "A model endpoint needs a name and an address.",
        ));
    }

    let runtime_id = format!("{}-model", slug(&display_name));
    state
        .agents
        .write()
        .add_local_model(runtime_id.clone(), display_name, base_url, api_key);

    let discovery = state
        .agents
        .read()
        .get(&runtime_id)
        .ok_or_else(|| CommandError::new("no_adapter", "The endpoint was not registered."))?;
    Ok(discovery.discover().await)
}

/// A stable, filesystem- and config-safe id from a display name.
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "agent".to_string()
    } else {
        trimmed
    }
}

#[tauri::command]
pub fn agents_save_profiles(
    state: State<'_, Arc<AppState>>,
    profiles: Vec<AgentProfile>,
    default_profile: Option<String>,
) -> Result<String> {
    let config = ProfileConfig {
        default_profile,
        profiles,
    };
    let path = config
        .save()
        .map_err(|e| CommandError::new("save_profiles", e))?;
    *state.profiles.write() = config;
    Ok(path.display().to_string())
}

// ============================================================ threads

#[derive(Debug, Deserialize)]
pub struct ThreadStartRequest {
    pub profile_id: Option<String>,
    pub cwd: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub task_title: Option<String>,
    /// Resume a previous session by its runtime-issued id.
    pub resume_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ThreadStartResponse {
    pub thread_id: String,
    pub profile_id: String,
    pub runtime_id: String,
    pub capabilities: tervin_core::Capabilities,
    pub permissions: agent_runtime::PermissionState,
}

/// Start an agent Thread.
#[tauri::command]
pub async fn thread_start(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    request: ThreadStartRequest,
) -> Result<ThreadStartResponse> {
    let profile = state
        .profile(request.profile_id.as_deref())
        .ok_or_else(|| CommandError::new("no_profile", "No agent profile is configured."))?;

    let runtime = state
        .agents
        .read()
        .get(&profile.runtime_id)
        .ok_or_else(|| {
            CommandError::new(
                "no_adapter",
                format!(
                    "`{}` has no structured adapter. Run it as a managed pane instead.",
                    profile.runtime_id
                ),
            )
        })?;

    let thread_id = ThreadId::new();
    let cwd = request
        .cwd
        .unwrap_or_else(|| state.project_root().display().to_string());

    let mut config = LaunchConfig::new(thread_id.clone(), cwd.clone());
    config.prompt = Some(request.prompt.clone());
    config.attachments = request.attachments;
    config.model = request.model.or_else(|| profile.model.clone());
    config.permission_mode = request
        .permission_mode
        .or_else(|| profile.permission_mode.clone());
    config.task_title = request.task_title.clone();
    // The profile fully determines identity: ambient account variables are
    // cleared, then the profile's own applied.
    config.env = profile.resolved_env();
    // And which executable runs, so two profiles can drive one adapter against
    // different installs.
    config.binary = Some(profile.binary.clone()).filter(|b| !b.trim().is_empty());
    config.extra_args = profile.args.clone();

    let launched = match &request.resume_id {
        Some(id) => runtime.resume(id, config).await,
        None => runtime.launch(config).await,
    }
    .map_err(|e| CommandError::new("launch", e))?;

    // Persist the Thread so it survives a restart.
    let mut thread = tervin_core::thread::Thread::new(
        runtime.identity(),
        cwd.clone(),
        request
            .task_title
            .unwrap_or_else(|| first_line(&request.prompt)),
    );
    thread.id = thread_id.clone();
    thread.state = tervin_core::ThreadState::Starting;
    let _ = state.store.upsert_thread(&thread);

    let capabilities = launched.session.capabilities();
    let permissions = launched.session.permissions();

    // Drain the event stream into the store and on to the UI.
    {
        let store = state.store.clone();
        // The whole state, not just the store: the Block bridge holds the command
        // currently open for this Thread. `State<'_, _>` cannot cross into the task, so
        // the inner `Arc` is cloned out first.
        let app_state = state.inner().clone();
        let app = app.clone();
        let thread_id = thread_id.clone();
        let mut events = launched.events;
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let _ = app.emit("thread://event", &event);

                // A command an agent ran is the same kind of thing as one you ran, so it
                // becomes a Block — searchable and bookmarkable with the rest of your
                // history rather than only a row on this timeline.
                let update = app_state.agent_blocks.observe(&thread_id, &event);
                if update.started.is_some() || update.finished.is_some() {
                    crate::agent_blocks::apply(&store, &app, update);
                }

                if let tervin_core::EventPayload::ThreadState { state } = &event.payload {
                    let _ = app.emit(
                        "thread://state",
                        serde_json::json!({
                            "threadId": thread_id.as_str(),
                            "state": state,
                            "label": state.label(),
                        }),
                    );
                }
                let store = store.clone();
                let event = event.clone();
                // Persisting is blocking; keep it off the event loop.
                let _ = tokio::task::spawn_blocking(move || store.append_event(&event, None)).await;
            }
        });
    }

    state.threads.write().insert(
        thread_id.clone(),
        ThreadRuntime {
            // Box to Arc so handles can be taken without holding the lock.
            session: Arc::from(launched.session),
            profile_id: profile.id.clone(),
            runtime_id: profile.runtime_id.clone(),
        },
    );

    Ok(ThreadStartResponse {
        thread_id: thread_id.to_string(),
        profile_id: profile.id,
        runtime_id: profile.runtime_id,
        capabilities,
        permissions,
    })
}

#[tauri::command]
pub async fn thread_send(
    state: State<'_, Arc<AppState>>,
    thread_id: String,
    content: String,
    attachments: Vec<Attachment>,
) -> Result<()> {
    let session = session_for(&state, &thread_id)?;
    session
        .send_input(content, attachments)
        .await
        .map_err(|e| CommandError::new("send", e))
}

#[tauri::command]
pub async fn thread_interrupt(state: State<'_, Arc<AppState>>, thread_id: String) -> Result<()> {
    let session = session_for(&state, &thread_id)?;
    session
        .interrupt()
        .await
        .map_err(|e| CommandError::new("interrupt", e))
}

#[tauri::command]
pub async fn thread_set_permission_mode(
    state: State<'_, Arc<AppState>>,
    thread_id: String,
    mode: String,
) -> Result<()> {
    let session = session_for(&state, &thread_id)?;
    session
        .set_permission_mode(&mode)
        .await
        .map_err(|e| CommandError::new("permission_mode", e))
}

/// Take a handle to a live session, releasing the registry lock immediately.
fn session_for(state: &AppState, thread_id: &str) -> Result<Arc<dyn agent_runtime::AgentSession>> {
    let id = ThreadId::from_external(thread_id.to_string());
    state
        .threads
        .read()
        .get(&id)
        .map(|t| t.session.clone())
        .ok_or_else(|| CommandError::new("no_thread", "That Thread is not running."))
}

#[derive(Debug, Serialize)]
pub struct ThreadInfo {
    pub thread_id: String,
    pub profile_id: String,
    pub runtime_id: String,
    pub running: bool,
    pub metadata: agent_runtime::SessionMetadata,
    pub permissions: agent_runtime::PermissionState,
    pub capabilities: tervin_core::Capabilities,
    pub diagnostics: Vec<agent_runtime::runtime::RuntimeDiagnostic>,
}

#[tauri::command]
pub fn thread_info(state: State<'_, Arc<AppState>>, thread_id: String) -> Option<ThreadInfo> {
    let id = ThreadId::from_external(thread_id.clone());
    let threads = state.threads.read();
    threads.get(&id).map(|t| ThreadInfo {
        thread_id,
        profile_id: t.profile_id.clone(),
        runtime_id: t.runtime_id.clone(),
        running: t.session.is_running(),
        metadata: t.session.session_metadata(),
        permissions: t.session.permissions(),
        capabilities: t.session.capabilities(),
        diagnostics: t.session.diagnostics(),
    })
}

#[tauri::command]
pub async fn thread_events(
    state: State<'_, Arc<AppState>>,
    thread_id: String,
    limit: usize,
) -> Result<Vec<tervin_core::TervinEvent>> {
    let store = state.store.clone();
    let id = ThreadId::from_external(thread_id);
    blocking(move || {
        store
            .thread_events(&id, limit.min(5000))
            .map_err(CommandError::from)
    })
    .await
}

/// Build a portable handoff from a Thread's recorded work.
///
/// Reads the persisted event stream rather than live state, so a handoff can be taken
/// from a Thread that has already finished — which is when it is usually wanted.
#[tauri::command]
pub async fn thread_handoff(
    state: State<'_, Arc<AppState>>,
    thread_id: String,
) -> Result<HandoffResponse> {
    let store = state.store.clone();
    let id = ThreadId::from_external(thread_id);
    blocking(move || {
        // The whole stream: a partial one would produce a briefing that reads as
        // complete.
        let events = store.thread_events(&id, 50_000)?;
        if events.is_empty() {
            return Err(CommandError::new(
                "no_events",
                "This Thread has no recorded work to hand over.",
            ));
        }
        let bundle = agent_runtime::ContextBundle::from_events(&events);
        Ok(HandoffResponse {
            prompt: bundle.to_prompt(),
            summary: bundle.describe(),
            bundle,
        })
    })
    .await
}

#[derive(Debug, Serialize)]
pub struct HandoffResponse {
    /// Ready to send as the first prompt of a new Thread.
    pub prompt: String,
    /// One line for the UI.
    pub summary: String,
    pub bundle: agent_runtime::ContextBundle,
}

/// Search past prompts and agent replies.
///
/// The gap this fills: a shell keeps command history, and no agent keeps a searchable
/// record of what you asked it. Sessions end and the conversation goes with them.
#[tauri::command]
pub async fn prompts_search(
    state: State<'_, Arc<AppState>>,
    query: String,
    limit: usize,
) -> Result<Vec<block_engine::PromptHit>> {
    let store = state.store.clone();
    blocking(move || {
        store
            .search_prompts(&query, limit.clamp(1, 500))
            .map_err(CommandError::from)
    })
    .await
}

#[derive(Debug, Serialize)]
pub struct RetentionInfo {
    /// Days of agent history kept. Zero means nothing is pruned.
    pub days: u32,
    pub default_days: u32,
}

#[tauri::command]
pub fn history_retention(state: State<'_, Arc<AppState>>) -> Result<RetentionInfo> {
    Ok(RetentionInfo {
        days: state.retention_days(),
        default_days: crate::state::DEFAULT_RETENTION_DAYS,
    })
}

/// Change how long agent history is kept, pruning immediately if it shrank.
///
/// Pruning now rather than at the next launch, because a user who just reduced the
/// window expects the data to be gone — not to be told it will go eventually.
#[tauri::command]
pub async fn history_set_retention(state: State<'_, Arc<AppState>>, days: u32) -> Result<usize> {
    // A year is the ceiling: beyond that the setting is indistinguishable from "keep
    // everything", which `0` already says more clearly.
    let days = days.min(365);
    state
        .store
        .kv_set(crate::state::RETENTION_KEY, &days.to_string())
        .map_err(CommandError::from)?;

    if days == 0 {
        return Ok(0);
    }
    let store = state.store.clone();
    blocking(move || store.prune_events(days).map_err(CommandError::from)).await
}

#[tauri::command]
pub async fn threads_list(
    state: State<'_, Arc<AppState>>,
    limit: usize,
) -> Result<Vec<tervin_core::thread::Thread>> {
    let store = state.store.clone();
    blocking(move || {
        store
            .list_threads(limit.min(500))
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn thread_stop(state: State<'_, Arc<AppState>>, thread_id: String) -> Result<()> {
    let id = ThreadId::from_external(thread_id);
    let runtime = state.threads.write().remove(&id);
    if let Some(runtime) = runtime {
        let _ = runtime.session.shutdown().await;
    }
    state.rules.clear_task_grants(&id);
    Ok(())
}

// ============================================================ environment

#[derive(Debug, Serialize)]
pub struct ShellEnvironment {
    pub shell: Option<Shell>,
    pub integration: Vec<IntegrationStatus>,
    pub aliases: ShellAliases,
    pub project_root: String,
    pub home: Option<String>,
    pub notices: Vec<String>,
}

#[tauri::command]
pub async fn environment(state: State<'_, Arc<AppState>>) -> Result<ShellEnvironment> {
    let dir = tervin_core::paths::shell_dir();
    let project_root = state.project_root().display().to_string();
    let notices = state.startup_notices.read().clone();

    blocking(move || {
        let integration = shell_integration::ALL_SHELLS
            .into_iter()
            .map(|s| shell_integration::status(s, &dir))
            .collect();
        Ok(ShellEnvironment {
            shell: Shell::from_env(),
            integration,
            aliases: crate::aliases_snapshot(),
            project_root,
            home: dirs::home_dir().map(|p| p.display().to_string()),
            notices,
        })
    })
    .await
}

/// What Tervin could learn about reaching one SSH host.
///
/// On demand only, and per host. A `~/.ssh/config` can name a hundred machines across
/// several networks, so probing them all on open would be a port scan of the user's
/// infrastructure — slow, noisy in someone's logs, and quite possibly a conversation with
/// their security team.
#[tauri::command]
pub async fn ssh_probe(alias: String) -> Result<session_manager::Reachability> {
    blocking(move || {
        let config = session_manager::SshConfig::load();
        let Some(host) = config.get(&alias).cloned() else {
            return Ok(session_manager::Reachability::Skipped {
                reason: format!("{alias} is not in your SSH config"),
            });
        };
        Ok(session_manager::probe(&host))
    })
    .await
}

/// Which SSH keys the agent is holding, and what that means per host.
///
/// One call for the whole list rather than one per host: `ssh-add -l` is a single question
/// and asking it once per host would be the same answer many times over.
#[tauri::command]
pub async fn ssh_key_status() -> Result<Vec<(String, session_manager::KeyStatus)>> {
    blocking(move || {
        let agent = session_manager::agent_state();
        let config = session_manager::SshConfig::load();
        Ok(config
            .hosts
            .iter()
            .map(|host| {
                (
                    host.alias.clone(),
                    session_manager::key_status(host, &agent),
                )
            })
            .collect())
    })
    .await
}

/// Everything Tervin can attach a pane to.
///
/// Blocking: it reads `~/.ssh/config`, probes `/dev`, and shells out to tmux and
/// zellij, so it runs on the blocking pool rather than the async runtime.
#[tauri::command]
pub async fn connections() -> Result<session_manager::Connections> {
    blocking(move || Ok(session_manager::Connections::discover())).await
}

/// Resolve a session into the command that opens it, without running it.
///
/// Separate from `pty_spawn` so the UI can show exactly what will run — an SSH
/// host, a tmux attach, a serial device — before anything starts.
#[tauri::command]
pub async fn connection_launch_spec(
    kind: session_manager::SessionKind,
    cwd: Option<String>,
) -> Result<session_manager::LaunchSpec> {
    blocking(move || {
        let shells = session_manager::discover_shells();
        Ok(session_manager::resolve(&kind, &shells, cwd))
    })
    .await
}

#[tauri::command]
pub async fn shell_integration_install(shell: Shell) -> Result<shell_integration::InstallOutcome> {
    let dir = tervin_core::paths::shell_dir();
    blocking(move || shell_integration::install(shell, &dir).map_err(CommandError::from)).await
}

#[tauri::command]
pub async fn shell_integration_uninstall(shell: Shell) -> Result<bool> {
    blocking(move || shell_integration::uninstall(shell).map_err(CommandError::from)).await
}

/// Re-read aliases, for the settings pane's refresh action.
#[tauri::command]
pub async fn aliases_reload() -> Result<ShellAliases> {
    blocking(move || Ok(crate::reload_aliases())).await
}

/// Expand a command the way the shell would, for previews and re-runs.
#[tauri::command]
pub fn alias_expand(command: String) -> shell_integration::Expansion {
    crate::aliases_snapshot().expand_command_line(&command)
}

// ========================================================== path completion

/// Complete a path against the project index.
///
/// `relative_to` scopes results to a subdirectory, which is how a pane's own
/// working directory is honoured — completing in `crates/` should not offer `ui/`
/// paths. Reads an in-memory snapshot, so it is safe to call on every keystroke.
#[tauri::command]
pub async fn path_complete(
    state: State<'_, Arc<AppState>>,
    query: String,
    want: file_index::Want,
    relative_to: Option<String>,
    limit: usize,
) -> Result<Vec<file_index::Completion>> {
    let files = state.files.clone();
    blocking(move || Ok(files.complete(&query, want, relative_to.as_deref(), limit.clamp(1, 200))))
        .await
}

/// A saved command, with its holes already worked out for the UI.
#[derive(Debug, Serialize)]
pub struct SavedCommandView {
    #[serde(flatten)]
    pub command: block_engine::SavedCommand,
    /// Parsed here rather than in the UI, so one parser decides what a hole is. A second
    /// implementation in TypeScript would eventually disagree with this one, and the
    /// disagreement would show up as a corrupted command.
    pub parameters: Vec<block_engine::Parameter>,
}

#[tauri::command]
pub async fn saved_commands(state: State<'_, Arc<AppState>>) -> Result<Vec<SavedCommandView>> {
    let store = state.store.clone();
    blocking(move || {
        Ok(store
            .saved_commands()
            .map_err(CommandError::from)?
            .into_iter()
            .map(|command| SavedCommandView {
                parameters: block_engine::saved::parameters(&command.template),
                command,
            })
            .collect())
    })
    .await
}

/// One command from history, ranked and with its last outcome.
#[derive(Debug, Serialize)]
pub struct CommandSuggestion {
    pub command: String,
    pub uses: u32,
    pub age_hours: u32,
    /// True when the most recent run failed, which is worth seeing before rerunning it.
    pub failed_last_time: bool,
}

/// Commands you have run, ranked for a query.
///
/// What a shell's own `Ctrl-R` cannot do: it searches one shell's history, on one machine,
/// with no idea whether the command worked. This searches everything Tervin has recorded,
/// across panes and projects, and says whether it succeeded last time.
///
/// Same two signals as the directory picker, combined rather than chosen between: the fuzzy
/// score and frecency.
#[tauri::command]
pub async fn command_history(
    state: State<'_, Arc<AppState>>,
    query: String,
    project: Option<String>,
    limit: usize,
) -> Result<Vec<CommandSuggestion>> {
    let store = state.store.clone();
    blocking(move || {
        let hits = store
            .command_history(project.as_deref(), limit.clamp(1, 200))
            .map_err(|e| CommandError::new("command_history", e))?;

        let query = query.trim().to_string();
        let mut matcher = file_index::fuzzy::Matcher::default();
        let mut scored: Vec<(f64, block_engine::CommandHit)> = Vec::new();
        for hit in hits {
            let score = if query.is_empty() {
                hit.frecency()
            } else {
                match matcher.score(&query, &hit.command) {
                    Some(m) => f64::from(m.score) + hit.frecency(),
                    None => continue,
                }
            };
            scored.push((score, hit));
        }
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                // A stable tiebreak, so the list does not reshuffle between keystrokes.
                .then_with(|| a.1.command.cmp(&b.1.command))
        });
        scored.truncate(limit.clamp(1, 200));

        Ok(scored
            .into_iter()
            .map(|(_, hit)| CommandSuggestion {
                failed_last_time: hit.failed_last_time(),
                age_hours: hit.age_hours.min(f64::from(u32::MAX)) as u32,
                uses: hit.uses,
                command: hit.command,
            })
            .collect())
    })
    .await
}

/// A directory offered for `cd`, with why it ranked where it did.
#[derive(Debug, Serialize)]
pub struct DirSuggestion {
    pub path: String,
    /// The last component, for a compact list.
    pub name: String,
    pub visits: u32,
    /// Rounded to whole hours; the UI turns it into "3d".
    pub age_hours: u32,
    /// True when the directory is gone. Shown rather than hidden, with the offer to
    /// forget it — silently dropping it would look like the history had lost something.
    pub missing: bool,
}

/// Directories you have actually been in, ranked for a query.
///
/// Two signals, combined rather than chosen between: how well the path matches what was
/// typed, and frecency. Matching alone puts a directory visited once above the one lived
/// in daily; frecency alone ignores the query. The fuzzy score dominates once something
/// is typed, which is what makes an empty box show "where I usually am" and a typed one
/// show "the thing I mean".
#[tauri::command]
pub async fn recent_directories(
    state: State<'_, Arc<AppState>>,
    query: String,
    limit: usize,
) -> Result<Vec<DirSuggestion>> {
    let store = state.store.clone();
    blocking(move || {
        let mut dirs = store
            .recent_directories()
            .map_err(|e| CommandError::new("recent_dirs", e))?;

        let query = query.trim().to_string();
        let mut scored: Vec<(f64, block_engine::RecentDir)> = Vec::new();
        let mut matcher = file_index::fuzzy::Matcher::default();

        for dir in dirs.drain(..) {
            let score = if query.is_empty() {
                dir.frecency()
            } else {
                // Matched against the whole path, not just the last component: people
                // type `app/src` as readily as `src`.
                match matcher.score(&query, &dir.path) {
                    Some(m) => f64::from(m.score) + dir.frecency(),
                    None => continue,
                }
            };
            scored.push((score, dir));
        }

        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                // A stable tiebreak, so the list does not reshuffle between keystrokes.
                .then_with(|| a.1.path.cmp(&b.1.path))
        });
        scored.truncate(limit.clamp(1, 200));

        Ok(scored
            .into_iter()
            .map(|(_, dir)| DirSuggestion {
                name: std::path::Path::new(&dir.path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&dir.path)
                    .to_string(),
                // Checked at query time rather than on record: a directory is deleted
                // long after it was visited, and a stale flag is worse than none.
                missing: !std::path::Path::new(&dir.path).is_dir(),
                age_hours: dir.age_hours.min(f64::from(u32::MAX)) as u32,
                visits: dir.visits,
                path: dir.path,
            })
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn saved_command_upsert(
    state: State<'_, Arc<AppState>>,
    name: String,
    template: String,
    description: Option<String>,
) -> Result<()> {
    let store = state.store.clone();
    blocking(move || {
        store
            .upsert_saved_command(&block_engine::SavedCommand {
                // Only used for a brand-new command; a save under an existing name keeps
                // the id the row already has.
                id: format!("sc_{}", uuid::Uuid::new_v4().simple()),
                name,
                template,
                description: description.filter(|d| !d.trim().is_empty()),
                uses: 0,
            })
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn saved_command_delete(state: State<'_, Arc<AppState>>, id: String) -> Result<()> {
    let store = state.store.clone();
    blocking(move || store.delete_saved_command(&id).map_err(CommandError::from)).await
}

/// Fill a saved command's holes and note that it was used.
///
/// Rendered in Rust rather than in the UI for the same reason the parameters are: one
/// implementation of what a hole is.
#[tauri::command]
pub async fn saved_command_render(
    state: State<'_, Arc<AppState>>,
    id: String,
    template: String,
    values: Vec<(String, String)>,
) -> Result<String> {
    let store = state.store.clone();
    blocking(move || {
        // Recorded even if the user never sends the line: they reached for it, which is
        // what the ranking is trying to measure.
        let _ = store.record_saved_command_use(&id);
        Ok(block_engine::saved::render(&template, &values))
    })
    .await
}

/// Forget a directory, for one that no longer exists.
#[tauri::command]
pub async fn forget_directory(state: State<'_, Arc<AppState>>, path: String) -> Result<()> {
    let store = state.store.clone();
    blocking(move || store.forget_directory(&path).map_err(CommandError::from)).await
}

/// One entry in a directory listing.
#[derive(Debug, Serialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// True for a dot-file. Reported rather than filtered, so the explorer can offer
    /// to show them without a second round trip.
    pub hidden: bool,
    /// True when this is a symlink, because following one silently is how a file tree
    /// ends up walking in circles.
    pub symlink: bool,
}

/// List one directory, for the file explorer.
///
/// Deliberately one level at a time. A tree that eagerly reads a whole repository
/// spends seconds on `node_modules` before drawing anything, and the file index — which
/// does walk broadly — is for search, not for browsing.
///
/// The listing is bounded: a directory with a hundred thousand entries in it would
/// otherwise stall the UI thread rendering rows nobody will scroll to.
#[tauri::command]
pub async fn fs_list_dir(
    state: State<'_, Arc<AppState>>,
    path: Option<String>,
) -> Result<Vec<DirEntry>> {
    const MAX_ENTRIES: usize = 2_000;

    let root = state.project_root();
    let target = path.map(PathBuf::from).unwrap_or(root);

    blocking(move || {
        let read = std::fs::read_dir(&target)
            .map_err(|e| CommandError::new("read_dir", format!("{}: {e}", target.display())))?;

        let mut entries: Vec<DirEntry> = Vec::new();
        for item in read.flatten() {
            if entries.len() >= MAX_ENTRIES {
                break;
            }
            let name = item.file_name().to_string_lossy().to_string();
            // `file_type` does not follow symlinks, which is what is wanted: a link to
            // a directory should be marked, not silently descended into.
            let file_type = item.file_type().ok();
            let symlink = file_type.is_some_and(|t| t.is_symlink());
            let is_dir = if symlink {
                // Resolve only to decide whether it is expandable.
                item.path().is_dir()
            } else {
                file_type.is_some_and(|t| t.is_dir())
            };
            entries.push(DirEntry {
                hidden: name.starts_with('.'),
                path: item.path().display().to_string(),
                name,
                is_dir,
                symlink,
            });
        }

        // Directories first, then case-insensitive by name — the order every file tree
        // uses, and the one people can scan without reading.
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    })
    .await
}

/// What the index currently holds, for the settings pane.
#[derive(Debug, Serialize)]
pub struct IndexStatus {
    pub root: String,
    pub files: usize,
    pub directories: usize,
    pub truncated: bool,
    pub duration_ms: u64,
}

#[tauri::command]
pub fn path_index_status(state: State<'_, Arc<AppState>>) -> IndexStatus {
    let snapshot = state.files.snapshot();
    IndexStatus {
        root: snapshot.root.display().to_string(),
        files: snapshot.file_count(),
        directories: snapshot.dir_count(),
        truncated: snapshot.truncated,
        duration_ms: snapshot.duration_ms,
    }
}

/// Re-walk the project.
///
/// Explicit rather than watched: a filesystem watcher on a large tree costs more
/// than it saves, and completion staleness is measured in seconds of annoyance
/// rather than correctness.
#[tauri::command]
pub async fn path_index_rebuild(state: State<'_, Arc<AppState>>) -> Result<IndexStatus> {
    let files = state.files.clone();
    let root = state.project_root();
    blocking(move || {
        let snapshot = files.rebuild(&root);
        Ok(IndexStatus {
            root: snapshot.root.display().to_string(),
            files: snapshot.file_count(),
            directories: snapshot.dir_count(),
            truncated: snapshot.truncated,
            duration_ms: snapshot.duration_ms,
        })
    })
    .await
}

// ======================================================== frontend logging

/// Record a message from the UI in Tervin's own log.
///
/// Without this, a frontend failure is invisible: the webview has no console a
/// user can reach, and a crashed React tree renders a blank window with nothing
/// to report. Routing UI errors through here means `TERVIN_LOG=debug tervin`
/// shows what actually went wrong.
#[tauri::command]
pub fn ui_log(level: String, message: String, detail: Option<String>) {
    let detail = detail.unwrap_or_default();
    match level.as_str() {
        "error" => tracing::error!(target: "tervin::ui", "{message}{}{detail}",
            if detail.is_empty() { "" } else { "\n" }),
        "warn" => tracing::warn!(target: "tervin::ui", "{message}"),
        "debug" => tracing::debug!(target: "tervin::ui", "{message}"),
        _ => tracing::info!(target: "tervin::ui", "{message}"),
    }
}

// ============================================================ settings

#[tauri::command]
pub async fn settings_get(state: State<'_, Arc<AppState>>, key: String) -> Result<Option<String>> {
    let store = state.store.clone();
    blocking(move || store.kv_get(&key).map_err(CommandError::from)).await
}

#[tauri::command]
pub async fn settings_set(
    state: State<'_, Arc<AppState>>,
    key: String,
    value: String,
) -> Result<()> {
    let store = state.store.clone();
    blocking(move || store.kv_set(&key, &value).map_err(CommandError::from)).await
}

/// Save a pane's terminal output so it can be restored.
///
/// Keyed by the pane id that the *saved session* records, because pane ids are generated
/// per run — restoring maps the old key onto whichever pane takes its place.
#[tauri::command]
pub async fn scrollback_save(
    state: State<'_, Arc<AppState>>,
    pane_key: String,
    program: Option<String>,
    cwd: Option<String>,
    body: String,
) -> Result<()> {
    let store = state.store.clone();
    blocking(move || {
        store
            .save_scrollback(&pane_key, program.as_deref(), cwd.as_deref(), &body)
            .map_err(CommandError::from)
    })
    .await
}

/// Load a pane's saved output.
///
/// Returns nothing when the pane is now running a different program, so a shell's history
/// cannot be restored into an SSH session.
#[tauri::command]
pub async fn scrollback_load(
    state: State<'_, Arc<AppState>>,
    pane_key: String,
    program: Option<String>,
) -> Result<Option<String>> {
    let store = state.store.clone();
    blocking(move || {
        store
            .load_scrollback(&pane_key, program.as_deref())
            .map_err(CommandError::from)
    })
    .await
}

/// Forget saved output for panes the session no longer contains.
///
/// Called after a session is saved, so closing a pane stops its output being kept. An
/// empty list clears everything, which is what turning session restore off must do.
#[tauri::command]
pub async fn scrollback_retain(
    state: State<'_, Arc<AppState>>,
    pane_keys: Vec<String>,
) -> Result<usize> {
    let store = state.store.clone();
    blocking(move || {
        store
            .retain_scrollback(&pane_keys)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn workspace_save(
    state: State<'_, Arc<AppState>>,
    id: String,
    name: String,
    json: String,
) -> Result<()> {
    let store = state.store.clone();
    blocking(move || {
        store
            .save_workspace(&id, &name, &json)
            .map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub async fn workspace_load(state: State<'_, Arc<AppState>>, id: String) -> Result<Option<String>> {
    let store = state.store.clone();
    blocking(move || store.load_workspace(&id).map_err(CommandError::from)).await
}

#[tauri::command]
pub fn set_project_root(state: State<'_, Arc<AppState>>, path: String) -> Result<String> {
    let path = PathBuf::from(shellexpand_tilde(&path));
    if !path.is_dir() {
        return Err(CommandError::new(
            "not_a_dir",
            "That path is not a directory.",
        ));
    }
    *state.project_root.lock() = path.clone();
    // Remembered so the next launch opens here rather than re-inferring.
    let _ = state
        .store
        .kv_set(crate::state::LAST_PROJECT_KEY, &path.display().to_string());

    // A new project means a new index. Off-thread: the caller is a UI action.
    {
        let files = state.files.clone();
        let root = path.clone();
        std::thread::spawn(move || {
            files.rebuild(&root);
        });
    }

    Ok(path.display().to_string())
}

fn shellexpand_tilde(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|h| h.join(rest).display().to_string())
            .unwrap_or_else(|| path.to_string()),
        None => path.to_string(),
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 80 {
        line.chars().take(80).collect::<String>() + "…"
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_is_stable_safe_and_never_empty() {
        // The slug becomes a persisted profile id and a runtime id, so a name that
        // collapses to nothing must still produce something addressable.
        assert_eq!(slug("My Agent"), "my-agent");
        assert_eq!(slug("Gemini CLI 2.0"), "gemini-cli-2-0");
        assert_eq!(slug("  spaced  out  "), "spaced-out");
        assert_eq!(slug("../../etc/passwd"), "etc-passwd");
        assert_eq!(slug("!!!"), "agent");
        assert_eq!(slug(""), "agent");
        // Same input, same id: re-adding an agent must replace it rather than
        // accumulate duplicates.
        assert_eq!(slug("My Agent"), slug("my   agent"));
    }

    #[test]
    fn a_slug_contains_nothing_that_needs_escaping() {
        for name in ["A/B", "x\"y", "a b\tc", "Ünïcödé nàme", "..", "*"] {
            let s = slug(name);
            assert!(!s.is_empty(), "{name} produced an empty slug");
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{name} produced {s}"
            );
            assert!(
                !s.starts_with('-') && !s.ends_with('-'),
                "{name} produced {s}"
            );
        }
    }
}
