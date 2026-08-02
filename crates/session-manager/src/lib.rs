//! Sessions and connections.
//!
//! Everything a pane can be attached to: a local shell, an SSH host, a tmux or
//! zellij session, a serial device, or a WSL distribution. Each resolves to a
//! [`LaunchSpec`] — a program and arguments — which `terminal-core` runs in a PTY.
//!
//! The design decision that runs through this module: **attach, do not
//! reimplement.** Tervin does not speak the SSH protocol, does not implement the
//! tmux control protocol, and does not manage serial framing. It launches the
//! user's own `ssh`, `tmux`, and `screen`, which means their config, their
//! credential helpers, their agent forwarding, and their keybindings all keep
//! working exactly as they already do. A reimplementation would diverge from
//! those in ways that are hard to see and worse to debug.

pub mod probe;
pub mod ssh;

pub use probe::{probe, Reachability};
pub use ssh::{SshConfig, SshHost};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What a pane is attached to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionKind {
    /// A local shell.
    Shell { profile_id: String },
    /// A remote host from the user's SSH config.
    Ssh { alias: String },
    /// An existing tmux or zellij session.
    Multiplexer {
        program: MultiplexerKind,
        session: String,
    },
    /// A serial device, for embedded work.
    Serial { device: String, baud: u32 },
    /// A WSL distribution on Windows.
    Wsl { distribution: String },
    /// An arbitrary managed command — the Tier 3 agent case.
    Command { program: String, args: Vec<String> },
}

impl SessionKind {
    /// A short label for a tab.
    pub fn label(&self) -> String {
        match self {
            Self::Shell { profile_id } => profile_id.clone(),
            Self::Ssh { alias } => alias.clone(),
            Self::Multiplexer { program, session } => {
                format!("{} · {session}", program.binary())
            }
            Self::Serial { device, .. } => device.rsplit('/').next().unwrap_or(device).to_string(),
            Self::Wsl { distribution } => distribution.clone(),
            Self::Command { program, .. } => {
                program.rsplit('/').next().unwrap_or(program).to_string()
            }
        }
    }

    /// Whether this session reaches beyond the local machine.
    ///
    /// The status rail and the rules engine both need this: a destructive command
    /// on a remote host is not the same action as the same command locally.
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Ssh { .. } | Self::Serial { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiplexerKind {
    Tmux,
    Zellij,
}

impl MultiplexerKind {
    pub fn binary(&self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Zellij => "zellij",
        }
    }
}

/// A resolved program to run in a PTY.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    /// Human-readable description of what is being started, for the UI and for
    /// the audit log.
    pub description: String,
}

/// A configured local shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellProfile {
    pub id: String,
    pub name: String,
    /// Absolute path, so a `PATH` change cannot silently switch shells.
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Whether Tervin's shell-integration hook can report from this shell.
    pub supports_integration: bool,
}

impl ShellProfile {
    pub fn to_launch(&self, cwd: Option<String>) -> LaunchSpec {
        LaunchSpec {
            program: self.program.clone(),
            args: self.args.clone(),
            cwd: cwd.or_else(|| self.cwd.clone()),
            env: self.env.clone(),
            description: format!("{} ({})", self.name, self.program),
        }
    }
}

/// Shells Tervin knows how to launch, in the order they are offered.
///
/// `-l` is used for POSIX shells so the user's real environment, `PATH`, and
/// prompt apply. A terminal that quietly runs a non-login shell produces a
/// different `PATH` from the user's other terminals, which is a confusing bug to
/// chase.
const KNOWN_SHELLS: &[(&str, &str, &[&str], bool)] = &[
    ("zsh", "zsh", &["-l"], true),
    ("bash", "bash", &["-l"], true),
    ("fish", "fish", &["-l"], true),
    ("nu", "nushell", &[], false),
    ("pwsh", "PowerShell", &["-NoLogo"], true),
    ("sh", "sh", &["-l"], true),
];

/// Common install locations, searched in addition to `PATH`.
///
/// Homebrew and MacPorts shells are frequently absent from a GUI app's inherited
/// `PATH`, because that `PATH` comes from `launchd` rather than from a login
/// shell.
const EXTRA_SHELL_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/opt/local/bin",
    "/usr/bin",
    "/bin",
    "/run/current-system/sw/bin",
];

/// Discover the shells installed on this machine.
///
/// `/etc/shells` is consulted first because it is the system's own answer to this
/// question, then known names are probed directly.
pub fn discover_shells() -> Vec<ShellProfile> {
    let mut found: Vec<ShellProfile> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    let add = |path: PathBuf, found: &mut Vec<ShellProfile>, seen: &mut Vec<String>| {
        let resolved = path.display().to_string();
        if seen.contains(&resolved) || !path.is_file() {
            return;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return;
        };
        let Some((id, name, args, integration)) = KNOWN_SHELLS
            .iter()
            .find(|(known, ..)| *known == stem)
            .map(|(id, name, args, integration)| (*id, *name, *args, *integration))
        else {
            return;
        };

        seen.push(resolved.clone());
        found.push(ShellProfile {
            id: id.to_string(),
            name: name.to_string(),
            program: resolved,
            args: args.iter().map(|s| s.to_string()).collect(),
            env: Vec::new(),
            cwd: None,
            supports_integration: integration,
        });
    };

    // The system's own list.
    if let Ok(text) = std::fs::read_to_string("/etc/shells") {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            add(PathBuf::from(line), &mut found, &mut seen);
        }
    }

    // Then PATH and the usual install locations.
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();

    for dir in path_dirs
        .iter()
        .cloned()
        .chain(EXTRA_SHELL_DIRS.iter().map(PathBuf::from))
    {
        for (known, ..) in KNOWN_SHELLS {
            add(dir.join(known), &mut found, &mut seen);
        }
    }

    // The user's own shell first: it is the one they expect.
    if let Ok(current) = std::env::var("SHELL") {
        if let Some(index) = found.iter().position(|s| s.program == current) {
            let profile = found.remove(index);
            found.insert(0, profile);
        }
    }

    found
}

/// The shell a new pane opens with when the user has expressed no preference.
pub fn default_shell() -> ShellProfile {
    let current = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    discover_shells()
        .into_iter()
        .find(|s| s.program == current)
        .unwrap_or(ShellProfile {
            id: "sh".to_string(),
            name: "sh".to_string(),
            program: current,
            args: vec!["-l".to_string()],
            env: Vec::new(),
            cwd: None,
            supports_integration: true,
        })
}

/// A tmux or zellij session that already exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiplexerSession {
    pub program: MultiplexerKind,
    pub name: String,
    /// Windows, panes, or whatever the multiplexer reports; display only.
    pub detail: Option<String>,
    pub attached: bool,
}

impl MultiplexerSession {
    pub fn to_launch(&self) -> LaunchSpec {
        let args = match self.program {
            // `-A` attaches if the session exists and creates it otherwise, which
            // avoids a race between listing and attaching.
            MultiplexerKind::Tmux => vec![
                "new-session".to_string(),
                "-A".to_string(),
                "-s".to_string(),
                self.name.clone(),
            ],
            MultiplexerKind::Zellij => {
                vec![
                    "attach".to_string(),
                    self.name.clone(),
                    "--create".to_string(),
                ]
            }
        };
        LaunchSpec {
            program: self.program.binary().to_string(),
            args,
            cwd: None,
            env: Vec::new(),
            description: format!("Attach to {} session {}", self.program.binary(), self.name),
        }
    }
}

/// List existing multiplexer sessions.
///
/// Absence of the binary is normal and yields an empty list rather than an error.
pub fn list_multiplexer_sessions() -> Vec<MultiplexerSession> {
    let mut out = Vec::new();

    // tmux: `#{session_name}` plus window count and attach state.
    if let Some(text) = run_capture(
        "tmux",
        &[
            "list-sessions",
            "-F",
            "#{session_name}\t#{session_windows}\t#{session_attached}",
        ],
    ) {
        for line in text.lines() {
            let mut fields = line.split('\t');
            let Some(name) = fields.next().filter(|n| !n.is_empty()) else {
                continue;
            };
            let windows = fields.next().unwrap_or("");
            let attached = fields.next().unwrap_or("0") != "0";
            out.push(MultiplexerSession {
                program: MultiplexerKind::Tmux,
                name: name.to_string(),
                detail: (!windows.is_empty()).then(|| format!("{windows} windows")),
                attached,
            });
        }
    }

    // zellij prints a decorated list; take the leading token of each line.
    if let Some(text) = run_capture("zellij", &["list-sessions", "--no-formatting"]) {
        for line in text.lines() {
            let name = line.split_whitespace().next().unwrap_or("");
            if name.is_empty() {
                continue;
            }
            out.push(MultiplexerSession {
                program: MultiplexerKind::Zellij,
                name: name.to_string(),
                detail: None,
                attached: line.contains("current"),
            });
        }
    }

    out
}

/// A serial device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialDevice {
    pub path: String,
    /// A friendlier name where one can be derived from the device node.
    pub label: String,
}

/// Common baud rates, offered in the connection dialog.
pub const BAUD_RATES: [u32; 8] = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

/// Enumerate serial devices.
///
/// On macOS the `cu.*` nodes are listed rather than `tty.*`: opening a `tty.*`
/// node blocks until carrier detect, which would hang the pane on a device that
/// does not assert DCD.
pub fn list_serial_devices() -> Vec<SerialDevice> {
    let mut out = Vec::new();

    let Ok(entries) = std::fs::read_dir("/dev") else {
        return out;
    };

    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        let is_serial = name.starts_with("cu.usb")
            || name.starts_with("cu.SLAB")
            || name.starts_with("cu.wchusb")
            || name.starts_with("ttyUSB")
            || name.starts_with("ttyACM");
        if !is_serial {
            continue;
        }
        out.push(SerialDevice {
            label: name
                .trim_start_matches("cu.")
                .trim_start_matches("tty")
                .to_string(),
            path: entry.path().display().to_string(),
        });
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Build the command that opens a serial device.
///
/// `screen` is used because it is present on macOS and most Linux systems and
/// gives a real PTY-attached session. Where `picocom` exists it is preferred: it
/// exits cleanly on Ctrl-A Ctrl-X, whereas `screen` needs `quit` and strands the
/// device if the pane is closed abruptly.
pub fn serial_launch(device: &str, baud: u32) -> LaunchSpec {
    if which("picocom").is_some() {
        return LaunchSpec {
            program: "picocom".to_string(),
            args: vec!["--baud".to_string(), baud.to_string(), device.to_string()],
            cwd: None,
            env: Vec::new(),
            description: format!("Serial {device} at {baud} baud (picocom)"),
        };
    }
    LaunchSpec {
        program: "screen".to_string(),
        args: vec![device.to_string(), baud.to_string()],
        cwd: None,
        env: Vec::new(),
        description: format!("Serial {device} at {baud} baud (screen)"),
    }
}

/// A WSL distribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WslDistribution {
    pub name: String,
    pub is_default: bool,
}

/// List WSL distributions. Empty everywhere except Windows.
pub fn list_wsl_distributions() -> Vec<WslDistribution> {
    if !cfg!(windows) {
        return Vec::new();
    }
    // `--quiet` prints bare names; the default is marked in the verbose output,
    // so it is queried separately rather than parsed out of decorated text.
    let Some(text) = run_capture("wsl.exe", &["--list", "--quiet"]) else {
        return Vec::new();
    };
    let default = run_capture("wsl.exe", &["--list"])
        .and_then(|verbose| {
            verbose
                .lines()
                .find(|l| l.contains("(Default)"))
                .and_then(|l| l.split_whitespace().next().map(String::from))
        })
        .unwrap_or_default();

    text.lines()
        // wsl.exe emits UTF-16; interior NULs survive the lossy decode.
        .map(|l| l.replace('\u{0}', "").trim().to_string())
        .filter(|l| !l.is_empty())
        .map(|name| WslDistribution {
            is_default: name == default,
            name,
        })
        .collect()
}

/// Resolve a session into something runnable.
pub fn resolve(kind: &SessionKind, shells: &[ShellProfile], cwd: Option<String>) -> LaunchSpec {
    match kind {
        SessionKind::Shell { profile_id } => shells
            .iter()
            .find(|s| &s.id == profile_id)
            .map(|s| s.to_launch(cwd.clone()))
            .unwrap_or_else(|| default_shell().to_launch(cwd.clone())),

        SessionKind::Ssh { alias } => LaunchSpec {
            program: "ssh".to_string(),
            // `-t` forces a TTY so a remote shell is interactive even when the
            // command line would otherwise suppress it.
            args: vec!["-t".to_string(), alias.clone()],
            cwd: None,
            env: Vec::new(),
            description: format!("SSH to {alias}"),
        },

        SessionKind::Multiplexer { program, session } => MultiplexerSession {
            program: *program,
            name: session.clone(),
            detail: None,
            attached: false,
        }
        .to_launch(),

        SessionKind::Serial { device, baud } => serial_launch(device, *baud),

        SessionKind::Wsl { distribution } => LaunchSpec {
            program: "wsl.exe".to_string(),
            args: vec!["--distribution".to_string(), distribution.clone()],
            cwd,
            env: Vec::new(),
            description: format!("WSL · {distribution}"),
        },

        SessionKind::Command { program, args } => LaunchSpec {
            program: program.clone(),
            args: args.clone(),
            cwd,
            env: Vec::new(),
            description: format!("{program} {}", args.join(" ")),
        },
    }
}

/// Everything Tervin can offer to connect to, gathered once.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Connections {
    pub shells: Vec<ShellProfile>,
    pub ssh_hosts: Vec<SshHost>,
    pub ssh_warnings: Vec<String>,
    pub multiplexers: Vec<MultiplexerSession>,
    pub serial: Vec<SerialDevice>,
    pub wsl: Vec<WslDistribution>,
}

impl Connections {
    /// Gather every available connection. Blocking; call off the UI thread.
    pub fn discover() -> Self {
        let ssh = SshConfig::load();
        Self {
            shells: discover_shells(),
            ssh_hosts: ssh.hosts,
            ssh_warnings: ssh.warnings,
            multiplexers: list_multiplexer_sessions(),
            serial: list_serial_devices(),
            wsl: list_wsl_distributions(),
        }
    }
}

/// Run a command and capture stdout, or `None` if it is missing or fails.
fn run_capture(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn which(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_at_least_the_current_shell() {
        let shells = discover_shells();
        assert!(!shells.is_empty(), "no shells discovered");
        if let Ok(current) = std::env::var("SHELL") {
            // The user's own shell is offered first.
            if shells.iter().any(|s| s.program == current) {
                assert_eq!(shells[0].program, current);
            }
        }
    }

    #[test]
    fn discovered_shells_are_absolute_paths() {
        // A bare name would let a PATH change silently switch shells.
        for shell in discover_shells() {
            assert!(
                shell.program.starts_with('/'),
                "{} is not absolute",
                shell.program
            );
        }
    }

    #[test]
    fn posix_shells_launch_as_login_shells() {
        // Otherwise Tervin's PATH differs from every other terminal the user has.
        let shells = discover_shells();
        for shell in shells.iter().filter(|s| s.id != "nu" && s.id != "pwsh") {
            assert!(
                shell.args.iter().any(|a| a == "-l"),
                "{} should be a login shell",
                shell.id
            );
        }
    }

    #[test]
    fn no_duplicate_shells_from_overlapping_sources() {
        // /etc/shells, PATH, and the extra directories overlap heavily.
        let shells = discover_shells();
        let mut paths: Vec<&str> = shells.iter().map(|s| s.program.as_str()).collect();
        let before = paths.len();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(before, paths.len(), "duplicate shell entries");
    }

    #[test]
    fn ssh_sessions_force_a_tty() {
        let spec = resolve(
            &SessionKind::Ssh {
                alias: "bastion".to_string(),
            },
            &[],
            None,
        );
        assert_eq!(spec.program, "ssh");
        assert!(spec.args.contains(&"-t".to_string()));
        assert!(spec.args.contains(&"bastion".to_string()));
    }

    #[test]
    fn tmux_attach_creates_the_session_if_it_is_gone() {
        // Between listing and attaching a session can disappear; `-A` avoids the
        // race rather than erroring.
        let spec = MultiplexerSession {
            program: MultiplexerKind::Tmux,
            name: "work".to_string(),
            detail: None,
            attached: false,
        }
        .to_launch();
        assert_eq!(spec.program, "tmux");
        assert!(spec.args.contains(&"-A".to_string()));
        assert!(spec.args.contains(&"work".to_string()));
    }

    #[test]
    fn zellij_attach_creates_the_session_if_it_is_gone() {
        let spec = MultiplexerSession {
            program: MultiplexerKind::Zellij,
            name: "dev".to_string(),
            detail: None,
            attached: false,
        }
        .to_launch();
        assert_eq!(spec.program, "zellij");
        assert!(spec.args.contains(&"--create".to_string()));
    }

    #[test]
    fn serial_launch_names_a_real_program_and_baud() {
        let spec = serial_launch("/dev/cu.usbserial-1420", 115200);
        assert!(matches!(spec.program.as_str(), "screen" | "picocom"));
        assert!(spec.args.iter().any(|a| a.contains("usbserial")));
        assert!(spec.args.iter().any(|a| a == "115200"));
        assert!(spec.description.contains("115200"));
    }

    #[test]
    fn serial_enumeration_never_lists_blocking_tty_nodes() {
        // Opening /dev/tty.* blocks until carrier detect, which would hang a pane.
        for device in list_serial_devices() {
            assert!(
                !device.path.contains("/tty."),
                "{} would block on open",
                device.path
            );
        }
    }

    #[test]
    fn remote_sessions_are_marked_remote() {
        // The rules engine treats a destructive command differently off-machine.
        assert!(SessionKind::Ssh {
            alias: "prod".into()
        }
        .is_remote());
        assert!(SessionKind::Serial {
            device: "/dev/cu.x".into(),
            baud: 9600
        }
        .is_remote());
        assert!(!SessionKind::Shell {
            profile_id: "zsh".into()
        }
        .is_remote());
    }

    #[test]
    fn session_labels_are_short_enough_for_a_tab() {
        let cases = [
            SessionKind::Ssh {
                alias: "bastion".into(),
            },
            SessionKind::Serial {
                device: "/dev/cu.usbserial-1420".into(),
                baud: 9600,
            },
            SessionKind::Command {
                program: "/opt/homebrew/bin/aider".into(),
                args: vec!["--model".into()],
            },
        ];
        for kind in cases {
            let label = kind.label();
            assert!(!label.is_empty());
            assert!(!label.contains('/'), "{label} still contains a path");
        }
    }

    #[test]
    fn an_unknown_shell_profile_falls_back_rather_than_failing() {
        let spec = resolve(
            &SessionKind::Shell {
                profile_id: "does-not-exist".to_string(),
            },
            &[],
            Some("/tmp".to_string()),
        );
        assert!(!spec.program.is_empty());
        assert_eq!(spec.cwd.as_deref(), Some("/tmp"));
    }

    #[test]
    fn wsl_is_empty_off_windows() {
        if !cfg!(windows) {
            assert!(list_wsl_distributions().is_empty());
        }
    }

    #[test]
    fn discovery_does_not_error_on_a_bare_machine() {
        // Missing tmux, zellij, serial devices, and ssh config are all normal.
        let connections = Connections::discover();
        assert!(!connections.shells.is_empty());
    }
}
