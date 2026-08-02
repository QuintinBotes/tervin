//! Instruction files and MCP configuration that already exist in a project.
//!
//! Thirty-odd tools write instructions into a repository, and a user who has been
//! using any of them arrives with files already in place. Tervin should be able to
//! say what is in force. That is the whole feature.
//!
//! ## The distinction this module exists to make
//!
//! Finding `AGENTS.md` is trivial. The useful part is knowing **whether the runtime
//! you are about to launch will actually read it**, because the two failure modes
//! are quiet and expensive:
//!
//! - Tervin injects a file the runtime already reads, and the agent gets the same
//!   instructions twice. Not fatal, but it burns context and makes the agent's
//!   behaviour depend on something invisible.
//! - The user assumes an instruction file is in force when the runtime ignores it,
//!   then spends an hour wondering why the agent keeps doing the thing the file
//!   forbids.
//!
//! So every file is reported with a [`Readership`] per runtime, and one of its
//! variants is [`Readership::Unknown`]. A generic ACP agent may read anything or
//! nothing; Tervin does not know, and inventing an answer there would be worse than
//! admitting it.
//!
//! ## What was verified, rather than assumed
//!
//! The readership table below is not from documentation. `AGENTS.md` handling in
//! particular is easy to get wrong: the obvious assumption is that Claude Code reads
//! `CLAUDE.md` only and needs `AGENTS.md` passed in. That is false. Claude Code
//! 2.1.220 contains the string:
//!
//! ```text
//! Claude Code hardcodes CLAUDE.md / AGENTS.md discovery.
//! ```
//!
//! and treats the two as one discovery mechanism. Injecting `AGENTS.md` into Claude
//! Code would therefore duplicate it. The same binary also knows about
//! `.cursorrules`, `.github/copilot-instructions.md`, `GEMINI.md` and
//! `.windsurfrules`, but only inside its `/init` prompt and its migration adapter,
//! **not** in discovery, so those are correctly reported as ignored. Distinguishing
//! those two cases required reading the shipped binary; the version it was checked
//! against is recorded in [`VERIFIED_AGAINST`] so a future change is visible rather
//! than silently wrong.
//!
//! ## Discovery reads names, not contents
//!
//! Presence and size only. Contents are read when a user asks to see a file, or
//! when Tervin injects one into a runtime that reads nothing, and both are explicit
//! actions. Slurping every instruction file in a repository at startup would put
//! project text into memory that nobody asked for, which is the same promise
//! [`crate::mcp`] makes about servers.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The runtime versions the readership table was checked against.
///
/// Recorded because this table is a claim about other people's software. When one of
/// these changes behaviour, the honest failure is a stale note here rather than a
/// confident wrong answer in the UI.
pub const VERIFIED_AGAINST: &[(&str, &str)] = &[
    ("claude-code", "2.1.220"),
    // Checked earlier in development against the JSON event envelope and
    // `codex app-server generate-json-schema`.
    ("codex", "0.146.0"),
];

/// A budget on directories visited while looking for nested instruction files.
///
/// Claude Code reads a `CLAUDE.md` in any ancestor directory of a file it touches,
/// so nested files genuinely matter and a shallow search misses real ones.
///
/// The first version of this used a depth limit of three instead. Run against a real
/// repository it set `truncated` every single time, because every repository has
/// something more than three levels deep. A caveat that is always on is worse than no
/// caveat: it stops carrying information and users learn to skip it. So the search is
/// bounded by total work rather than by depth, which means it normally completes and
/// the flag means something when it appears.
const MAX_DIRS: usize = 4_000;

/// A generous depth stop, as defence rather than as the working limit.
///
/// Not expected to be reached. It exists so a pathologically deep tree cannot make
/// the queue grow without bound.
const MAX_DEPTH: usize = 24;

/// A cap on how many nested files to report.
const MAX_NESTED: usize = 100;

/// Directories never descended into, because an instruction file inside one is not
/// governing the user's project.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    "vendor",
    "__pycache__",
    ".venv",
    "venv",
];

/// A kind of instruction file, identified by the tool that established it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionKind {
    /// `AGENTS.md`. The nearest thing to a cross-tool standard.
    Agents,
    /// `CLAUDE.md`.
    ClaudeMd,
    /// `CLAUDE.local.md`, conventionally gitignored personal overrides.
    ClaudeLocal,
    /// `.cursorrules`, or a file under `.cursor/rules/`.
    CursorRules,
    /// `.github/copilot-instructions.md`.
    CopilotInstructions,
    /// `GEMINI.md`.
    GeminiMd,
    /// `.windsurfrules`, or a file under `.windsurf/rules/`.
    WindsurfRules,
    /// `.clinerules`.
    ClineRules,
}

impl InstructionKind {
    /// The tool that reads this by convention, for display.
    pub fn origin(&self) -> &'static str {
        match self {
            Self::Agents => "the AGENTS.md convention",
            Self::ClaudeMd | Self::ClaudeLocal => "Claude Code",
            Self::CursorRules => "Cursor",
            Self::CopilotInstructions => "GitHub Copilot",
            Self::GeminiMd => "Gemini CLI",
            Self::WindsurfRules => "Windsurf",
            Self::ClineRules => "Cline",
        }
    }

    /// Whether this file is conventionally personal rather than committed.
    ///
    /// Worth surfacing: a teammate cloning the repo does not have it, so an agent
    /// behaving differently for two people is explained by this and nothing else.
    pub fn is_personal(&self) -> bool {
        matches!(self, Self::ClaudeLocal)
    }
}

/// Where a file sits relative to the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum Scope {
    /// Under the user's home directory, so it applies to every project.
    User,
    /// At the project root.
    ProjectRoot,
    /// In a subdirectory, so it governs only part of the tree.
    Nested { relative_dir: String },
}

/// An instruction file that exists on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionFile {
    pub path: PathBuf,
    pub kind: InstructionKind,
    pub scope: Scope,
    /// Size in bytes. Shown because a 40 KB instruction file is worth knowing about
    /// before it is added to a context window.
    pub bytes: u64,
}

/// Whether a given runtime will actually read a given file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "readership", rename_all = "snake_case")]
pub enum Readership {
    /// The runtime discovers and reads this itself. Tervin must not also inject it.
    Native {
        /// How this is known, shown verbatim so the claim is auditable.
        ///
        /// Owned rather than `&'static str` because this type crosses the Tauri
        /// boundary, and `Deserialize` cannot produce a `'static` borrow.
        evidence: String,
    },
    /// Found, but this runtime does not read it. Reported so nobody assumes it is
    /// in force.
    Ignored,
    /// The runtime reads no instruction files, so Tervin can supply the text. Still
    /// not automatic: injecting changes what the agent was told.
    Injectable,
    /// Tervin cannot tell. A generic agent may read anything or nothing, and a
    /// guess here is worse than an admission.
    Unknown,
}

impl Readership {
    /// One line for the UI.
    pub fn summary(&self) -> String {
        match self {
            Self::Native { .. } => "read by the runtime itself".to_string(),
            Self::Ignored => "not read by this runtime".to_string(),
            Self::Injectable => "Tervin can pass this in".to_string(),
            Self::Unknown => "unknown whether this runtime reads it".to_string(),
        }
    }

    /// Whether the instructions are actually governing the agent.
    ///
    /// `Injectable` is deliberately false: the runtime *could* be given the file,
    /// but until a user asks, it has not been.
    pub fn in_force(&self) -> bool {
        matches!(self, Self::Native { .. })
    }
}

/// Evidence strings, kept as constants so the same claim reads identically wherever
/// it appears and there is exactly one place to correct it.
const CC_HARDCODED: &str =
    "Claude Code hardcodes CLAUDE.md / AGENTS.md discovery (verified in 2.1.220)";
const CODEX_AGENTS: &str = "Codex reads AGENTS.md as its instruction file";
const GEMINI_CONTEXT: &str = "Gemini CLI reads GEMINI.md as its context file";
const CURSOR_RULES: &str = "Cursor reads .cursorrules and .cursor/rules/";
const COPILOT_FILE: &str = "Copilot reads .github/copilot-instructions.md";

/// Whether `runtime_id` reads `kind`.
///
/// The table is deliberately explicit rather than pattern-matched on substrings: an
/// id that merely contains "claude" is not necessarily Claude Code, and a wrong
/// `Native` is the most damaging answer this function can give, because it silences
/// the injection path *and* tells the user the file is in force.
pub fn readership(kind: InstructionKind, runtime_id: &str) -> Readership {
    use InstructionKind as K;

    match runtime_id {
        // Claude Code, directly or behind its ACP shim: one discovery mechanism
        // covering both filenames, plus its own local override.
        "claude-code" | "claude-code-acp" => match kind {
            K::Agents | K::ClaudeMd | K::ClaudeLocal => Readership::Native {
                evidence: CC_HARDCODED.to_string(),
            },
            _ => Readership::Ignored,
        },

        "codex" => match kind {
            K::Agents => Readership::Native {
                evidence: CODEX_AGENTS.to_string(),
            },
            _ => Readership::Ignored,
        },

        "gemini" | "gemini-acp" => match kind {
            K::GeminiMd => Readership::Native {
                evidence: GEMINI_CONTEXT.to_string(),
            },
            // Gemini CLI is not known to read AGENTS.md, so it is reported as
            // ignored rather than assumed. If that changes, this is the line.
            _ => Readership::Ignored,
        },

        "cursor-agent" => match kind {
            K::CursorRules => Readership::Native {
                evidence: CURSOR_RULES.to_string(),
            },
            _ => Readership::Ignored,
        },

        "copilot-acp" => match kind {
            K::CopilotInstructions => Readership::Native {
                evidence: COPILOT_FILE.to_string(),
            },
            _ => Readership::Ignored,
        },

        // Local model endpoints are conversational: they receive a prompt and
        // nothing else, so every instruction file is Tervin's to pass or withhold.
        "lmstudio" | "ollama" | "vllm" | "llamacpp" => Readership::Injectable,

        // Anything else, including a generic ACP agent and any runtime added after
        // this table was written.
        _ => Readership::Unknown,
    }
}

/// A file paired with what a specific runtime will do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InForce {
    pub file: InstructionFile,
    pub readership: Readership,
}

/// The result of a discovery pass.
///
/// Carries `truncated` because a bounded walk that presents itself as complete is
/// the kind of quiet inaccuracy this project treats as a bug.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovered {
    pub files: Vec<InstructionFile>,
    pub mcp: Vec<McpConfigFile>,
    /// True when the nested walk hit [`MAX_DEPTH`] or [`MAX_NESTED`], so the list is
    /// a sample rather than everything.
    pub truncated: bool,
}

impl Discovered {
    /// Pair every file with what `runtime_id` does about it.
    pub fn for_runtime(&self, runtime_id: &str) -> Vec<InForce> {
        self.files
            .iter()
            .map(|f| InForce {
                file: f.clone(),
                readership: readership(f.kind, runtime_id),
            })
            .collect()
    }

    /// One sentence for the Bridge panel header.
    pub fn summary(&self, runtime_id: &str) -> String {
        let paired = self.for_runtime(runtime_id);
        let native = paired.iter().filter(|p| p.readership.in_force()).count();
        let ignored = paired
            .iter()
            .filter(|p| matches!(p.readership, Readership::Ignored))
            .count();
        let unknown = paired
            .iter()
            .filter(|p| matches!(p.readership, Readership::Unknown))
            .count();

        if paired.is_empty() {
            // Not simply "none found": the walk may have stopped before reaching
            // anything, and saying "none" then is the false-completeness claim that
            // `truncated` exists to prevent. This was caught by a test rather than
            // by review.
            return if self.truncated {
                "no instruction files found near the root, and the nested search was \
                 capped, so there may be some deeper"
                    .to_string()
            } else {
                "no instruction files found".to_string()
            };
        }
        let mut parts = Vec::new();
        if native > 0 {
            parts.push(format!("{native} in force"));
        }
        if ignored > 0 {
            parts.push(format!("{ignored} this runtime ignores"));
        }
        if unknown > 0 {
            parts.push(format!("{unknown} unknown"));
        }
        let injectable = paired
            .iter()
            .filter(|p| matches!(p.readership, Readership::Injectable))
            .count();
        if injectable > 0 {
            parts.push(format!("{injectable} Tervin could pass in"));
        }
        let mut s = parts.join(", ");
        if self.truncated {
            s.push_str(" (nested search was capped, so there may be more)");
        }
        s
    }
}

/// A kind of MCP configuration file belonging to another tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpConfigKind {
    /// `.mcp.json` at a project root. The closest thing to a shared convention.
    ProjectMcpJson,
    /// `~/.claude.json`, Claude Code's own store.
    ClaudeJson,
    /// `.codex/config.toml` or `~/.codex/config.toml`.
    CodexToml,
    /// `.gemini/settings.json` or `~/.gemini/settings.json`.
    GeminiSettings,
}

impl McpConfigKind {
    pub fn owner(&self) -> &'static str {
        match self {
            Self::ProjectMcpJson => "the .mcp.json convention",
            Self::ClaudeJson => "Claude Code",
            Self::CodexToml => "Codex",
            Self::GeminiSettings => "Gemini CLI",
        }
    }
}

/// An MCP configuration file found on the machine.
///
/// Server *names* are parsed, never their commands or environment: a name is enough
/// to tell a user what is configured, and an MCP entry routinely carries an API key
/// in `env`. Reading those into Tervin's memory to render a panel would be
/// gratuitous, and [`crate::mcp`] already promises not to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfigFile {
    pub path: PathBuf,
    pub kind: McpConfigKind,
    pub servers: Vec<String>,
    /// Set when the file exists but could not be parsed. Reported rather than
    /// dropped, because a malformed config that a runtime silently ignores is
    /// exactly the situation a user cannot diagnose alone.
    pub error: Option<String>,
}

/// Discover instruction files and MCP configs for a project.
///
/// `home` is a parameter rather than read from the environment so tests can point it
/// at a temporary directory instead of touching the developer's real config.
pub fn discover(project_root: &Path, home: &Path) -> Discovered {
    let mut out = Discovered::default();

    // User scope first, because it applies to everything and reads as the outermost
    // layer in the panel.
    for (rel, kind) in [
        (".claude/CLAUDE.md", InstructionKind::ClaudeMd),
        (".codex/AGENTS.md", InstructionKind::Agents),
        (".gemini/GEMINI.md", InstructionKind::GeminiMd),
    ] {
        let p = home.join(rel);
        if let Some(bytes) = file_size(&p) {
            out.files.push(InstructionFile {
                path: p,
                kind,
                scope: Scope::User,
                bytes,
            });
        }
    }

    // Project root.
    for (rel, kind) in root_candidates() {
        let p = project_root.join(rel);
        if let Some(bytes) = file_size(&p) {
            out.files.push(InstructionFile {
                path: p,
                kind,
                scope: Scope::ProjectRoot,
                bytes,
            });
        }
    }

    // Rule *directories*, where each file inside is its own rule.
    for (dir, kind) in [
        (".cursor/rules", InstructionKind::CursorRules),
        (".windsurf/rules", InstructionKind::WindsurfRules),
    ] {
        let d = project_root.join(dir);
        if let Ok(entries) = std::fs::read_dir(&d) {
            let mut found: Vec<_> = entries
                .flatten()
                .filter(|e| e.path().is_file())
                .map(|e| e.path())
                .collect();
            // Sorted so the panel is stable between launches; `read_dir` order is
            // whatever the filesystem gives.
            found.sort();
            for p in found {
                if let Some(bytes) = file_size(&p) {
                    out.files.push(InstructionFile {
                        path: p,
                        kind,
                        scope: Scope::ProjectRoot,
                        bytes,
                    });
                }
            }
        }
    }

    // Nested files, bounded.
    let (nested, truncated) = walk_nested(project_root);
    out.truncated = truncated;
    out.files.extend(nested);

    out.mcp = discover_mcp(project_root, home);
    out
}

/// The files looked for at a project root, with the kind each maps to.
fn root_candidates() -> Vec<(&'static str, InstructionKind)> {
    vec![
        ("AGENTS.md", InstructionKind::Agents),
        ("CLAUDE.md", InstructionKind::ClaudeMd),
        ("CLAUDE.local.md", InstructionKind::ClaudeLocal),
        (".cursorrules", InstructionKind::CursorRules),
        (
            ".github/copilot-instructions.md",
            InstructionKind::CopilotInstructions,
        ),
        ("GEMINI.md", InstructionKind::GeminiMd),
        (".windsurfrules", InstructionKind::WindsurfRules),
        (".clinerules", InstructionKind::ClineRules),
    ]
}

/// Size of a regular file, or `None` if it is absent or not a file.
///
/// A directory named `AGENTS.md` is not an instruction file, and neither is a broken
/// symlink; both would otherwise be reported as present with an odd size.
fn file_size(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    meta.is_file().then_some(meta.len())
}

/// Walk subdirectories for the instruction files that meaningfully nest.
///
/// Only `AGENTS.md`, `CLAUDE.md` and `CLAUDE.local.md`, because those are the ones a
/// runtime resolves per directory. A nested `.cursorrules` is not a thing Cursor
/// looks for, so reporting one would imply a mechanism that does not exist.
fn walk_nested(root: &Path) -> (Vec<InstructionFile>, bool) {
    let mut out = Vec::new();
    let mut truncated = false;
    let mut visited = 0usize;
    // (directory, depth). Breadth-first, so when a cap is hit the files kept are the
    // ones nearest the root, which are the ones most likely to matter.
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = std::collections::VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));

    while let Some((dir, depth)) = queue.pop_front() {
        if visited >= MAX_DIRS || depth >= MAX_DEPTH {
            // Something was left unexamined, so the result is genuinely partial.
            truncated = true;
            continue;
        }
        visited += 1;
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            // An unreadable directory is not an error worth surfacing here: it is
            // simply not contributing instruction files.
            Err(_) => continue,
        };
        let mut subdirs: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            // `file_type` does not follow symlinks, unlike `path.is_dir()`. That
            // matters: a link pointing at an ancestor is an ordinary thing to find in
            // a repository, and following it would walk the same tree repeatedly
            // until the budget ran out, then report a truncated result for a project
            // that has almost nothing in it.
            let is_real_dir = entry
                .file_type()
                .map(|t| t.is_dir())
                .unwrap_or_else(|_| path.is_dir());
            if is_real_dir {
                if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    subdirs.push(path);
                }
                continue;
            }
            // A symlink to a file is still an instruction file, so those fall through
            // to the filename check below and `file_size` resolves them.
            // The root itself is handled by `root_candidates`; this pass is only
            // about directories below it.
            if depth == 0 {
                continue;
            }
            let kind = match name.as_str() {
                "AGENTS.md" => InstructionKind::Agents,
                "CLAUDE.md" => InstructionKind::ClaudeMd,
                "CLAUDE.local.md" => InstructionKind::ClaudeLocal,
                _ => continue,
            };
            if out.len() >= MAX_NESTED {
                truncated = true;
                continue;
            }
            let relative_dir = dir
                .strip_prefix(root)
                .unwrap_or(&dir)
                .to_string_lossy()
                .to_string();
            if let Some(bytes) = file_size(&path) {
                out.push(InstructionFile {
                    path,
                    kind,
                    scope: Scope::Nested { relative_dir },
                    bytes,
                });
            }
        }
        subdirs.sort();
        for sub in subdirs {
            queue.push_back((sub, depth + 1));
        }
    }
    (out, truncated)
}

/// Find MCP configuration belonging to other tools.
pub fn discover_mcp(project_root: &Path, home: &Path) -> Vec<McpConfigFile> {
    let mut out = Vec::new();

    let candidates: Vec<(PathBuf, McpConfigKind)> = vec![
        (
            project_root.join(".mcp.json"),
            McpConfigKind::ProjectMcpJson,
        ),
        (home.join(".claude.json"), McpConfigKind::ClaudeJson),
        (
            project_root.join(".codex/config.toml"),
            McpConfigKind::CodexToml,
        ),
        (home.join(".codex/config.toml"), McpConfigKind::CodexToml),
        (
            project_root.join(".gemini/settings.json"),
            McpConfigKind::GeminiSettings,
        ),
        (
            home.join(".gemini/settings.json"),
            McpConfigKind::GeminiSettings,
        ),
    ];

    for (path, kind) in candidates {
        if file_size(&path).is_none() {
            continue;
        }
        let (servers, error) = match kind {
            McpConfigKind::CodexToml => parse_codex_toml(&path),
            _ => parse_json_servers(&path),
        };
        out.push(McpConfigFile {
            path,
            kind,
            servers,
            error,
        });
    }
    out
}

/// Pull server names out of any file using the conventional `mcpServers` object.
///
/// Only names. See [`McpConfigFile`] for why the rest of each entry is left on disk.
fn parse_json_servers(path: &Path) -> (Vec<String>, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return (Vec::new(), Some(format!("could not read: {e}"))),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), Some(format!("not valid JSON: {e}"))),
    };

    let mut names = Vec::new();
    // Top level, which is what `.mcp.json` and Gemini's settings use.
    collect_server_names(&value, &mut names);
    // `~/.claude.json` keys its configuration by project path, so the servers for
    // any project live one level down. Collected without caring which project,
    // because the panel is reporting what exists on the machine.
    if let Some(projects) = value.get("projects").and_then(|p| p.as_object()) {
        for entry in projects.values() {
            collect_server_names(entry, &mut names);
        }
    }
    names.sort();
    names.dedup();
    (names, None)
}

fn collect_server_names(value: &serde_json::Value, out: &mut Vec<String>) {
    if let Some(servers) = value.get("mcpServers").and_then(|s| s.as_object()) {
        out.extend(servers.keys().cloned());
    }
}

/// Pull server names out of Codex's TOML, where they live under `mcp_servers`.
fn parse_codex_toml(path: &Path) -> (Vec<String>, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return (Vec::new(), Some(format!("could not read: {e}"))),
    };
    // `toml::Table`, not `toml::Value`: in toml 1.x a `Value`'s `FromStr` parses a
    // single value, so a whole document fails with "unexpected content, expected
    // nothing" after the first table header. That error is easy to mistake for a
    // malformed config file, and it would have reported every Codex user's servers
    // as unparseable.
    let doc: toml::Table = match toml::from_str(&text) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), Some(format!("not valid TOML: {e}"))),
    };
    let mut names: Vec<String> = doc
        .get("mcp_servers")
        .and_then(|t| t.as_table())
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    names.sort();
    (names, None)
}

/// An MCP server found in another tool's configuration, offered for adoption.
///
/// Never adopted automatically. Tervin supplies MCP servers to ACP agents, which
/// have no configuration of their own, so copying a server here genuinely adds
/// tools to an agent. That is the user's decision, exactly as it is for a profile
/// in [`crate::profile::ImportCandidate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAdoption {
    pub name: String,
    /// Shown verbatim before the user accepts it.
    pub source: String,
    /// True when Tervin already has a server with this name, so accepting would
    /// overwrite rather than add.
    pub conflicts: bool,
}

/// Which discovered servers Tervin could adopt, given what it already has.
pub fn adoption_candidates(found: &[McpConfigFile], existing: &[String]) -> Vec<McpAdoption> {
    let mut out: Vec<McpAdoption> = Vec::new();
    for file in found {
        for name in &file.servers {
            // A server configured in two places is one offer, attributed to the
            // first file it was seen in.
            if out.iter().any(|c| &c.name == name) {
                continue;
            }
            out.push(McpAdoption {
                name: name.clone(),
                source: format!("{} ({})", file.path.display(), file.kind.owner()),
                conflicts: existing.iter().any(|e| e == name),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A temporary directory that cleans itself up, so tests never touch the real
    /// home directory.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!(
                "tervin-instructions-{tag}-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn write(&self, rel: &str, contents: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, contents).unwrap();
            p
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn kinds(d: &Discovered) -> Vec<InstructionKind> {
        d.files.iter().map(|f| f.kind).collect()
    }

    #[test]
    fn claude_code_reads_agents_md_natively_so_tervin_must_not_inject_it() {
        // The single most important row in the table, and the one an implementation
        // gets wrong by assuming CLAUDE.md is the only file Claude Code knows.
        // Verified against the shipped 2.1.220 binary, which contains
        // "Claude Code hardcodes CLAUDE.md / AGENTS.md discovery".
        let r = readership(InstructionKind::Agents, "claude-code");
        assert!(
            r.in_force(),
            "Claude Code reads AGENTS.md itself; reporting otherwise leads to it \
             being injected twice"
        );
        match r {
            Readership::Native { evidence } => assert!(evidence.contains("hardcodes")),
            other => panic!("expected Native, got {other:?}"),
        }
        // And it is never Injectable, which is the state that would cause the
        // duplicate.
        assert!(!matches!(
            readership(InstructionKind::Agents, "claude-code"),
            Readership::Injectable
        ));
    }

    #[test]
    fn the_acp_shim_for_claude_code_has_the_same_readership_as_the_cli() {
        // Same program behind a different transport, so a different answer here
        // would be incoherent.
        for kind in [
            InstructionKind::Agents,
            InstructionKind::ClaudeMd,
            InstructionKind::ClaudeLocal,
        ] {
            assert_eq!(
                readership(kind, "claude-code"),
                readership(kind, "claude-code-acp"),
                "{kind:?} differs between the CLI and its ACP shim"
            );
        }
    }

    #[test]
    fn files_claude_code_only_knows_from_init_are_reported_as_ignored() {
        // The binary contains ".cursorrules", "copilot-instructions", "GEMINI.md"
        // and ".windsurfrules", but only inside the /init prompt and the migration
        // adapter. Treating a mention as discovery would tell a user their Cursor
        // rules are in force when they are not.
        for kind in [
            InstructionKind::CursorRules,
            InstructionKind::CopilotInstructions,
            InstructionKind::GeminiMd,
            InstructionKind::WindsurfRules,
            InstructionKind::ClineRules,
        ] {
            assert_eq!(
                readership(kind, "claude-code"),
                Readership::Ignored,
                "{kind:?} should be reported as ignored by Claude Code"
            );
        }
    }

    #[test]
    fn codex_reads_agents_md_but_not_claude_md() {
        assert!(readership(InstructionKind::Agents, "codex").in_force());
        assert_eq!(
            readership(InstructionKind::ClaudeMd, "codex"),
            Readership::Ignored
        );
    }

    #[test]
    fn local_models_read_nothing_so_everything_is_injectable() {
        // They receive a prompt and nothing else. This is the only runtime family
        // where Tervin supplying the file is the right behaviour.
        for id in ["lmstudio", "ollama", "vllm", "llamacpp"] {
            for kind in [InstructionKind::Agents, InstructionKind::ClaudeMd] {
                assert_eq!(
                    readership(kind, id),
                    Readership::Injectable,
                    "{id} should treat {kind:?} as injectable"
                );
            }
        }
    }

    #[test]
    fn an_unknown_runtime_reports_unknown_rather_than_guessing() {
        // A generic ACP agent may read anything or nothing. Both a confident
        // "native" and a confident "ignored" would be wrong; Injectable would be
        // worst, because it would inject into an agent that may already have read
        // the file.
        let r = readership(InstructionKind::Agents, "some-agent-invented-tomorrow");
        assert_eq!(r, Readership::Unknown);
        assert!(!r.in_force());
        assert!(r.summary().contains("unknown"));
    }

    #[test]
    fn an_id_that_merely_contains_claude_is_not_treated_as_claude_code() {
        // The failure mode of a substring match. "claude-ish" is not Claude Code,
        // and claiming AGENTS.md is natively read would suppress injection for an
        // agent that may need it.
        assert_eq!(
            readership(InstructionKind::Agents, "claude-ish"),
            Readership::Unknown
        );
        assert_eq!(
            readership(InstructionKind::Agents, "not-codex-really"),
            Readership::Unknown
        );
    }

    #[test]
    fn every_kind_has_an_origin_and_only_the_local_file_is_personal() {
        let all = [
            InstructionKind::Agents,
            InstructionKind::ClaudeMd,
            InstructionKind::ClaudeLocal,
            InstructionKind::CursorRules,
            InstructionKind::CopilotInstructions,
            InstructionKind::GeminiMd,
            InstructionKind::WindsurfRules,
            InstructionKind::ClineRules,
        ];
        for k in all {
            assert!(!k.origin().is_empty(), "{k:?} has no origin");
        }
        assert_eq!(all.iter().filter(|k| k.is_personal()).count(), 1);
        assert!(InstructionKind::ClaudeLocal.is_personal());
    }

    #[test]
    fn discovery_finds_root_files_and_classifies_their_scope() {
        let proj = TempDir::new("root");
        let home = TempDir::new("home");
        proj.write("AGENTS.md", "# rules\n");
        proj.write("CLAUDE.md", "# claude\n");
        proj.write(".github/copilot-instructions.md", "# copilot\n");

        let d = discover(proj.path(), home.path());
        let found = kinds(&d);
        assert!(found.contains(&InstructionKind::Agents));
        assert!(found.contains(&InstructionKind::ClaudeMd));
        assert!(found.contains(&InstructionKind::CopilotInstructions));
        assert!(d.files.iter().all(|f| f.scope == Scope::ProjectRoot));
        assert!(d.files.iter().all(|f| f.bytes > 0));
    }

    #[test]
    fn a_directory_named_like_an_instruction_file_is_not_reported_as_one() {
        // `metadata().len()` on a directory returns a number, so without the
        // is_file check this would appear in the panel as a real file with a
        // nonsense size.
        let proj = TempDir::new("dir");
        let home = TempDir::new("dirhome");
        fs::create_dir_all(proj.path().join("AGENTS.md")).unwrap();

        let d = discover(proj.path(), home.path());
        assert!(
            !kinds(&d).contains(&InstructionKind::Agents),
            "a directory must not be reported as an instruction file"
        );
    }

    #[test]
    fn an_empty_file_is_still_reported_because_it_is_still_in_force() {
        // Zero bytes is not the same as absent: the runtime reads it, finds nothing,
        // and a user asking "why are my instructions ignored" needs to see that the
        // file is empty rather than missing.
        let proj = TempDir::new("empty");
        let home = TempDir::new("emptyhome");
        proj.write("AGENTS.md", "");

        let d = discover(proj.path(), home.path());
        let f = d
            .files
            .iter()
            .find(|f| f.kind == InstructionKind::Agents)
            .expect("an empty AGENTS.md should still be found");
        assert_eq!(f.bytes, 0);
    }

    #[test]
    fn user_scope_files_are_found_and_marked_as_user_scope() {
        let proj = TempDir::new("uproj");
        let home = TempDir::new("uhome");
        home.write(".claude/CLAUDE.md", "# global\n");

        let d = discover(proj.path(), home.path());
        let f = d
            .files
            .iter()
            .find(|f| f.kind == InstructionKind::ClaudeMd)
            .expect("the user-level CLAUDE.md should be found");
        assert_eq!(f.scope, Scope::User);
    }

    #[test]
    fn nested_files_record_the_directory_they_govern() {
        let proj = TempDir::new("nested");
        let home = TempDir::new("nestedhome");
        proj.write("CLAUDE.md", "root\n");
        proj.write("crates/engine/CLAUDE.md", "engine only\n");

        let d = discover(proj.path(), home.path());
        let nested: Vec<_> = d
            .files
            .iter()
            .filter(|f| matches!(f.scope, Scope::Nested { .. }))
            .collect();
        assert_eq!(nested.len(), 1, "expected exactly one nested file");
        match &nested[0].scope {
            Scope::Nested { relative_dir } => {
                assert!(
                    relative_dir.contains("engine"),
                    "the governed directory should be named: {relative_dir}"
                );
            }
            other => panic!("expected Nested, got {other:?}"),
        }
        // The root file must not also be reported as nested.
        assert_eq!(
            d.files
                .iter()
                .filter(|f| f.scope == Scope::ProjectRoot && f.kind == InstructionKind::ClaudeMd)
                .count(),
            1
        );
    }

    #[test]
    fn the_root_file_is_reported_once_and_not_duplicated_by_the_nested_walk() {
        // Both passes visit the root directory, so this is the obvious duplicate.
        let proj = TempDir::new("dup");
        let home = TempDir::new("duphome");
        proj.write("AGENTS.md", "x\n");

        let d = discover(proj.path(), home.path());
        assert_eq!(
            d.files
                .iter()
                .filter(|f| f.kind == InstructionKind::Agents)
                .count(),
            1,
            "the root AGENTS.md was reported twice"
        );
    }

    #[test]
    fn skipped_directories_are_not_searched() {
        // A CLAUDE.md inside a dependency is not governing this project, and
        // reporting one would be noise in every JS repository on earth.
        let proj = TempDir::new("skip");
        let home = TempDir::new("skiphome");
        proj.write("node_modules/some-pkg/CLAUDE.md", "not ours\n");
        proj.write("target/debug/AGENTS.md", "not ours\n");

        let d = discover(proj.path(), home.path());
        assert!(
            d.files.is_empty(),
            "found files inside skipped directories: {:?}",
            d.files
        );
    }

    #[test]
    fn a_deep_but_ordinary_tree_is_searched_in_full_and_not_reported_as_capped() {
        // This test used to assert the opposite, against a depth limit of three. Run
        // against a real repository that limit tripped every time, so the caveat was
        // permanently on and carried no information. Depth is no longer the working
        // bound, and a file six levels down is found.
        let proj = TempDir::new("deep");
        let home = TempDir::new("deephome");
        proj.write("a/b/c/d/e/f/CLAUDE.md", "deep\n");

        let d = discover(proj.path(), home.path());
        assert!(
            !d.truncated,
            "an ordinary nested tree must not be reported as capped"
        );
        let found = d
            .files
            .iter()
            .find(|f| f.kind == InstructionKind::ClaudeMd)
            .expect("a CLAUDE.md six levels down should be found");
        match &found.scope {
            Scope::Nested { relative_dir } => assert!(relative_dir.contains("f")),
            other => panic!("expected Nested, got {other:?}"),
        }
    }

    #[test]
    fn hitting_the_file_cap_is_reported_as_truncated() {
        // The cap that does still exist. A partial list presented as complete is the
        // failure this flag prevents.
        let proj = TempDir::new("many");
        let home = TempDir::new("manyhome");
        for i in 0..(MAX_NESTED + 5) {
            proj.write(&format!("d{i}/CLAUDE.md"), "x\n");
        }

        let d = discover(proj.path(), home.path());
        assert!(d.truncated, "exceeding MAX_NESTED must set truncated");
        assert_eq!(
            d.files.len(),
            MAX_NESTED,
            "the cap should be honoured exactly"
        );
        assert!(d.summary("claude-code").contains("capped"));
    }

    #[test]
    fn a_symlink_loop_does_not_hang_or_exhaust_the_budget() {
        // A link pointing at an ancestor is an ordinary thing to find in a
        // repository. `path.is_dir()` follows symlinks, so descending on it would
        // walk the same tree until the directory budget ran out and then report a
        // truncated result for a project containing two files.
        let proj = TempDir::new("loop");
        let home = TempDir::new("loophome");
        proj.write("sub/CLAUDE.md", "real\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink(proj.path(), proj.path().join("sub/back")).unwrap();

        let d = discover(proj.path(), home.path());
        assert!(
            !d.truncated,
            "a symlink loop was followed until the budget ran out"
        );
        assert_eq!(
            d.files.len(),
            1,
            "the same file was reported more than once: {:?}",
            d.files
        );
    }

    #[test]
    fn a_symlinked_instruction_file_is_still_reported() {
        // The other side of not following links: a symlink to a real file is a real
        // instruction file, and skipping it would lose a genuine source.
        let proj = TempDir::new("slink");
        let home = TempDir::new("slinkhome");
        let real = proj.write("shared/rules.md", "shared\n");
        std::fs::create_dir_all(proj.path().join("pkg")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, proj.path().join("pkg/CLAUDE.md")).unwrap();

        let d = discover(proj.path(), home.path());
        assert!(
            d.files
                .iter()
                .any(|f| f.kind == InstructionKind::ClaudeMd && f.bytes > 0),
            "a symlinked CLAUDE.md should be reported: {:?}",
            d.files
        );
    }

    #[test]
    fn a_shallow_tree_does_not_claim_to_have_been_capped() {
        // The other half: a false "there may be more" is its own small lie.
        let proj = TempDir::new("shallow");
        let home = TempDir::new("shallowhome");
        proj.write("AGENTS.md", "x\n");
        proj.write("sub/CLAUDE.md", "y\n");

        let d = discover(proj.path(), home.path());
        assert!(!d.truncated, "a shallow tree was reported as capped");
        assert!(!d.summary("claude-code").contains("capped"));
    }

    #[test]
    fn rule_directories_report_each_file_and_in_a_stable_order() {
        let proj = TempDir::new("rules");
        let home = TempDir::new("ruleshome");
        proj.write(".cursor/rules/z-last.mdc", "z\n");
        proj.write(".cursor/rules/a-first.mdc", "a\n");

        let d = discover(proj.path(), home.path());
        let names: Vec<String> = d
            .files
            .iter()
            .filter(|f| f.kind == InstructionKind::CursorRules)
            .map(|f| f.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a-first.mdc", "z-last.mdc"]);
    }

    #[test]
    fn the_summary_separates_in_force_from_ignored_for_the_same_project() {
        // The point of the whole module: identical files, different runtime, and
        // the panel must not read the same way.
        let proj = TempDir::new("sum");
        let home = TempDir::new("sumhome");
        proj.write("AGENTS.md", "a\n");
        proj.write(".cursorrules", "c\n");

        let d = discover(proj.path(), home.path());

        let cc = d.summary("claude-code");
        assert!(cc.contains("1 in force"), "claude-code summary: {cc}");
        assert!(cc.contains("1 this runtime ignores"), "claude-code: {cc}");

        let cursor = d.summary("cursor-agent");
        assert!(cursor.contains("1 in force"), "cursor summary: {cursor}");

        let ollama = d.summary("ollama");
        assert!(
            ollama.contains("2 Tervin could pass in"),
            "a local model reads nothing, so both are injectable: {ollama}"
        );
        assert!(
            !ollama.contains("in force"),
            "nothing is in force for a runtime that reads no files: {ollama}"
        );
    }

    #[test]
    fn an_empty_project_says_so_rather_than_producing_an_empty_string() {
        let proj = TempDir::new("none");
        let home = TempDir::new("nonehome");
        let d = discover(proj.path(), home.path());
        assert_eq!(d.summary("claude-code"), "no instruction files found");
    }

    #[test]
    fn injectable_is_not_counted_as_in_force() {
        // Tervin *could* pass the file in, but until a user asks, the agent has not
        // been told anything. Counting it as in force would be the overclaim.
        assert!(!Readership::Injectable.in_force());
        assert!(Readership::Native {
            evidence: "x".to_string()
        }
        .in_force());
        assert!(!Readership::Ignored.in_force());
        assert!(!Readership::Unknown.in_force());
    }

    #[test]
    fn mcp_servers_are_discovered_by_name_from_a_project_mcp_json() {
        let proj = TempDir::new("mcp");
        let home = TempDir::new("mcphome");
        proj.write(
            ".mcp.json",
            r#"{"mcpServers":{"github":{"command":"gh-mcp"},"sentry":{"command":"sentry-mcp"}}}"#,
        );

        let found = discover_mcp(proj.path(), home.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, McpConfigKind::ProjectMcpJson);
        assert_eq!(found[0].servers, vec!["github", "sentry"]);
        assert!(found[0].error.is_none());
    }

    #[test]
    fn a_server_command_and_env_are_never_captured_only_the_name() {
        // An MCP entry routinely holds an API key in `env`. Reading it to render a
        // list of names would put a credential in memory for no reason, and the
        // serialised form would carry it into any log or export.
        let proj = TempDir::new("secret");
        let home = TempDir::new("secrethome");
        proj.write(
            ".mcp.json",
            r#"{"mcpServers":{"paid":{"command":"x","env":{"API_KEY":"sk-do-not-capture"}}}}"#,
        );

        let found = discover_mcp(proj.path(), home.path());
        let json = serde_json::to_string(&found).unwrap();
        assert!(json.contains("paid"), "the name should be reported");
        assert!(
            !json.contains("sk-do-not-capture"),
            "a credential reached the discovery result: {json}"
        );
        assert!(!json.contains("API_KEY"));
    }

    #[test]
    fn claude_json_servers_are_found_under_their_project_key() {
        // `~/.claude.json` nests configuration per project path, so a top-level
        // lookup alone finds nothing and the panel would wrongly say "none".
        let proj = TempDir::new("cj");
        let home = TempDir::new("cjhome");
        home.write(
            ".claude.json",
            r#"{"projects":{"/some/path":{"mcpServers":{"nested-one":{"command":"x"}}}}}"#,
        );

        let found = discover_mcp(proj.path(), home.path());
        let cj = found
            .iter()
            .find(|f| f.kind == McpConfigKind::ClaudeJson)
            .expect("~/.claude.json should be found");
        assert_eq!(cj.servers, vec!["nested-one"]);
    }

    #[test]
    fn codex_toml_uses_the_snake_case_key() {
        // Codex spells it `mcp_servers`, not `mcpServers`. Reusing the JSON key
        // would silently report zero servers for every Codex user.
        let proj = TempDir::new("ct");
        let home = TempDir::new("cthome");
        proj.write(
            ".codex/config.toml",
            "[mcp_servers.docs]\ncommand = \"docs-mcp\"\n",
        );

        let found = discover_mcp(proj.path(), home.path());
        let ct = found
            .iter()
            .find(|f| f.kind == McpConfigKind::CodexToml)
            .expect("the Codex config should be found");
        assert_eq!(ct.servers, vec!["docs"]);
        assert!(ct.error.is_none());
    }

    #[test]
    fn a_malformed_config_is_reported_rather_than_dropped() {
        // A runtime that silently ignores its own broken config leaves a user with
        // no way to find out. Reporting the parse error is the whole value.
        let proj = TempDir::new("bad");
        let home = TempDir::new("badhome");
        proj.write(".mcp.json", "{ this is not json");

        let found = discover_mcp(proj.path(), home.path());
        assert_eq!(found.len(), 1, "a broken file must still be listed");
        assert!(found[0].servers.is_empty());
        let err = found[0].error.as_ref().expect("expected a parse error");
        assert!(err.contains("not valid JSON"), "unhelpful error: {err}");
    }

    #[test]
    fn a_config_with_no_servers_is_listed_with_none_rather_than_as_an_error() {
        let proj = TempDir::new("emptycfg");
        let home = TempDir::new("emptycfghome");
        proj.write(".mcp.json", r#"{"other":true}"#);

        let found = discover_mcp(proj.path(), home.path());
        assert!(found[0].servers.is_empty());
        assert!(
            found[0].error.is_none(),
            "valid JSON without servers is not an error"
        );
    }

    #[test]
    fn adoption_flags_a_name_tervin_already_has() {
        // Accepting a conflicting name overwrites rather than adds, and a user
        // clicking "adopt" deserves to know which of the two it will be.
        let file = McpConfigFile {
            path: PathBuf::from("/p/.mcp.json"),
            kind: McpConfigKind::ProjectMcpJson,
            servers: vec!["github".into(), "sentry".into()],
            error: None,
        };
        let candidates = adoption_candidates(&[file], &["github".to_string()]);
        assert_eq!(candidates.len(), 2);
        let github = candidates.iter().find(|c| c.name == "github").unwrap();
        assert!(github.conflicts);
        let sentry = candidates.iter().find(|c| c.name == "sentry").unwrap();
        assert!(!sentry.conflicts);
        // The source is shown before the user accepts, so it must name the file.
        assert!(sentry.source.contains(".mcp.json"));
    }

    #[test]
    fn a_server_configured_twice_is_offered_once() {
        let a = McpConfigFile {
            path: PathBuf::from("/p/.mcp.json"),
            kind: McpConfigKind::ProjectMcpJson,
            servers: vec!["shared".into()],
            error: None,
        };
        let b = McpConfigFile {
            path: PathBuf::from("/h/.claude.json"),
            kind: McpConfigKind::ClaudeJson,
            servers: vec!["shared".into()],
            error: None,
        };
        let candidates = adoption_candidates(&[a, b], &[]);
        assert_eq!(candidates.len(), 1);
        // Attributed to the first file it was seen in, so the attribution is stable.
        assert!(candidates[0].source.contains(".mcp.json"));
    }

    #[test]
    fn discovery_does_not_read_home_when_home_is_a_temp_dir() {
        // Guards the design decision that `home` is a parameter. If discovery ever
        // reaches for `dirs::home_dir()` internally, this test starts finding the
        // developer's real files and fails.
        let proj = TempDir::new("iso");
        let home = TempDir::new("isohome");
        let d = discover(proj.path(), home.path());
        assert!(
            d.files.is_empty() && d.mcp.is_empty(),
            "discovery reached outside the temporary directories: {d:?}"
        );
    }

    /// Every (kind, runtime) pair, as the implementation answers it.
    ///
    /// Used both to assert against the committed fixture and, with
    /// `TERVIN_WRITE_READERSHIP_FIXTURE=1`, to regenerate it after a deliberate
    /// change. Regenerating is a visible step rather than automatic, so a table
    /// change always shows up in a diff.
    fn matrix() -> serde_json::Value {
        let kinds = [
            ("agents", InstructionKind::Agents),
            ("claude_md", InstructionKind::ClaudeMd),
            ("claude_local", InstructionKind::ClaudeLocal),
            ("cursor_rules", InstructionKind::CursorRules),
            ("copilot_instructions", InstructionKind::CopilotInstructions),
            ("gemini_md", InstructionKind::GeminiMd),
            ("windsurf_rules", InstructionKind::WindsurfRules),
            ("cline_rules", InstructionKind::ClineRules),
        ];
        let runtimes = [
            "claude-code",
            "claude-code-acp",
            "codex",
            "gemini",
            "gemini-acp",
            "cursor-agent",
            "copilot-acp",
            "lmstudio",
            "ollama",
            "vllm",
            "llamacpp",
            "aider",
            "an-agent-tervin-has-never-heard-of",
        ];
        let mut out = serde_json::Map::new();
        for runtime in runtimes {
            let mut per = serde_json::Map::new();
            for (name, kind) in kinds {
                per.insert(
                    name.to_string(),
                    serde_json::to_value(readership(kind, runtime)).unwrap(),
                );
            }
            out.insert(runtime.to_string(), serde_json::Value::Object(per));
        }
        serde_json::Value::Object(out)
    }

    fn fixture_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/lib/readership-matrix.json")
    }

    #[test]
    fn the_readership_table_matches_the_committed_fixture() {
        // The UI needs this table client-side: the panel switches runtime on a click
        // and a round trip per click would make it feel broken. So the table exists
        // twice, and this fixture is what stops the two drifting. The TypeScript side
        // asserts against the same file in `readership.matrix.test.ts`.
        //
        // Changing the table deliberately: run with
        // TERVIN_WRITE_READERSHIP_FIXTURE=1 and commit the regenerated file, which
        // makes the change visible in review rather than implicit.
        let current = matrix();
        let path = fixture_path();

        if std::env::var("TERVIN_WRITE_READERSHIP_FIXTURE").is_ok() {
            fs::write(
                &path,
                serde_json::to_string_pretty(&current).unwrap() + "\n",
            )
            .unwrap();
            eprintln!("wrote {}", path.display());
            return;
        }

        let text = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "could not read {}: {e}. Regenerate with \
                 TERVIN_WRITE_READERSHIP_FIXTURE=1",
                path.display()
            )
        });
        let committed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            committed, current,
            "the readership table and the committed fixture disagree. If the change \
             was intended, regenerate with TERVIN_WRITE_READERSHIP_FIXTURE=1 and \
             update ProjectInstructions.tsx to match."
        );
    }

    /// Run discovery against a real project on this machine.
    ///
    /// Skipped by default because it depends on what is checked out locally. Every
    /// other test here builds its own tree, which proves the logic but not that the
    /// paths match what tools really write. Point this at a repository that has been
    /// used with Claude Code or Codex:
    ///
    /// ```sh
    /// TERVIN_VERIFY_DISCOVERY=~/Projects/some-repo \
    ///   cargo test -p agent-runtime discovery_against_a_real_project -- --nocapture
    /// ```
    #[test]
    fn discovery_against_a_real_project() {
        let Ok(root) = std::env::var("TERVIN_VERIFY_DISCOVERY") else {
            eprintln!("skipped: set TERVIN_VERIFY_DISCOVERY=<path to a real project>");
            return;
        };
        let home = dirs::home_dir().expect("a home directory");
        let d = discover(Path::new(&root), &home);

        eprintln!(
            "\n{} instruction files, truncated={}",
            d.files.len(),
            d.truncated
        );
        for f in &d.files {
            eprintln!(
                "  {:?} {:?} {} bytes  {}",
                f.kind,
                f.scope,
                f.bytes,
                f.path.display()
            );
        }
        for runtime in ["claude-code", "codex", "ollama", "some-unknown-agent"] {
            eprintln!("\n{runtime}: {}", d.summary(runtime));
            for p in d.for_runtime(runtime) {
                eprintln!(
                    "  {:<28} {}",
                    p.file
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    p.readership.summary()
                );
            }
        }
        eprintln!("\n{} MCP config files", d.mcp.len());
        for m in &d.mcp {
            eprintln!(
                "  {:?} {} servers{}  {}",
                m.kind,
                m.servers.len(),
                m.error
                    .as_ref()
                    .map(|e| format!(" ERROR: {e}"))
                    .unwrap_or_default(),
                m.path.display()
            );
        }
        // The one thing that must hold regardless of what is checked out: nothing is
        // reported without a path that exists.
        for f in &d.files {
            assert!(
                f.path.exists(),
                "reported a file that is not there: {:?}",
                f.path
            );
        }
    }

    #[test]
    fn the_verified_against_table_names_the_runtimes_whose_behaviour_is_claimed() {
        // The table asserts things about other people's software. Any runtime
        // reported as Native must appear here, so a version bump has somewhere
        // obvious to be checked.
        let native_runtimes = ["claude-code", "codex"];
        for id in native_runtimes {
            assert!(
                VERIFIED_AGAINST.iter().any(|(r, _)| *r == id),
                "{id} is claimed to read files natively but is not in VERIFIED_AGAINST"
            );
        }
        for (_, version) in VERIFIED_AGAINST {
            assert!(!version.is_empty());
        }
    }
}
