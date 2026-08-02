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
    pub fn resolved_env(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();

        // Anything the profile sets wins outright.
        for (k, v) in &self.env {
            out.push((k.clone(), expand_tilde(v)));
        }

        // Clear identity-bearing variables the profile did not set, so an
        // ambient value cannot decide which account runs.
        for var in ACCOUNT_SELECTING_VARS
            .iter()
            .chain(INHERITED_SESSION_VARS.iter())
        {
            if !self.env.contains_key(*var) {
                out.push((var.to_string(), String::new()));
            }
        }

        out
    }

    /// A one-line description of what makes this profile distinct, for the picker.
    pub fn describe(&self) -> String {
        if let Some(dir) = self.env.get("CLAUDE_CONFIG_DIR") {
            return format!("config: {}", abbreviate(&expand_tilde(dir)));
        }
        if self.env.contains_key("ANTHROPIC_API_KEY") {
            return "API key from profile".to_string();
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
        let path = Self::path();
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
                      # Switch with the command palette, the composer's agent picker, or\n\
                      # `tervin agent --profile <id>`.\n\n";
        std::fs::write(&path, format!("{header}{text}"))?;
        Ok(path)
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

    text.lines()
        .filter_map(parse_alias_line)
        .map(|(name, profile)| ImportCandidate {
            source: format!("shell alias `{name}`"),
            profile,
        })
        .collect()
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

    // Leading VAR=value assignments, then the program.
    let mut env = BTreeMap::new();
    let mut rest = tokens.into_iter().peekable();
    while let Some(tok) = rest.peek() {
        match tok.split_once('=') {
            Some((k, v))
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                env.insert(k.to_string(), v.to_string());
                rest.next();
            }
            _ => break,
        }
    }

    let binary = rest.next()?;
    let program = binary.rsplit('/').next().unwrap_or(&binary).to_string();
    let runtime_id = runtime_for_program(&program)?;

    // An alias with no overrides adds nothing over the default profile.
    if env.is_empty() {
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
}
