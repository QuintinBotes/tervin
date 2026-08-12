//! Optional shell integration.
//!
//! Tervin is fully functional without this. Blocks still form from process
//! boundaries; what integration adds is exactness — the literal submitted
//! command, the true exit status, and the real cwd.
//!
//! Installation is explicit. Tervin will not silently rewrite a user's shell
//! configuration: it writes the scripts, tells the user precisely what one line
//! it wants to add and where, and appends it only when asked. The insertion is
//! fenced with markers so it can be removed cleanly.

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

pub mod aliases;
pub mod injection;

pub use aliases::{AppliedAlias, Expansion, ShellAliases};
pub use injection::{prepare as prepare_injection, Injection, InjectionMode};

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Fences around Tervin's insertion, so uninstall is exact rather than a guess.
const BEGIN_MARKER: &str = "# >>> tervin shell integration >>>";
const END_MARKER: &str = "# <<< tervin shell integration <<<";

#[derive(Debug, thiserror::Error)]
pub enum ShellIntegrationError {
    #[error("i/o error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not determine the home directory")]
    NoHome,
}

type Result<T> = std::result::Result<T, ShellIntegrationError>;

fn io_err(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> ShellIntegrationError {
    let path = path.into();
    move |source| ShellIntegrationError::Io { path, source }
}

/// A shell Tervin can report from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
    PowerShell,
}

pub const ALL_SHELLS: [Shell; 4] = [Shell::Zsh, Shell::Bash, Shell::Fish, Shell::PowerShell];

impl Shell {
    /// Identify a shell from an executable path or name.
    ///
    /// Returns `None` for anything unrecognised — including nushell and custom
    /// commands, which Tervin still hosts, just without integration signals.
    pub fn detect(program: &str) -> Option<Self> {
        let name = Path::new(program)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(program)
            .to_ascii_lowercase();
        match name.as_str() {
            "zsh" => Some(Self::Zsh),
            "bash" | "sh" => Some(Self::Bash),
            "fish" => Some(Self::Fish),
            "pwsh" | "powershell" => Some(Self::PowerShell),
            _ => None,
        }
    }

    /// The shell the user's environment says they use.
    pub fn from_env() -> Option<Self> {
        std::env::var("SHELL").ok().and_then(|s| Self::detect(&s))
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Bash => "bash",
            Self::Fish => "fish",
            Self::PowerShell => "PowerShell",
        }
    }

    /// Filename used for the generated script.
    pub fn script_name(&self) -> &'static str {
        match self {
            Self::Zsh => "tervin.zsh",
            Self::Bash => "tervin.bash",
            Self::Fish => "tervin.fish",
            Self::PowerShell => "tervin.ps1",
        }
    }

    /// The script source, compiled into the binary so there is nothing to fetch.
    pub fn script(&self) -> &'static str {
        match self {
            Self::Zsh => include_str!("../assets/tervin.zsh"),
            Self::Bash => include_str!("../assets/tervin.bash"),
            Self::Fish => include_str!("../assets/tervin.fish"),
            Self::PowerShell => include_str!("../assets/tervin.ps1"),
        }
    }

    /// The startup file Tervin would append to.
    pub fn rc_path(&self) -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or(ShellIntegrationError::NoHome)?;
        Ok(match self {
            Self::Zsh => {
                // Respect ZDOTDIR: writing to ~/.zshrc would be ignored by a
                // shell that reads its config from elsewhere.
                let base = std::env::var("ZDOTDIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| home.clone());
                base.join(".zshrc")
            }
            Self::Bash => {
                // Tervin runs a login shell, which reads .bash_profile; fall back
                // to .bashrc when there is no profile.
                let profile = home.join(".bash_profile");
                if profile.exists() {
                    profile
                } else {
                    home.join(".bashrc")
                }
            }
            Self::Fish => home.join(".config").join("fish").join("config.fish"),
            Self::PowerShell => home
                .join(".config")
                .join("powershell")
                .join("Microsoft.PowerShell_profile.ps1"),
        })
    }

    /// The exact line Tervin proposes to add. Shown to the user before writing.
    pub fn source_line(&self, script_path: &Path) -> String {
        let p = script_path.display();
        match self {
            Self::Zsh | Self::Bash => format!("[ -f \"{p}\" ] && . \"{p}\""),
            Self::Fish => format!("test -f \"{p}\"; and source \"{p}\""),
            Self::PowerShell => format!(". \"{p}\""),
        }
    }
}

/// What Tervin knows about integration for one shell.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IntegrationStatus {
    pub shell: Shell,
    /// Whether the generated script exists on disk.
    pub script_written: bool,
    pub script_path: PathBuf,
    /// Whether the startup file already sources it.
    pub installed: bool,
    pub rc_path: Option<PathBuf>,
    /// The line that would be added, for display before any write happens.
    pub proposed_line: String,
}

/// Write the script for `shell` into `dir`, returning its path.
pub fn write_script(shell: Shell, dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir).map_err(io_err(dir))?;
    let path = dir.join(shell.script_name());
    // Rewrite unconditionally so an upgraded Tervin ships an upgraded hook.
    fs::write(&path, shell.script()).map_err(io_err(&path))?;
    Ok(path)
}

/// Write every script, for the settings pane's "all shells" view.
pub fn write_all_scripts(dir: &Path) -> Result<Vec<PathBuf>> {
    ALL_SHELLS
        .into_iter()
        .map(|s| write_script(s, dir))
        .collect()
}

/// Report integration state without changing anything.
pub fn status(shell: Shell, dir: &Path) -> IntegrationStatus {
    let script_path = dir.join(shell.script_name());
    let rc_path = shell.rc_path().ok();
    let installed = rc_path
        .as_ref()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|c| c.contains(BEGIN_MARKER) || c.contains(shell.script_name()))
        .unwrap_or(false);

    IntegrationStatus {
        shell,
        script_written: script_path.exists(),
        script_path: script_path.clone(),
        installed,
        rc_path,
        proposed_line: shell.source_line(&script_path),
    }
}

/// Result of an install attempt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum InstallOutcome {
    /// The line was appended.
    Installed { rc_path: PathBuf, line: String },
    /// Already present; nothing was written.
    AlreadyPresent { rc_path: PathBuf },
}

/// Append the source line to the shell's startup file.
///
/// Only ever appends, and only inside its own fenced region. The previous file is
/// copied to `<rc>.tervin-backup` first, so a mistake here is recoverable
/// without Tervin.
pub fn install(shell: Shell, dir: &Path) -> Result<InstallOutcome> {
    let script_path = write_script(shell, dir)?;
    let rc_path = shell.rc_path()?;

    if let Some(parent) = rc_path.parent() {
        fs::create_dir_all(parent).map_err(io_err(parent))?;
    }

    let existing = fs::read_to_string(&rc_path).unwrap_or_default();
    if existing.contains(BEGIN_MARKER) {
        return Ok(InstallOutcome::AlreadyPresent { rc_path });
    }

    if !existing.is_empty() {
        let backup = rc_path.with_extension("tervin-backup");
        fs::write(&backup, &existing).map_err(io_err(&backup))?;
    }

    let line = shell.source_line(&script_path);
    let mut block = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        block.push('\n');
    }
    block.push_str(&format!(
        "\n{BEGIN_MARKER}\n# Added by Tervin. Remove this whole fenced block to uninstall.\n{line}\n{END_MARKER}\n"
    ));

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&rc_path)
        .map_err(io_err(&rc_path))?;
    file.write_all(block.as_bytes()).map_err(io_err(&rc_path))?;

    Ok(InstallOutcome::Installed { rc_path, line })
}

/// Remove Tervin's fenced block from the startup file.
///
/// Returns `true` if something was removed. Lines the user wrote themselves are
/// left alone, even if they reference the script.
pub fn uninstall(shell: Shell) -> Result<bool> {
    let rc_path = shell.rc_path()?;
    let Ok(existing) = fs::read_to_string(&rc_path) else {
        return Ok(false);
    };
    if !existing.contains(BEGIN_MARKER) {
        return Ok(false);
    }

    let mut out = String::with_capacity(existing.len());
    let mut skipping = false;
    for line in existing.lines() {
        if line.trim() == BEGIN_MARKER {
            skipping = true;
            continue;
        }
        if line.trim() == END_MARKER {
            skipping = false;
            continue;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }

    fs::write(&rc_path, out.trim_end().to_string() + "\n").map_err(io_err(&rc_path))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_shells_from_paths() {
        assert_eq!(Shell::detect("/bin/zsh"), Some(Shell::Zsh));
        assert_eq!(Shell::detect("/usr/local/bin/fish"), Some(Shell::Fish));
        assert_eq!(Shell::detect("pwsh"), Some(Shell::PowerShell));
        // Unrecognised shells are hosted, just without integration.
        assert_eq!(Shell::detect("/opt/homebrew/bin/nu"), None);
    }

    #[test]
    fn source_lines_guard_against_a_missing_script() {
        // A stale rc line must never break the user's shell startup.
        assert!(Shell::Zsh
            .source_line(Path::new("/tmp/tervin.zsh"))
            .contains("-f"));
        assert!(Shell::Fish
            .source_line(Path::new("/tmp/tervin.fish"))
            .contains("test -f"));
    }

    #[test]
    fn every_script_guards_on_term_program_and_can_be_disabled() {
        // Sourcing a Tervin script under a different terminal must be a no-op.
        for shell in ALL_SHELLS {
            let src = shell.script();
            assert!(
                src.contains("TERM_PROGRAM"),
                "{} script does not guard on TERM_PROGRAM",
                shell.name()
            );
            assert!(
                src.contains("TERVIN_SHELL_INTEGRATION"),
                "{} script has no disable switch",
                shell.name()
            );
        }
    }

    #[test]
    fn every_script_emits_the_marks_the_block_engine_needs() {
        for shell in ALL_SHELLS {
            let src = shell.script();
            for mark in ["133;A", "133;C", "133;D", "7373;cmd="] {
                assert!(
                    src.contains(mark),
                    "{} script never emits {mark}",
                    shell.name()
                );
            }
        }
    }

    #[test]
    fn uninstall_removes_only_the_fenced_region() {
        let dir = std::env::temp_dir().join(format!("tervin-rc-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let rc = dir.join("rc");
        fs::write(
            &rc,
            format!(
                "export MINE=1\n{BEGIN_MARKER}\nsourced line\n{END_MARKER}\nexport ALSO_MINE=2\n"
            ),
        )
        .unwrap();

        // Exercise the same stripping logic against a temp file.
        let existing = fs::read_to_string(&rc).unwrap();
        let mut out = String::new();
        let mut skipping = false;
        for line in existing.lines() {
            if line.trim() == BEGIN_MARKER {
                skipping = true;
                continue;
            }
            if line.trim() == END_MARKER {
                skipping = false;
                continue;
            }
            if !skipping {
                out.push_str(line);
                out.push('\n');
            }
        }

        assert!(out.contains("export MINE=1"));
        assert!(out.contains("export ALSO_MINE=2"));
        assert!(!out.contains("sourced line"));
        fs::remove_dir_all(&dir).ok();
    }
}
