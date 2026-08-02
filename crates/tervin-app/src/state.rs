//! Application state and the wiring between subsystems.
//!
//! Everything long-lived hangs off [`AppState`]. Subsystems do not know about
//! each other: the terminal does not know what a Thread is, and the agent runtime
//! does not know what a pane is. This module is the only place they meet, which
//! is what keeps the dependency graph a tree rather than a web.

use agent_runtime::runtime::{AgentSession, ArbiterDecision, PermissionArbiter};
use agent_runtime::{AgentProfile, ProfileConfig, RuntimeRegistry};
use block_engine::{BlockBuilder, Store};
use file_index::FileIndex;
use git_service::GitService;
use parking_lot::{Mutex, RwLock};
use rules_engine::{ActionContext, ActionKind, Decision, RulesEngine};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use terminal_core::TerminalRegistry;
use tervin_core::{PaneId, ThreadId};

/// Per-pane bookkeeping the terminal layer does not own.
pub struct PaneState {
    pub builder: BlockBuilder,
    /// Set when this pane hosts an agent rather than a user shell.
    pub thread_id: Option<ThreadId>,
    pub title: String,
}

/// A live agent Thread.
pub struct ThreadRuntime {
    /// `Arc` rather than `Box` so a caller can take a handle and release the
    /// registry lock before awaiting. Holding a lock across `.await` risks a
    /// deadlock and makes the resulting future `!Send`.
    pub session: Arc<dyn AgentSession>,
    pub profile_id: String,
    pub runtime_id: String,
}

/// The terminal's reported light/dark state, and who wants to hear about changes.
pub struct ColorSchemeState {
    pub scheme: terminal_core::ColorScheme,
    /// Panes that enabled mode 2031. A report sent to a pane that never asked would
    /// appear as stray characters on its command line.
    pub subscribers: std::collections::HashSet<PaneId>,
}

impl Default for ColorSchemeState {
    fn default() -> Self {
        Self {
            // Dark until the UI says otherwise: every Tervin theme shipped dark by
            // default, and guessing light would misreport for one frame at startup.
            scheme: terminal_core::ColorScheme::Dark,
            subscribers: std::collections::HashSet::new(),
        }
    }
}

/// Everything the application owns.
pub struct AppState {
    pub terminals: Arc<TerminalRegistry>,
    pub panes: RwLock<HashMap<PaneId, PaneState>>,
    pub store: Arc<Store>,
    pub git: GitService,
    /// Project file index, backing `@path` completion and file search.
    pub files: FileIndex,
    pub rules: Arc<RulesEngine>,
    pub agents: RwLock<RuntimeRegistry>,
    /// Tervin Rules as a permission arbiter, kept so an adapter registered later
    /// gets the same gate as the ones present at startup.
    arbiter: RwLock<Option<Arc<dyn PermissionArbiter>>>,
    pub profiles: RwLock<ProfileConfig>,
    pub threads: RwLock<HashMap<ThreadId, ThreadRuntime>>,
    /// Directory large Block outputs spill to.
    pub spill_dir: PathBuf,
    /// The project the workspace is currently pointed at.
    pub project_root: Mutex<PathBuf>,
    /// Warnings raised during startup, surfaced once in the UI.
    pub startup_notices: RwLock<Vec<String>>,
    /// Agents the user started themselves in a pane, which Tervin observes but
    /// cannot drive.
    pub pane_agents: crate::pane_agents::PaneAgents,
    /// Whether the active theme's background is dark, and which panes asked to be told
    /// when that changes (DEC mode 2031).
    ///
    /// Held here because the answer comes from the UI's theme while the question arrives
    /// on the PTY pump, and the two never meet otherwise.
    pub color_scheme: Mutex<ColorSchemeState>,
}

impl AppState {
    /// Build application state, opening the local database.
    pub fn new() -> anyhow::Result<Arc<Self>> {
        tervin_core::paths::ensure_dirs()?;

        let store = Arc::new(Store::open(&tervin_core::paths::workspace_db())?);
        let rules = Arc::new(RulesEngine::new());
        for rule in rules_engine::default_rules() {
            rules.add_rule(rule);
        }

        // Retention, applied at startup.
        //
        // Agent transcripts are the part of history people lose: a session ends and the
        // conversation goes with it. Tervin keeps them so "what did I ask about this
        // last week" is answerable — but keeping them forever would grow the database
        // without bound, so old ones are pruned and the window is the user's to set.
        //
        // Blocks are deliberately exempt: a command and its output are small and stay
        // useful for years, while a transcript is large and stops being useful quickly.
        let retention = store
            .kv_get(RETENTION_KEY)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(DEFAULT_RETENTION_DAYS);
        let mut notices = Vec::new();
        if retention > 0 {
            match store.prune_events(retention) {
                Ok(0) => {}
                Ok(removed) => {
                    tracing::info!("pruned {removed} agent events older than {retention} days")
                }
                Err(e) => notices.push(format!(
                    "Could not prune old agent history: {e}. Nothing was lost."
                )),
            }
            // Saved terminal output ages out on the same window. It is bulkier than an
            // event and is raw output, so keeping it longer than the transcripts it sits
            // beside would be the wrong way round.
            if let Err(e) = store.prune_scrollback(retention) {
                tracing::warn!("could not prune saved scrollback: {e}");
            }
        }

        let (profiles, profile_error) = ProfileConfig::load();
        if let Some(err) = profile_error {
            notices.push(err);
        }
        // Write the file on first run so there is something to edit.
        if !ProfileConfig::path().exists() {
            if let Err(e) = profiles.save() {
                notices.push(format!("Could not write the agent profile file: {e}"));
            }
        }

        // Same for MCP: an empty file that explains itself beats a feature nobody
        // discovers because it has no visible surface.
        let mcp_path = agent_runtime::McpConfig::path();
        if !mcp_path.exists() {
            let _ = std::fs::write(&mcp_path, agent_runtime::McpConfig::example());
        }
        if let (_, Some(error)) = agent_runtime::McpConfig::load() {
            notices.push(error);
        }

        let spill_dir = tervin_core::paths::data_dir().join("blocks");
        std::fs::create_dir_all(&spill_dir)?;

        // A previously chosen project wins over any inferred default: reopening
        // Tervin should land where the user left off.
        let project_root = store
            .kv_get(LAST_PROJECT_KEY)
            .ok()
            .flatten()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(default_project_root);

        let state = Arc::new(Self {
            terminals: Arc::new(TerminalRegistry::new()),
            panes: RwLock::new(HashMap::new()),
            store,
            git: GitService::new(),
            files: FileIndex::new(),
            rules: rules.clone(),
            // Populated below, once the arbiter can reference the finished state.
            agents: RwLock::new(RuntimeRegistry::new(None)),
            arbiter: RwLock::new(None),
            profiles: RwLock::new(profiles),
            threads: RwLock::new(HashMap::new()),
            spill_dir,
            project_root: Mutex::new(project_root),
            startup_notices: RwLock::new(notices),
            pane_agents: crate::pane_agents::PaneAgents::new(),
            color_scheme: Mutex::new(ColorSchemeState::default()),
        });

        // Build the file index off the startup path: walking a large project takes
        // long enough to delay the first window paint.
        {
            let files = state.files.clone();
            let root = state.project_root();
            std::thread::spawn(move || {
                let snapshot = files.rebuild(&root);
                tracing::info!(
                    "indexed {} files and {} directories in {}ms{}",
                    snapshot.file_count(),
                    snapshot.dir_count(),
                    snapshot.duration_ms,
                    if snapshot.truncated {
                        " (truncated)"
                    } else {
                        ""
                    }
                );
            });
        }

        // Wire Tervin Rules in as the permission arbiter. This only takes effect
        // for runtimes that actually ask; for the rest the UI reports approvals
        // as provider-native.
        let arbiter: Arc<dyn PermissionArbiter> = Arc::new(TervinArbiter {
            rules,
            store: state.store.clone(),
        });
        *state.agents.write() = RuntimeRegistry::new(Some(arbiter.clone()));
        *state.arbiter.write() = Some(arbiter);

        Ok(state)
    }

    /// How long agent history is kept, in days. Zero disables pruning.
    pub fn retention_days(&self) -> u32 {
        self.store
            .kv_get(RETENTION_KEY)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_RETENTION_DAYS)
    }

    /// The permission arbiter, for adapters registered after startup.
    pub fn arbiter(&self) -> Option<Arc<dyn PermissionArbiter>> {
        self.arbiter.read().clone()
    }

    /// Resolve the agent profile to launch with.
    pub fn profile(&self, id: Option<&str>) -> Option<AgentProfile> {
        let profiles = self.profiles.read();
        match id {
            Some(id) => profiles.get(id).cloned(),
            None => profiles.default_or_first().cloned(),
        }
    }

    pub fn project_root(&self) -> PathBuf {
        self.project_root.lock().clone()
    }

    /// How shell integration reaches a new pane.
    ///
    /// Automatic by default, because a product whose main feature needs manual
    /// setup does not work when it is opened. Persisted so a user who turns it off
    /// stays off.
    pub fn injection_mode(&self) -> shell_integration::InjectionMode {
        match self.store.kv_get(INJECTION_KEY).ok().flatten().as_deref() {
            Some("off") => shell_integration::InjectionMode::Off,
            _ => shell_integration::InjectionMode::Automatic,
        }
    }

    pub fn notice(&self, message: impl Into<String>) {
        self.startup_notices.write().push(message.into());
    }
}

/// Key controlling automatic shell-integration injection.
pub const INJECTION_KEY: &str = "shell_injection";

/// How long agent history is kept, in days.
///
/// A month: long enough to answer "what did I ask about this recently", short enough
/// that a year of transcripts does not accumulate silently. Zero disables pruning.
pub const DEFAULT_RETENTION_DAYS: u32 = 30;

/// Where the retention window is stored.
pub const RETENTION_KEY: &str = "history.retention_days";

/// Key under which the last opened project is remembered.
pub const LAST_PROJECT_KEY: &str = "last_project_root";

/// Where a new workspace points when the user has not chosen a project.
///
/// The process working directory is the obvious answer and the wrong one: a GUI
/// application launched from Finder or the Dock inherits `/`, which is not a
/// project and not somewhere anyone wants a shell. So the cwd is used only when it
/// is plausibly a project, and the home directory is the fallback.
fn default_project_root() -> PathBuf {
    let home = dirs::home_dir();

    if let Ok(cwd) = std::env::current_dir() {
        let is_root = cwd.parent().is_none();
        // A cwd equal to `/` or to the home directory carries no intent; anything
        // else means Tervin was launched from a directory on purpose.
        let is_home = home.as_ref().is_some_and(|h| &cwd == h);
        if !is_root && !is_home {
            return cwd;
        }
    }

    // Prefer a directory that is plausibly full of projects over the home directory
    // itself. Rooting at `~` means indexing the whole account: slow, mostly
    // irrelevant, and on macOS it walks into folders the system guards — which asks
    // the user for access to their music and photos, from a terminal. The file index
    // refuses to descend into those regardless, but starting somewhere sensible is
    // the better half of the fix.
    if let Some(home) = &home {
        for name in [
            "Projects",
            "projects",
            "Code",
            "code",
            "src",
            "dev",
            "Developer",
        ] {
            let candidate = home.join(name);
            if candidate.is_dir() {
                return candidate;
            }
        }
    }

    home.unwrap_or_else(|| PathBuf::from("/"))
}

/// Tervin Rules acting as an agent runtime's permission arbiter.
///
/// Reached only when a runtime asks Tervin before acting. Every decision is
/// written to the audit log, whichever way it goes, so the record shows what was
/// requested as well as what ran.
struct TervinArbiter {
    rules: Arc<RulesEngine>,
    store: Arc<Store>,
}

#[async_trait::async_trait]
impl PermissionArbiter for TervinArbiter {
    async fn decide(
        &self,
        thread_id: &ThreadId,
        tool_name: &str,
        input: &serde_json::Value,
        cwd: &str,
    ) -> ArbiterDecision {
        // Render the action the way the user will see it.
        let action = match tool_name {
            "Bash" => input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            _ => format!(
                "{tool_name} {}",
                agent_runtime::claude::normalize::summarise_tool_input(tool_name, input)
            ),
        };

        let kind = match tool_name {
            "Bash" => ActionKind::Command,
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => ActionKind::FileWrite,
            "WebFetch" | "WebSearch" => ActionKind::NetworkRequest,
            _ => ActionKind::ToolCall,
        };

        let ctx = ActionContext::agent("claude-code", cwd, thread_id.clone(), true);
        let decision = self.rules.evaluate(&action, kind, &ctx);

        let _ = self.store.append_audit(
            Some(thread_id),
            "claude-code",
            &action,
            "requested",
            None,
            Some("tervin"),
            None,
            None,
            None,
        );

        match decision {
            Decision::Allow { reason, .. } => {
                let _ = self.store.append_audit(
                    Some(thread_id),
                    "tervin",
                    &action,
                    "decided",
                    Some("allowed"),
                    Some("tervin"),
                    None,
                    None,
                    Some(&reason),
                );
                ArbiterDecision::Allow
            }
            Decision::Deny { reason, .. } => {
                let _ = self.store.append_audit(
                    Some(thread_id),
                    "tervin",
                    &action,
                    "decided",
                    Some("denied"),
                    Some("tervin"),
                    None,
                    None,
                    Some(&reason),
                );
                ArbiterDecision::Deny { reason }
            }
            // The action needs a human. The request is already queued for the
            // UI; refusing this attempt is the safe answer, and the user can
            // approve and let the agent retry.
            Decision::RequireApproval { request } => {
                let reason = format!(
                    "Held for review by Tervin Rules: {}. Approve it in Tervin and ask the agent to retry.",
                    request.reason
                );
                let _ = self.store.append_audit(
                    Some(thread_id),
                    "tervin",
                    &action,
                    "decided",
                    Some("held"),
                    Some("tervin"),
                    None,
                    serde_json::to_string(&request.risk).ok().as_deref(),
                    Some(&reason),
                );
                ArbiterDecision::Deny { reason }
            }
        }
    }
}
