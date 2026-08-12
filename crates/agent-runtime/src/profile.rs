//! Agent profiles: several installs or accounts of the same runtime.
//!
//! It is normal to have more than one of the same agent — a work account and a
//! personal one, a stable release and a nightly, a local model and a hosted one.
//! For Claude Code these are usually distinguished by `CLAUDE_CONFIG_DIR`, and
//! people typically wire them up as shell aliases:
//!
//! ```text
//! alias claude-work='CLAUDE_CONFIG_DIR=~/.claude-work claude'
//! alias claude-personal='CLAUDE_CONFIG_DIR=~/.claude-personal claude'
//! ```
//!
//! **Those aliases cannot work here.** Tervin launches agents as direct child
//! processes rather than through an interactive shell, which is deliberate: going
//! through a shell would mean quoting user input into a command line, and would
//! make what actually ran depend on the user's rc files. An alias is a shell
//! feature and is simply not visible to `execve`.
//!
//! So the same idea is modelled explicitly. A profile names a runtime, a binary,
//! and the environment to launch it with — and [`import_candidates`] can read
//! existing aliases and config directories so an existing setup carries over
//! rather than being retyped.
//!
//! ## Environment isolation
//!
//! A profile's environment is authoritative. Tervin scrubs the variables that
//! select an account before applying the profile's own, because Tervin may itself
//! have been launched from a shell where `CLAUDE_CONFIG_DIR` is already set — and
//! silently inheriting it would run the *work* account under a profile labelled
//! *personal*. Getting that wrong is not a cosmetic bug.
//!
//! ## Secrets
//!
//! The aliases people actually have often carry a key inline:
//! `alias claude-work='ANTHROPIC_API_KEY=sk-… claude'`. Importing one verbatim
//! would copy a live credential into `agents.toml` and print it in Settings, so a
//! secret-shaped name is kept and its value is not: it goes in
//! [`AgentProfile::secrets_from_env`], and [`AgentProfile::resolved_env`] reads the
//! value from Tervin's own environment at launch. A profile that names a secret
//! nobody exported refuses to start rather than failing on authentication later.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Variables that select which account or configuration an agent uses.
///
/// These are cleared before a profile's own environment is applied, so a profile
/// fully determines the identity it runs as.
pub const ACCOUNT_SELECTING_VARS: [&str; 4] = [
    "CLAUDE_CONFIG_DIR",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
];

/// Variables Tervin always clears from a child agent's environment.
///
/// These are set by an enclosing Claude Code session. Leaking them into a child
/// makes the child believe it is a continuation of its parent.
pub const INHERITED_SESSION_VARS: [&str; 6] = [
    "CLAUDECODE",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_PID",
    "CLAUDE_PARENT_SESSION_ID",
];

/// Substrings that make a variable name a credential rather than a setting.
///
/// Matched against the name, never the value: a value test would have to guess at
/// key formats and would miss whichever one is invented next, while a name that says
/// `KEY` is the user saying what it holds. The match is deliberately broad. A false
/// positive costs a value Tervin declines to store and reads from the environment
/// instead — visible, and said out loud before a Thread starts. A false negative
/// writes a live credential into a config file, which is the failure this exists to
/// prevent.
///
/// Every credential in [`ACCOUNT_SELECTING_VARS`] matches one of these today;
/// `every_account_selecting_credential_is_a_secret` fails if a future one does not.
const SECRET_NAME_MARKERS: [&str; 4] = ["KEY", "TOKEN", "SECRET", "PASSWORD"];

/// Whether a variable name is one whose value Tervin refuses to hold.
fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_NAME_MARKERS.iter().any(|m| upper.contains(m))
}

/// One configured agent the user can pick from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfile {
    /// Stable key, used in config, the CLI, and workspace defaults.
    pub id: String,
    /// What the picker shows, e.g. "Claude · Work".
    pub name: String,
    /// Which adapter drives it.
    pub runtime_id: String,
    /// Executable to run. Resolved on `PATH` when not absolute.
    pub binary: String,
    /// Arguments placed before Tervin's own.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment applied after scrubbing. `~` is expanded in values.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Names of variables whose *values* Tervin refuses to hold.
    ///
    /// The value is read from the environment Tervin itself is running in, at
    /// launch, and is never written to `agents.toml`. This exists because the
    /// aliases people already have look like
    /// `alias claude-work='ANTHROPIC_API_KEY=sk-… claude'`, and importing one
    /// verbatim would copy a live credential into a config file and print it in
    /// Settings. A profile that names a secret it cannot find says so before a
    /// Thread starts; see [`AgentProfile::missing_secrets`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets_from_env: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// A short label for the status rail, e.g. "work".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    /// Marks a profile as touching production or a shared account, so the UI can
    /// make that visible before a Thread starts.
    #[serde(default)]
    pub sensitive: bool,
}

impl AgentProfile {
    /// The default profile: whatever `claude` resolves to, with no overrides.
    pub fn default_claude() -> Self {
        Self {
            id: "claude".to_string(),
            name: "Claude Code".to_string(),
            runtime_id: "claude-code".to_string(),
            binary: "claude".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            secrets_from_env: Vec::new(),
            model: None,
            permission_mode: None,
            badge: None,
            sensitive: false,
        }
    }

    /// The environment to launch with: the current environment, scrubbed of
    /// account- and session-selecting variables, then overlaid with the profile's.
    ///
    /// Returned as explicit `(key, value)` pairs, with removals represented as an
    /// empty value so the caller can apply them deterministically.
    ///
    /// Names in `secrets_from_env` are read from Tervin's own environment on the way
    /// through. A name that is not set there is left absent rather than emptied —
    /// [`Self::missing_secrets`] is what reports it.
    ///
    /// # Every launch path this is applied on
    ///
    /// This function only produces a list. Something else has to put it on a child
    /// process, and a launch path that assembles an environment of its own instead is
    /// how the scrub below silently stops happening: the profile still looks right in
    /// Settings, `missing_secrets` still refuses, and an ambient `CLAUDE_CONFIG_DIR`
    /// decides which account runs anyway. So there is one producer and four runtimes,
    /// and all of them are named here.
    ///
    /// **Produced** by `tervin_app::commands::thread_start`, which assigns it to
    /// [`crate::runtime::LaunchConfig`]'s `env`. That is the only place in the app
    /// where a profile becomes a running process, which is why it is also the only
    /// place [`Self::missing_secrets`] has to be consulted.
    ///
    /// **Applied** by [`crate::claude::ClaudeCodeRuntime`], [`crate::codex::CodexRuntime`]
    /// and [`crate::acp::AcpRuntime`], each through [`crate::runtime::apply_env`] —
    /// the only thing that spells an empty value as a removal rather than as an empty
    /// string. Codex clones the list onto its session and re-applies it per turn,
    /// because every `codex exec` is a fresh process.
    ///
    /// **Not applied** by [`crate::local::LocalModelRuntime`], which starts no process
    /// at all: it is an HTTP client, and an endpoint's API key is held in memory for
    /// as long as the app runs and is written nowhere.
    ///
    /// Panes are deliberately not on that list. `terminal_core::PtySession::spawn`
    /// builds a shell's environment itself and no profile reaches it, and an agent
    /// someone starts by hand in a pane is observed rather than launched — see
    /// `tervin_app::pane_agents`, which spawns nothing.
    ///
    /// `every_agent_runtime_is_named_with_how_it_gets_the_environment` holds the
    /// runtime half of that list against the source. The producer is in another crate
    /// and out of its reach; that half is a claim in prose.
    pub fn resolved_env(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();

        // Anything the profile sets wins outright.
        for (k, v) in &self.env {
            out.push((k.clone(), expand_tilde(v)));
        }

        // Named secrets are read from Tervin's own environment here, and the names
        // that produced a value are then held back from the scrub below. Order is
        // load-bearing: the scrub pushes an *empty* value, an empty value is a
        // removal (see `apply_env`), and `ANTHROPIC_API_KEY=""` would unset the very
        // key the user exported rather than pass it through.
        let mut passed_through: Vec<&str> = Vec::new();
        for name in &self.secrets_from_env {
            if self.env.contains_key(name) {
                continue;
            }
            match std::env::var(name) {
                Ok(value) if !value.is_empty() => {
                    out.push((name.clone(), value));
                    passed_through.push(name.as_str());
                }
                // Absent, or present but empty, which the child cannot tell apart
                // from absent. Left to the scrub, and reported by `missing_secrets`
                // before a Thread starts.
                _ => {}
            }
        }

        // Clear identity-bearing variables the profile did not set, so an
        // ambient value cannot decide which account runs.
        for var in ACCOUNT_SELECTING_VARS
            .iter()
            .chain(INHERITED_SESSION_VARS.iter())
        {
            if !self.env.contains_key(*var) && !passed_through.contains(var) {
                out.push((var.to_string(), String::new()));
            }
        }

        out
    }

    /// Secrets this profile names that are not set where Tervin is running.
    ///
    /// A profile carries the name and not the value, so an absent variable means the
    /// agent would launch and then fail on authentication with an error of its own
    /// wording. Saying it here, before anything starts, is the only place the answer
    /// is still specific: this profile, this variable name.
    pub fn missing_secrets(&self) -> Vec<String> {
        self.secrets_from_env
            .iter()
            .filter(|name| !self.env.contains_key(*name))
            .filter(|name| !matches!(std::env::var(name.as_str()), Ok(v) if !v.is_empty()))
            .cloned()
            .collect()
    }

    /// A one-line description of what makes this profile distinct, for the picker.
    pub fn describe(&self) -> String {
        if let Some(dir) = self.env.get("CLAUDE_CONFIG_DIR") {
            return format!("config: {}", abbreviate(&expand_tilde(dir)));
        }
        if self.env.contains_key("ANTHROPIC_API_KEY") {
            return "API key from profile".to_string();
        }
        if let Some(first) = self.secrets_from_env.first() {
            return format!("{first} from your environment");
        }
        if self.binary != "claude" {
            return format!("binary: {}", self.binary);
        }
        "default configuration".to_string()
    }
}

/// The on-disk profile set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// Profile id used when none is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, rename = "profile")]
    pub profiles: Vec<AgentProfile>,
}

impl ProfileConfig {
    /// Path of the profile file. Platform-dependent; see
    /// [`tervin_core::paths::config_dir`].
    pub fn path() -> PathBuf {
        tervin_core::paths::config_dir().join("agents.toml")
    }

    /// Load profiles, falling back to a single default profile.
    ///
    /// A malformed file is reported rather than silently replaced, so a typo
    /// never quietly discards a user's configuration.
    pub fn load() -> (Self, Option<String>) {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (Self::bootstrap(), None);
        };
        match toml::from_str::<Self>(&text) {
            Ok(mut config) => {
                if config.profiles.is_empty() {
                    config.profiles.push(AgentProfile::default_claude());
                }
                (config, None)
            }
            Err(e) => (
                Self::bootstrap(),
                Some(format!("{} could not be parsed: {e}", path.display())),
            ),
        }
    }

    /// A starting configuration, including any profiles discoverable from an
    /// existing setup.
    pub fn bootstrap() -> Self {
        let mut profiles = vec![AgentProfile::default_claude()];
        for candidate in import_candidates() {
            if !profiles.iter().any(|p| p.id == candidate.profile.id) {
                profiles.push(candidate.profile);
            }
        }
        Self {
            default_profile: Some("claude".to_string()),
            profiles,
        }
    }

    pub fn save(&self) -> std::io::Result<PathBuf> {
        self.save_to(&Self::path())
    }

    /// Write the profile set to a given path.
    ///
    /// A parameter rather than always [`Self::path`] so what lands on disk can be
    /// asserted without writing over the developer's own configuration — the same
    /// reason [`crate::instructions::discover`] takes a home directory.
    pub fn save_to(&self, path: &Path) -> std::io::Result<PathBuf> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let header = "# Tervin agent profiles.\n\
                      #\n\
                      # Each profile is one agent install or account. Shell aliases cannot be\n\
                      # used here: Tervin launches agents directly rather than through a shell,\n\
                      # so the environment is set explicitly below.\n\
                      #\n\
                      # `secrets_from_env` names variables read from the environment Tervin was\n\
                      # launched in. Tervin never writes their values here.\n\
                      #\n\
                      # Switch with the command palette, the composer's agent picker, or\n\
                      # `tervin agent --profile <id>`.\n\n";
        // Owner-only: this file says which account each agent runs as, and the user
        // may well add a value to `env` by hand that they would not want readable by
        // every other account on the machine.
        tervin_core::paths::write_private(path, format!("{header}{text}"))?;
        Ok(path.to_path_buf())
    }

    pub fn get(&self, id: &str) -> Option<&AgentProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn default_or_first(&self) -> Option<&AgentProfile> {
        self.default_profile
            .as_deref()
            .and_then(|id| self.get(id))
            .or_else(|| self.profiles.first())
    }
}

/// A profile Tervin found on the machine but has not adopted.
///
/// Never enabled automatically: adopting a profile decides which account an agent
/// runs as, and that is the user's call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportCandidate {
    pub profile: AgentProfile,
    /// Where this was found, shown verbatim before the user accepts it.
    pub source: String,
}

/// Discover plausible profiles from the existing setup.
///
/// Two sources, both read-only:
///
/// 1. **Shell aliases.** The conventional way to run multiple accounts. The
///    user's interactive shell is asked to list its aliases, and any that invoke a
///    known agent binary with environment overrides are offered.
/// 2. **Config directories.** `~/.claude-*` siblings, which is what
///    `CLAUDE_CONFIG_DIR` points at.
/// 3. **Installed ACP agents.** Any agent Tervin can drive over the Agent Client
///    Protocol whose binary is on `PATH`, because a structured integration that
///    the user has to hand-configure is one most people will never find.
pub fn import_candidates() -> Vec<ImportCandidate> {
    let mut out: Vec<ImportCandidate> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for candidate in candidates_from_aliases()
        .into_iter()
        .chain(candidates_from_config_dirs())
        .chain(candidates_from_acp_agents())
        .chain(candidates_from_codex())
    {
        if seen.contains(&candidate.profile.id) {
            continue;
        }
        seen.push(candidate.profile.id.clone());
        out.push(candidate);
    }
    out
}

/// Offer a profile for every installed ACP agent.
/// Offer Codex when it is installed.
///
/// Offered rather than added: the source line says what Tervin can and cannot do with it,
/// and a user who sees "cannot gate" before accepting is making an informed choice. A
/// profile that appeared on its own would not give them that.
fn candidates_from_codex() -> Vec<ImportCandidate> {
    let binary = crate::codex::runtime::DEFAULT_BINARY;
    let Some(path) = crate::which(binary) else {
        return Vec::new();
    };
    vec![ImportCandidate {
        profile: AgentProfile {
            id: "codex".to_string(),
            name: "Codex".to_string(),
            runtime_id: "codex".to_string(),
            binary: binary.to_string(),
            // `exec --json` belongs to the adapter, not the profile: a user who removed
            // it from here would get a session that produces no events at all.
            args: Vec::new(),
            env: BTreeMap::new(),
            secrets_from_env: Vec::new(),
            model: None,
            permission_mode: None,
            badge: None,
            sensitive: false,
        },
        source: format!(
            "{path} (structured JSONL; Tervin reads it but cannot gate it — Codex's own sandbox decides)"
        ),
    }]
}

fn candidates_from_acp_agents() -> Vec<ImportCandidate> {
    crate::acp::known_acp_agents()
        .into_iter()
        .filter_map(|spec| {
            let path = crate::which(&spec.binary)?;
            Some(ImportCandidate {
                profile: AgentProfile {
                    id: spec.runtime_id.clone(),
                    name: spec.display_name.clone(),
                    runtime_id: spec.runtime_id,
                    binary: spec.binary,
                    // The ACP flag belongs to the adapter, not the profile: putting
                    // it here would let a user remove it and get a silent hang.
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    secrets_from_env: Vec::new(),
                    model: None,
                    permission_mode: None,
                    badge: None,
                    sensitive: false,
                },
                source: format!("{path} (speaks the Agent Client Protocol)"),
            })
        })
        .collect()
}

/// How long the user's shell gets to list its aliases before Tervin stops caring.
const ALIAS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run a command, collect its standard output, and give up after `limit`.
///
/// `std::process::Command` offers no timeout, and the one thing worse than a
/// discovery that finds nothing is a discovery that never returns. Returns `None`
/// if the command could not start, ran out of time, or was killed — all of which
/// mean the same thing to every caller here: no answer.
fn run_briefly(command: &mut std::process::Command, limit: std::time::Duration) -> Option<Vec<u8>> {
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // Drain the pipe on another thread. A child that fills the pipe buffer blocks
    // until someone reads it, so waiting for exit without draining would be waiting
    // for a child that is itself waiting for us.
    let mut pipe = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
        let _ = tx.send(buf);
    });

    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            // Out of time, or a child that cannot be waited on at all. Kill it either
            // way: whatever the rc files are still doing, nobody is waiting for it now.
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }

    rx.recv_timeout(limit).ok()
}

/// Ask the user's shell for its aliases and parse agent-launching ones.
///
/// Runs `$SHELL -ic alias`, which sources the user's rc files. That is the only
/// way to see aliases, and it is why the results are *offered* rather than
/// adopted: the command's output is treated as untrusted input and only ever
/// parsed, never executed.
fn candidates_from_aliases() -> Vec<ImportCandidate> {
    let shell = match std::env::var("SHELL") {
        Ok(s) if !s.is_empty() => s,
        _ => return Vec::new(),
    };

    // An interactive shell runs whatever the user's rc files contain, and some of
    // that waits: version managers that hit the network, prompt frameworks, a stray
    // `read`. Aliases are a convenience, so a shell that will not answer promptly is
    // simply one that offers nothing — never a reason to keep the caller waiting.
    let Some(stdout) = run_briefly(
        std::process::Command::new(&shell)
            .args(["-ic", "alias"])
            .stdin(std::process::Stdio::null()),
        ALIAS_TIMEOUT,
    ) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&stdout);

    text.lines().filter_map(candidate_from_alias_line).collect()
}

/// One `alias` line as an offer, including what Tervin will not carry over.
fn candidate_from_alias_line(line: &str) -> Option<ImportCandidate> {
    let (name, profile) = parse_alias_line(line)?;
    // A dropped value is said here rather than discovered at launch. An alias is
    // often the only place a key is written down, so an offer that stayed quiet
    // about it would look complete and then fail on authentication.
    let source = match profile.secrets_from_env.join(", ") {
        s if s.is_empty() => format!("shell alias `{name}`"),
        s => format!(
            "shell alias `{name}` ({s} read from your environment at launch; \
             Tervin does not copy the value out of the alias)"
        ),
    };
    Some(ImportCandidate { profile, source })
}

/// Parse one `alias` line into a profile, if it launches a known agent.
///
/// Handles both `name='body'` and `alias name='body'`, with either quote style.
fn parse_alias_line(line: &str) -> Option<(String, AgentProfile)> {
    let line = line.trim().strip_prefix("alias ").unwrap_or(line.trim());
    let (name, body) = line.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }

    let body = body.trim();
    let body = body
        .strip_prefix('\'')
        .and_then(|b| b.strip_suffix('\''))
        .or_else(|| body.strip_prefix('"').and_then(|b| b.strip_suffix('"')))
        .unwrap_or(body);

    let tokens = shell_words_split(body)?;

    // Leading VAR=value assignments, then the program. A name that looks like a
    // credential keeps its name and loses its value: the value goes no further than
    // this function, and the profile records where to read it from instead.
    let mut env = BTreeMap::new();
    let mut secrets_from_env = Vec::new();
    let mut rest = tokens.into_iter().peekable();
    while let Some(tok) = rest.peek() {
        match tok.split_once('=') {
            Some((k, v))
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                if is_secret_name(k) {
                    secrets_from_env.push(k.to_string());
                } else {
                    env.insert(k.to_string(), v.to_string());
                }
                rest.next();
            }
            _ => break,
        }
    }

    let binary = rest.next()?;
    let program = binary.rsplit('/').next().unwrap_or(&binary).to_string();
    let runtime_id = runtime_for_program(&program)?;

    // An alias with no overrides adds nothing over the default profile.
    if env.is_empty() && secrets_from_env.is_empty() {
        return None;
    }

    let args: Vec<String> = rest.collect();
    let badge = name
        .rsplit('-')
        .next()
        .filter(|s| *s != name)
        .map(|s| s.to_string());

    Some((
        name.to_string(),
        AgentProfile {
            id: name.to_string(),
            name: pretty_name(name),
            runtime_id,
            binary,
            args,
            env,
            secrets_from_env,
            model: None,
            permission_mode: None,
            badge,
            sensitive: name.contains("work") || name.contains("prod"),
        },
    ))
}

/// Offer a profile per `~/.claude-*` configuration directory.
fn candidates_from_config_dirs() -> Vec<ImportCandidate> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&home) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        let Some(suffix) = name.strip_prefix(".claude-") else {
            continue;
        };
        // Only directories that look like a real config home.
        if !looks_like_claude_config(&entry.path()) {
            continue;
        }

        let id = format!("claude-{suffix}");
        let mut env = BTreeMap::new();
        env.insert(
            "CLAUDE_CONFIG_DIR".to_string(),
            entry.path().display().to_string(),
        );

        out.push(ImportCandidate {
            source: format!(
                "config directory {}",
                abbreviate(&entry.path().display().to_string())
            ),
            profile: AgentProfile {
                id: id.clone(),
                name: pretty_name(&id),
                runtime_id: "claude-code".to_string(),
                binary: "claude".to_string(),
                args: Vec::new(),
                env,
                secrets_from_env: Vec::new(),
                model: None,
                permission_mode: None,
                badge: Some(suffix.to_string()),
                sensitive: suffix.contains("work") || suffix.contains("prod"),
            },
        });
    }
    out.sort_by(|a, b| a.profile.id.cmp(&b.profile.id));
    out
}

/// Whether a directory is a Claude Code config home.
///
/// The test is for a *session store*, not merely a settings file. Plugins and
/// adjacent tools write their own `~/.claude-*` directories and often drop a
/// `settings.json` in them; treating that as an account produced profiles that
/// pointed at a plugin's database. A session store — conversation history, the
/// per-project directory, or stored credentials — only exists where Claude Code
/// has actually run.
fn looks_like_claude_config(path: &Path) -> bool {
    const SESSION_MARKERS: [&str; 3] = ["history.jsonl", "projects", ".credentials.json"];
    SESSION_MARKERS.iter().any(|m| path.join(m).exists())
}

/// Map a program name to the adapter that drives it.
fn runtime_for_program(program: &str) -> Option<String> {
    match program {
        "claude" => Some("claude-code".to_string()),
        "codex" => Some("codex".to_string()),
        "gemini" => Some("gemini".to_string()),
        "aider" => Some("aider".to_string()),
        "opencode" => Some("opencode".to_string()),
        _ => None,
    }
}

/// `claude-work` becomes `Claude · Work`.
fn pretty_name(id: &str) -> String {
    let mut parts = id.splitn(2, '-');
    let base = parts.next().unwrap_or(id);
    let base = match base {
        "claude" => "Claude".to_string(),
        other => capitalise(other),
    };
    match parts.next() {
        Some(rest) if !rest.is_empty() => format!("{base} · {}", capitalise(rest)),
        _ => base,
    }
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn expand_tilde(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    if value == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.display().to_string();
        }
    }
    value.to_string()
}

fn abbreviate(path: &str) -> String {
    match dirs::home_dir() {
        Some(home) => {
            let home = home.display().to_string();
            match path.strip_prefix(&home) {
                Some(rest) => format!("~{rest}"),
                None => path.to_string(),
            }
        }
        None => path.to_string(),
    }
}

/// Minimal POSIX-ish tokeniser, sufficient for alias bodies.
fn shell_words_split(input: &str) -> Option<Vec<String>> {
    shell_words::split(input).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_that_answers_is_read_in_full() {
        let out = run_briefly(
            std::process::Command::new("sh").args(["-c", "echo alias-one; echo alias-two"]),
            std::time::Duration::from_secs(10),
        )
        .expect("a prompt command should produce output");
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("alias-one") && text.contains("alias-two"),
            "{text}"
        );
    }

    #[test]
    fn a_command_that_hangs_is_abandoned_rather_than_waited_on() {
        // The bug this exists to prevent: `$SHELL -ic alias` sources rc files, and an
        // rc file that blocks used to block the whole agents view behind it.
        let started = std::time::Instant::now();
        let out = run_briefly(
            std::process::Command::new("sh").args(["-c", "sleep 30"]),
            std::time::Duration::from_millis(300),
        );
        assert!(out.is_none(), "a hung command has no answer to give");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "gave up after {:?}, which is not giving up",
            started.elapsed()
        );
    }

    #[test]
    fn a_command_that_does_not_exist_is_not_an_answer() {
        assert!(run_briefly(
            &mut std::process::Command::new("tervin-no-such-binary-anywhere"),
            std::time::Duration::from_secs(5),
        )
        .is_none());
    }

    #[test]
    fn parses_a_config_dir_alias() {
        // The exact shape people actually use for multiple accounts.
        let (name, profile) =
            parse_alias_line("claude-work='CLAUDE_CONFIG_DIR=~/.claude-work claude'").unwrap();
        assert_eq!(name, "claude-work");
        assert_eq!(profile.runtime_id, "claude-code");
        assert_eq!(profile.binary, "claude");
        assert_eq!(
            profile.env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
            Some("~/.claude-work")
        );
        assert_eq!(profile.name, "Claude · Work");
        assert_eq!(profile.badge.as_deref(), Some("work"));
    }

    #[test]
    fn parses_an_alias_with_the_alias_keyword_and_double_quotes() {
        let (_, profile) = parse_alias_line(
            "alias claude-personal=\"CLAUDE_CONFIG_DIR=~/.claude-personal claude\"",
        )
        .unwrap();
        assert_eq!(profile.id, "claude-personal");
        assert!(!profile.sensitive);
    }

    #[test]
    fn ignores_aliases_that_are_not_agents() {
        assert!(parse_alias_line("ll='ls -la'").is_none());
        assert!(parse_alias_line("g='git'").is_none());
        assert!(parse_alias_line("claude-mem='/path/to/bun script.cjs'").is_none());
    }

    #[test]
    fn ignores_an_alias_that_adds_nothing() {
        // `alias c='claude'` is not a distinct profile.
        assert!(parse_alias_line("c='claude'").is_none());
    }

    #[test]
    fn a_work_profile_is_marked_sensitive() {
        let (_, profile) =
            parse_alias_line("claude-work='CLAUDE_CONFIG_DIR=~/.claude-work claude'").unwrap();
        assert!(
            profile.sensitive,
            "a work account should be visibly distinct before a Thread starts"
        );
    }

    /// A directory of this run's own, so a test never writes over a real config.
    fn scratch_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tervin-profile-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("could not create a scratch directory");
        dir
    }

    #[test]
    fn an_adopted_alias_never_writes_its_api_key_to_disk() {
        // The whole slice in one line of shell. `agents.toml` is a plain file in the
        // user's config directory, and a copied key would sit in it indefinitely.
        let (_, profile) = parse_alias_line(
            "alias claude-work='ANTHROPIC_API_KEY=sk-do-not-store CLAUDE_CONFIG_DIR=~/.claude-work claude'",
        )
        .expect("an alias that launches claude with overrides is a profile");

        let config = ProfileConfig {
            default_profile: Some("claude-work".to_string()),
            profiles: vec![profile],
        };
        let dir = scratch_dir();
        let path = config
            .save_to(&dir.join("agents.toml"))
            .expect("saving the profile set failed");
        let text = std::fs::read_to_string(&path).expect("the file should be readable");

        assert!(
            text.contains("CLAUDE_CONFIG_DIR") && text.contains(".claude-work"),
            "the setting that distinguishes the account must survive:\n{text}"
        );
        assert!(
            text.contains("ANTHROPIC_API_KEY"),
            "the name is kept, so the profile can say where the value comes from:\n{text}"
        );
        assert!(
            !text.contains("sk-do-not-store"),
            "the value must not reach disk:\n{text}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_alias_whose_only_override_is_a_key_is_still_a_profile() {
        // Dropping the value must not drop the profile: this alias is how a second
        // account is spelled when the key is the only thing that differs.
        let (_, profile) = parse_alias_line("claude-alt='ANTHROPIC_API_KEY=sk-alt claude'")
            .expect("an alias that only sets a key is still a distinct profile");
        assert!(profile.env.is_empty(), "no value should have been kept");
        assert_eq!(profile.secrets_from_env, vec!["ANTHROPIC_API_KEY"]);
    }

    #[test]
    fn an_offered_alias_says_which_value_it_did_not_copy() {
        // An offer that stayed quiet would look complete and then fail to authenticate.
        let candidate = candidate_from_alias_line(
            "claude-work='ANTHROPIC_API_KEY=sk-do-not-store CLAUDE_CONFIG_DIR=~/.claude-work claude'",
        )
        .expect("the alias should be offered");
        assert!(
            candidate.source.contains("ANTHROPIC_API_KEY")
                && candidate.source.contains("your environment"),
            "the source line must say where the value will come from: {}",
            candidate.source
        );
        assert!(
            !candidate.source.contains("sk-do-not-store"),
            "and must not print the value it declined to keep: {}",
            candidate.source
        );

        let plain =
            candidate_from_alias_line("claude-work='CLAUDE_CONFIG_DIR=~/.claude-work claude'")
                .expect("the alias should be offered");
        assert_eq!(
            plain.source, "shell alias `claude-work`",
            "an alias with nothing to withhold says nothing extra"
        );
    }

    #[test]
    fn every_account_selecting_credential_is_a_secret() {
        // CLAUDE_CONFIG_DIR is a path and belongs in `env` — it is the whole reason
        // profiles exist. Every other account-selecting variable authenticates, so a
        // new one added to that list must be caught by the name markers.
        for var in ACCOUNT_SELECTING_VARS {
            let expected = var != "CLAUDE_CONFIG_DIR";
            assert_eq!(
                is_secret_name(var),
                expected,
                "{var} is classified wrongly; add a marker to SECRET_NAME_MARKERS"
            );
        }
        assert!(!is_secret_name("CLAUDE_CONFIG_DIR"));
        assert!(is_secret_name("OPENAI_API_KEY"));
        assert!(is_secret_name("GITHUB_TOKEN"));
        assert!(is_secret_name("AWS_SECRET_ACCESS_KEY"));
        assert!(is_secret_name("DB_PASSWORD"));
    }

    #[test]
    fn a_secret_named_in_a_profile_is_read_from_the_environment_not_from_disk() {
        // Set for real rather than injected: the value has to come from the process
        // environment Tervin was launched with, which is the thing under test.
        std::env::set_var("ANTHROPIC_API_KEY", "sk-live-from-the-environment");

        let mut profile = AgentProfile::default_claude();
        profile.secrets_from_env = vec!["ANTHROPIC_API_KEY".to_string()];
        let env = profile.resolved_env();

        let entries: Vec<&(String, String)> = env
            .iter()
            .filter(|(k, _)| k == "ANTHROPIC_API_KEY")
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "exactly one entry, so application order cannot decide the outcome: {env:?}"
        );
        assert_eq!(
            entries[0].1, "sk-live-from-the-environment",
            "the live value must be passed through"
        );
        // The trap this ordering exists to avoid: an empty value is a removal, so a
        // scrub entry here would unset the key the user exported.
        assert!(
            !env.iter()
                .any(|(k, v)| k == "ANTHROPIC_API_KEY" && v.is_empty()),
            "no removal may be emitted for a passed-through secret: {env:?}"
        );
        // And the scrub still runs for everything else.
        assert_eq!(
            env.iter()
                .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
                .map(|(_, v)| v.as_str()),
            Some(""),
            "naming one secret must not stop the rest being cleared"
        );
        assert!(
            profile.missing_secrets().is_empty(),
            "a secret that is set is not missing"
        );
    }

    #[test]
    fn a_profile_whose_secret_is_absent_says_so_before_launching() {
        // An agent launched without its key fails on authentication, in its own
        // wording, naming neither the profile nor the variable. This is the only
        // point where both are still known.
        let mut profile = AgentProfile::default_claude();
        profile.secrets_from_env = vec!["TERVIN_TEST_ABSENT_API_KEY".to_string()];
        assert!(std::env::var("TERVIN_TEST_ABSENT_API_KEY").is_err());

        assert_eq!(
            profile.missing_secrets(),
            vec!["TERVIN_TEST_ABSENT_API_KEY".to_string()]
        );
        let env = profile.resolved_env();
        assert!(
            !env.iter().any(|(k, _)| k == "TERVIN_TEST_ABSENT_API_KEY"),
            "an absent secret must not be launched as an empty value: {env:?}"
        );
    }

    #[test]
    fn a_value_set_in_the_profile_beats_the_environment_for_the_same_name() {
        // A hand-edited `agents.toml` is the user's own file: if they put a value in
        // `env` themselves, that is the value, and nothing here overwrites it.
        std::env::set_var("TERVIN_TEST_PROFILE_WINS_KEY", "from-the-environment");
        let mut profile = AgentProfile::default_claude();
        profile.env.insert(
            "TERVIN_TEST_PROFILE_WINS_KEY".to_string(),
            "from-the-profile".to_string(),
        );
        profile.secrets_from_env = vec!["TERVIN_TEST_PROFILE_WINS_KEY".to_string()];

        let env = profile.resolved_env();
        let entries: Vec<&(String, String)> = env
            .iter()
            .filter(|(k, _)| k == "TERVIN_TEST_PROFILE_WINS_KEY")
            .collect();
        assert_eq!(entries.len(), 1, "{env:?}");
        assert_eq!(entries[0].1, "from-the-profile");
        assert!(profile.missing_secrets().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_profile_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        // Both files as the app writes them on first run: `state.rs` calls exactly
        // these two functions. The mode is the point — an `agents.toml` left at the
        // umask is readable by every other account on a shared machine.
        let dir = scratch_dir();
        let config = ProfileConfig {
            default_profile: None,
            profiles: vec![AgentProfile::default_claude()],
        };
        let profiles = config
            .save_to(&dir.join("agents.toml"))
            .expect("saving the profile set failed");
        let mcp = dir.join("mcp.json");
        crate::McpConfig::write_example(&mcp).expect("writing the starter file failed");

        for path in [&profiles, &mcp] {
            let mode = std::fs::metadata(path)
                .expect("the file should exist")
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o077,
                0,
                "{} is readable beyond its owner: {:o}",
                path.display(),
                mode
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolved_env_scrubs_an_ambient_config_dir() {
        // The bug this prevents: Tervin launched from a shell where
        // CLAUDE_CONFIG_DIR is already set, running the wrong account under a
        // profile labelled otherwise.
        let profile = AgentProfile::default_claude();
        let env = profile.resolved_env();
        let cleared = env
            .iter()
            .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
            .expect("CLAUDE_CONFIG_DIR must be addressed explicitly");
        assert_eq!(cleared.1, "", "an unset profile must clear, not inherit");
    }

    #[test]
    fn a_profile_env_value_wins_and_is_tilde_expanded() {
        let mut profile = AgentProfile::default_claude();
        profile.env.insert(
            "CLAUDE_CONFIG_DIR".to_string(),
            "~/.claude-personal".to_string(),
        );
        let env = profile.resolved_env();
        let value = env
            .iter()
            .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
            .map(|(_, v)| v.clone())
            .unwrap();
        assert!(value.ends_with("/.claude-personal"));
        assert!(!value.starts_with('~'), "tilde must be expanded: {value}");
        // Exactly one entry for the key, so application order cannot matter.
        assert_eq!(
            env.iter().filter(|(k, _)| k == "CLAUDE_CONFIG_DIR").count(),
            1
        );
    }

    #[test]
    fn parent_session_variables_are_always_cleared() {
        // Without this a spawned agent believes it is a continuation of the
        // Claude Code session that launched Tervin.
        let env = AgentProfile::default_claude().resolved_env();
        for var in INHERITED_SESSION_VARS {
            let entry = env.iter().find(|(k, _)| k == var);
            assert_eq!(
                entry.map(|(_, v)| v.as_str()),
                Some(""),
                "{var} must be cleared for a child agent"
            );
        }
    }

    #[test]
    fn profiles_describe_what_makes_them_different() {
        let mut profile = AgentProfile::default_claude();
        assert_eq!(profile.describe(), "default configuration");
        profile.env.insert(
            "CLAUDE_CONFIG_DIR".to_string(),
            "~/.claude-work".to_string(),
        );
        assert!(profile.describe().starts_with("config: "));

        let mut keyed = AgentProfile::default_claude();
        keyed.secrets_from_env = vec!["ANTHROPIC_API_KEY".to_string()];
        assert_eq!(
            keyed.describe(),
            "ANTHROPIC_API_KEY from your environment",
            "a profile distinguished only by a secret still has to say what it is"
        );
    }

    #[test]
    fn config_round_trips_through_toml() {
        let mut config = ProfileConfig {
            default_profile: Some("claude-work".to_string()),
            profiles: vec![AgentProfile::default_claude()],
        };
        let mut work = AgentProfile::default_claude();
        work.id = "claude-work".to_string();
        work.name = "Claude · Work".to_string();
        work.env.insert(
            "CLAUDE_CONFIG_DIR".to_string(),
            "~/.claude-work".to_string(),
        );
        config.profiles.push(work);

        let text = toml::to_string_pretty(&config).unwrap();
        let parsed: ProfileConfig = toml::from_str(&text).unwrap();
        assert_eq!(parsed.profiles.len(), 2);
        assert_eq!(parsed.default_or_first().unwrap().id, "claude-work");
    }

    #[test]
    fn a_malformed_config_is_reported_not_silently_replaced() {
        let bad: Result<ProfileConfig, _> = toml::from_str("this is not toml {{{");
        assert!(bad.is_err());
    }

    #[test]
    fn a_plugin_data_directory_is_not_mistaken_for_an_account() {
        // Regression: `~/.claude-mem` is a plugin's data directory. It carries a
        // settings.json, which is why a settings-file test produced a profile
        // pointing at a plugin database instead of a Claude account.
        let base = std::env::temp_dir().join(format!("tervin-cfg-{}", uuid::Uuid::new_v4()));
        let plugin = base.join(".claude-plugin");
        let account = base.join(".claude-work");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::create_dir_all(&account).unwrap();

        std::fs::write(plugin.join("settings.json"), "{}").unwrap();
        std::fs::write(plugin.join("plugin.db"), "").unwrap();

        std::fs::write(account.join("settings.json"), "{}").unwrap();
        std::fs::write(account.join("history.jsonl"), "").unwrap();

        assert!(
            !looks_like_claude_config(&plugin),
            "a settings.json alone must not make a directory an account"
        );
        assert!(
            looks_like_claude_config(&account),
            "a session store identifies a real account"
        );

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_projects_directory_alone_identifies_an_account() {
        let base = std::env::temp_dir().join(format!("tervin-cfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(base.join("projects")).unwrap();
        assert!(looks_like_claude_config(&base));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn pretty_names_read_well() {
        assert_eq!(pretty_name("claude"), "Claude");
        assert_eq!(pretty_name("claude-work"), "Claude · Work");
        assert_eq!(pretty_name("claude-personal"), "Claude · Personal");
    }

    /// Everything that can start a Thread, and how the profile's environment reaches
    /// what it starts.
    ///
    /// This is the enforced half of the list on [`AgentProfile::resolved_env`]. The
    /// failure it guards is a new adapter that assembles a child environment itself:
    /// the scrub keeps running, the profile keeps looking right, and an ambient
    /// account variable decides which account runs anyway. A runtime that is not in
    /// this table fails the test, and a `Spawns` entry fails until its file actually
    /// calls `apply_env`.
    const LAUNCH_PATHS: &[(&str, &str, Reaches)] = &[
        (
            "ClaudeCodeRuntime",
            "claude/mod.rs",
            Reaches::Spawns("one `claude` process per Thread"),
        ),
        (
            "CodexRuntime",
            "codex/runtime.rs",
            Reaches::Spawns("one `codex exec` process per turn, so it is applied per turn"),
        ),
        (
            "AcpRuntime",
            "acp/mod.rs",
            Reaches::Spawns("one agent process per session, spoken to over its pipes"),
        ),
        (
            "LocalModelRuntime",
            "local/mod.rs",
            Reaches::StartsNothing("an HTTP client; there is no child to give an environment to"),
        ),
    ];

    /// How a runtime relates to a child environment. The string is the reason, not a
    /// label: a table entry that cannot say why is an entry nobody checked.
    enum Reaches {
        Spawns(&'static str),
        StartsNothing(&'static str),
    }

    /// Everything a file compiles outside `cfg(test)`, best-effort.
    ///
    /// The split is the file's first `#[cfg(test)]`, which holds because every file in
    /// this crate keeps its unit tests in one module at the end. Proved on a fixture
    /// in the test below before it is trusted on the tree.
    fn production_prefix(text: &str) -> &str {
        match text.split_once("#[cfg(test)]") {
            Some((before, _)) => before,
            None => text,
        }
    }

    fn crate_sources() -> Vec<PathBuf> {
        fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    found.push(path);
                }
            }
        }
        let mut found = Vec::new();
        walk(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut found,
        );
        found.sort();
        found
    }

    #[test]
    fn every_agent_runtime_is_named_with_how_it_gets_the_environment() {
        // Prove the cfg(test) split before trusting it, or a broken split would report
        // a clean crate while reading nothing.
        assert_eq!(
            production_prefix(
                "impl AgentRuntime for A {}\n#[cfg(test)]\nimpl AgentRuntime for B {}"
            )
            .matches("impl AgentRuntime for")
            .count(),
            1,
            "the cfg(test) split is broken"
        );

        let sources = crate_sources();
        assert!(
            sources.len() > 10,
            "walked {} files under this crate's src — the walk is broken",
            sources.len()
        );

        // Every `impl AgentRuntime for X` in production code, with the file it is in.
        let mut implemented: BTreeMap<String, String> = BTreeMap::new();
        let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        for path in &sources {
            let text =
                std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            // `split(..).skip(1)` rather than `split_once`, so a file holding two
            // adapters cannot hide the second one.
            for rest in production_prefix(&text)
                .split("impl AgentRuntime for")
                .skip(1)
            {
                let name: String = rest
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let relative = path
                    .strip_prefix(&src_root)
                    .unwrap_or(path)
                    .display()
                    .to_string();
                implemented.insert(name, relative);
            }
        }
        assert!(
            implemented.len() >= 4,
            "found {} AgentRuntime implementations — the scan is broken",
            implemented.len()
        );

        let listed: BTreeMap<&str, (&str, &Reaches)> = LAUNCH_PATHS
            .iter()
            .map(|(runtime, file, reaches)| (*runtime, (*file, reaches)))
            .collect();

        for (runtime, file) in &implemented {
            let (expected_file, reaches) = listed.get(runtime.as_str()).unwrap_or_else(|| {
                panic!(
                    "{runtime} in {file} can start a Thread but is not in LAUNCH_PATHS. \
                     Say how a profile's environment reaches what it starts, and if the \
                     answer is `apply_env`, `resolved_env`'s doc comment needs the name too."
                )
            });
            assert_eq!(
                file, expected_file,
                "{runtime} moved to {file}; LAUNCH_PATHS still says {expected_file}"
            );

            let text = std::fs::read_to_string(src_root.join(file))
                .unwrap_or_else(|e| panic!("{file}: {e}"));
            let production = production_prefix(&text);
            match reaches {
                Reaches::Spawns(what) => assert!(
                    production.contains("apply_env(&mut command"),
                    "{runtime} spawns {what} but {file} never calls `apply_env`. Passing \
                     the pairs to `Command::envs` instead sets a removal to an empty \
                     string, and an empty CLAUDE_CONFIG_DIR is an empty path, not an \
                     absent one."
                ),
                Reaches::StartsNothing(why) => assert!(
                    !production.contains("Command::new"),
                    "{file} starts a process, so `{why}` is no longer true and its \
                     environment is nobody's responsibility"
                ),
            }
        }

        let missing: Vec<&&str> = listed
            .keys()
            .filter(|runtime| !implemented.contains_key(**runtime))
            .collect();
        assert!(
            missing.is_empty(),
            "LAUNCH_PATHS lists {missing:?}, which no longer implements AgentRuntime. A \
             stale entry reads as coverage that is not there."
        );
    }
}
