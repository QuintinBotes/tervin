//! The Tervin Block: one submitted command and everything known about it.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tervin_core::{ArtifactId, BlockId, PaneId, SessionId, ThreadId, Timestamp};

/// Bytes of a Block's output held in the database row before spilling to disk.
pub const MAX_INLINE_OUTPUT: usize = 256 * 1024;

/// Default ceiling on a single Block's spilled output. Scrollback is
/// disk-backed by design, but one runaway process must not fill the disk.
pub const DEFAULT_MAX_SPILL: u64 = 64 * 1024 * 1024;

/// Lifecycle of a Block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockStatus {
    Running,
    Succeeded,
    Failed,
    /// Ended by a signal — for example Ctrl-C, which exits 130.
    Interrupted,
    /// The command ran but Tervin never saw a completion mark, typically because
    /// the shell exited mid-command or integration was disabled part-way.
    Unknown,
}

impl BlockStatus {
    /// Derive status from an exit code, following shell convention: 128+N means
    /// terminated by signal N.
    pub fn from_exit_code(code: Option<i32>) -> Self {
        match code {
            Some(0) => Self::Succeeded,
            Some(c) if (129..=192).contains(&c) => Self::Interrupted,
            Some(_) => Self::Failed,
            None => Self::Unknown,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::Interrupted => "Interrupted",
            Self::Unknown => "Unknown",
        }
    }

    /// Semantic colour role. Success is deliberately unpainted in the list: a
    /// screen where every ordinary command glows green is noise, so only the
    /// small status marker carries the colour.
    pub fn tone(&self) -> &'static str {
        match self {
            Self::Running => "teal",
            Self::Succeeded => "green",
            Self::Failed => "red",
            Self::Interrupted => "amber",
            Self::Unknown => "muted",
        }
    }
}

/// A Block's captured output.
///
/// A PTY delivers one interleaved stream: the child's stdout and stderr share a
/// single file descriptor, which is exactly what makes an interactive terminal
/// work. Tervin therefore does not claim to separate them for shell Blocks —
/// splitting them would mean replacing the PTY with pipes and breaking every
/// program that checks `isatty`. Where a runtime *does* report the streams apart
/// (an agent invoking a subprocess), that distinction is carried on the event
/// instead. Errors and warnings inside this stream are found by parsing, which is
/// what a user actually wants to locate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockOutput {
    /// Head of the output, raw bytes with ANSI intact, kept in the row.
    #[serde(with = "serde_bytes_base64")]
    pub inline: Vec<u8>,
    /// Where the full output lives once it outgrew `inline`.
    pub spill_path: Option<PathBuf>,
    /// Total bytes the command produced, including anything beyond the cap.
    pub total_bytes: u64,
    /// True when output exceeded the spill ceiling and was cut off. Surfaced in
    /// the UI so nobody mistakes a truncated log for the whole story.
    pub truncated: bool,
}

impl BlockOutput {
    /// Whether the full output requires reading the spill file.
    pub fn is_spilled(&self) -> bool {
        self.spill_path.is_some()
    }

    /// Best-effort lossy text of the inline portion, for previews and indexing.
    pub fn inline_text(&self) -> String {
        String::from_utf8_lossy(&self.inline).to_string()
    }
}

/// Base64 for the output blob, so a Block round-trips through JSON to the UI
/// without the cost of a per-byte array.
mod serde_bytes_base64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map_err(serde::de::Error::custom)
    }
}

/// Repository state as it was when the command started.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitContext {
    pub repo_root: Option<String>,
    pub branch: Option<String>,
    pub dirty: Option<bool>,
    pub head_sha: Option<String>,
}

/// A file path found in output, with a source location when one was present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathHit {
    pub path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// True once Tervin has confirmed the path exists on disk, so the UI only
    /// offers to open things that are actually openable.
    pub exists: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedDiagnostic {
    pub severity: tervin_core::events::Severity,
    pub message: String,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// The toolchain the diagnostic came from, when identifiable: `rustc`,
    /// `tsc`, `eslint`, and so on.
    pub source: Option<String>,
}

/// A test summary recovered from output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSummary {
    pub runner: String,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
}

/// Structure Tervin recovered from a Block's output.
///
/// Everything here is best-effort by nature. It drives affordances — open this
/// path, follow this port — and never replaces the raw output, which stays
/// authoritative and always available.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedOutput {
    pub paths: Vec<PathHit>,
    pub urls: Vec<String>,
    /// Local ports the command mentioned listening on.
    pub ports: Vec<u16>,
    pub diagnostics: Vec<ParsedDiagnostic>,
    pub tests: Option<TestSummary>,
    pub error_count: u32,
    pub warning_count: u32,
}

/// One submitted command and its related output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub id: BlockId,
    pub pane_id: PaneId,
    pub session_id: SessionId,
    /// Set when an agent ran this command rather than the user.
    pub thread_id: Option<ThreadId>,

    pub command: String,
    pub cwd: String,
    pub host: String,
    pub shell: Option<String>,
    pub project: Option<String>,

    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub status: BlockStatus,

    pub output: BlockOutput,
    pub git: GitContext,
    pub parsed: ParsedOutput,

    pub tags: Vec<String>,
    pub note: Option<String>,
    pub bookmarked: bool,
    pub artifacts: Vec<ArtifactId>,
}

impl Block {
    pub fn new(
        pane_id: PaneId,
        session_id: SessionId,
        command: impl Into<String>,
        cwd: impl Into<String>,
        host: impl Into<String>,
    ) -> Self {
        Self {
            id: BlockId::new(),
            pane_id,
            session_id,
            thread_id: None,
            command: command.into(),
            cwd: cwd.into(),
            host: host.into(),
            shell: None,
            project: None,
            started_at: tervin_core::now(),
            ended_at: None,
            duration_ms: None,
            exit_code: None,
            status: BlockStatus::Running,
            output: BlockOutput::default(),
            git: GitContext::default(),
            parsed: ParsedOutput::default(),
            tags: Vec::new(),
            note: None,
            bookmarked: false,
            artifacts: Vec::new(),
        }
    }

    /// First line of the command, for collapsed rows and the palette.
    pub fn title(&self) -> &str {
        self.command.lines().next().unwrap_or("").trim()
    }

    /// Whether this Block is worth surfacing when filtering for attention:
    /// failures, and anything the user marked.
    pub fn is_notable(&self) -> bool {
        self.bookmarked || matches!(self.status, BlockStatus::Failed) || self.parsed.error_count > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_terminated_commands_are_interrupted_not_failed() {
        // Ctrl-C exits 130. Presenting that as a plain failure misleads: the
        // command did not go wrong, the user stopped it.
        assert_eq!(
            BlockStatus::from_exit_code(Some(130)),
            BlockStatus::Interrupted
        );
        assert_eq!(
            BlockStatus::from_exit_code(Some(143)),
            BlockStatus::Interrupted
        );
        assert_eq!(BlockStatus::from_exit_code(Some(1)), BlockStatus::Failed);
        assert_eq!(BlockStatus::from_exit_code(Some(0)), BlockStatus::Succeeded);
        assert_eq!(BlockStatus::from_exit_code(None), BlockStatus::Unknown);
    }

    #[test]
    fn exit_code_127_is_a_failure_not_a_signal() {
        // 127 is "command not found"; the signal range starts above it.
        assert_eq!(BlockStatus::from_exit_code(Some(127)), BlockStatus::Failed);
        assert_eq!(BlockStatus::from_exit_code(Some(128)), BlockStatus::Failed);
        assert_eq!(
            BlockStatus::from_exit_code(Some(129)),
            BlockStatus::Interrupted
        );
    }
}

/// A directory Tervin has seen a pane sit in.
///
/// Carries what ranking needs and nothing else, so the score can be computed without a
/// second query or a clock reading per row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentDir {
    pub path: String,
    pub visits: u32,
    /// Hours since it was last used, at the moment it was read.
    pub age_hours: f64,
}

impl RecentDir {
    /// Frecency: how often, discounted by how long ago.
    ///
    /// The shape `z` and `autojump` settled on, and for the same reason — neither half
    /// works alone. Pure recency loses the directory you live in as soon as you glance
    /// anywhere else; a pure count keeps somewhere you abandoned months ago at the top.
    ///
    /// The bands are coarse on purpose. A smooth decay curve invites tuning that nobody
    /// can perceive, while "today, this week, this month, older" is a distinction people
    /// actually make.
    pub fn frecency(&self) -> f64 {
        let weight = if self.age_hours < 24.0 {
            4.0
        } else if self.age_hours < 24.0 * 7.0 {
            2.0
        } else if self.age_hours < 24.0 * 30.0 {
            1.0
        } else {
            0.4
        };
        f64::from(self.visits) * weight
    }
}
