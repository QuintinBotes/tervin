//! Git types shared by the review workspace and the status rail.

use serde::{Deserialize, Serialize};

/// Whether a change is staged, unstaged, or untracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageState {
    Staged,
    Unstaged,
    /// Both an index change and a further worktree change exist for one path.
    Both,
    Untracked,
    Ignored,
    /// A merge conflict: the path needs resolution before anything else.
    Conflicted,
}

/// What happened to a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Unmerged,
}

impl ChangeKind {
    fn from_code(c: char) -> Self {
        match c {
            'A' => Self::Added,
            'M' => Self::Modified,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            'T' => Self::TypeChanged,
            'U' => Self::Unmerged,
            _ => Self::Modified,
        }
    }

    /// Single-letter marker for the changed-file tree.
    pub fn marker(&self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Copied => "C",
            Self::TypeChanged => "T",
            Self::Untracked => "?",
            Self::Unmerged => "!",
        }
    }
}

/// One changed path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStatus {
    pub path: String,
    /// Set for renames and copies.
    pub original_path: Option<String>,
    pub stage: StageState,
    /// Change recorded in the index.
    pub index_change: Option<ChangeKind>,
    /// Change present in the working tree.
    pub worktree_change: Option<ChangeKind>,
}

impl FileStatus {
    /// The change to show in a one-line summary, preferring the worktree.
    pub fn primary_change(&self) -> ChangeKind {
        self.worktree_change
            .or(self.index_change)
            .unwrap_or(ChangeKind::Modified)
    }
}

/// Repository state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoStatus {
    pub root: String,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    /// True when HEAD is not on a branch, which changes what a commit means.
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileStatus>,
    /// True when any tracked file differs from HEAD.
    pub dirty: bool,
    /// True when a merge, rebase, or cherry-pick is in progress.
    pub operation_in_progress: Option<String>,
}

impl RepoStatus {
    pub fn staged_count(&self) -> usize {
        self.files
            .iter()
            .filter(|f| matches!(f.stage, StageState::Staged | StageState::Both))
            .count()
    }

    pub fn unstaged_count(&self) -> usize {
        self.files
            .iter()
            .filter(|f| matches!(f.stage, StageState::Unstaged | StageState::Both))
            .count()
    }

    pub fn conflicted_count(&self) -> usize {
        self.files
            .iter()
            .filter(|f| f.stage == StageState::Conflicted)
            .count()
    }
}

/// One line inside a diff hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
    /// Line number in the old file, absent for additions.
    pub old_lineno: Option<u32>,
    /// Line number in the new file, absent for deletions.
    pub new_lineno: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file`.
    NoNewline,
}

/// A contiguous changed region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// The function or section context git puts after the `@@` marker.
    pub section: Option<String>,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// The exact `@@` header, needed to reconstruct an applyable patch for
    /// hunk-level staging or reverting.
    pub fn header(&self) -> String {
        let mut h = format!(
            "@@ -{},{} +{},{} @@",
            self.old_start, self.old_lines, self.new_start, self.new_lines
        );
        if let Some(section) = &self.section {
            h.push(' ');
            h.push_str(section);
        }
        h
    }
}

/// A file's diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    /// Binary files have no renderable hunks; the UI says so instead of showing
    /// an empty diff.
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    pub added_lines: u32,
    pub removed_lines: u32,
    /// Raw `diff --git` header lines, kept so a patch can be reconstructed
    /// byte-for-byte for hunk-level apply.
    pub raw_header: Vec<String>,
}

impl FileDiff {
    /// Rebuild an applyable patch containing only the selected hunks.
    ///
    /// Used by hunk-level accept and revert. The header must be preserved
    /// verbatim, and later hunks' line offsets are unchanged because `git apply`
    /// tolerates the resulting gaps.
    pub fn patch_for_hunks(&self, indices: &[usize]) -> Option<String> {
        if self.binary {
            return None;
        }
        let mut out = String::new();
        for line in &self.raw_header {
            out.push_str(line);
            out.push('\n');
        }
        let mut any = false;
        for (i, hunk) in self.hunks.iter().enumerate() {
            if !indices.contains(&i) {
                continue;
            }
            any = true;
            out.push_str(&hunk.header());
            out.push('\n');
            for line in &hunk.lines {
                match line.kind {
                    DiffLineKind::Context => out.push(' '),
                    DiffLineKind::Added => out.push('+'),
                    DiffLineKind::Removed => out.push('-'),
                    DiffLineKind::NoNewline => out.push('\\'),
                }
                out.push_str(&line.content);
                out.push('\n');
            }
        }
        if any {
            Some(out)
        } else {
            None
        }
    }
}

/// Which set of changes to diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffMode {
    /// Working tree against the index.
    Unstaged,
    /// Index against HEAD.
    Staged,
    /// Working tree against HEAD — everything uncommitted.
    WorkingTree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub is_head: bool,
    pub upstream: Option<String>,
    pub last_commit_sha: Option<String>,
    pub last_commit_subject: Option<String>,
    pub last_commit_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub is_current: bool,
    pub locked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author_name: String,
    pub author_email: String,
    pub date: String,
    /// True when the commit was not made through Tervin.
    ///
    /// Agent-created and externally-created commits must be visibly distinct
    /// from the user's own.
    pub external: bool,
}

pub(crate) fn change_kind_from_code(c: char) -> ChangeKind {
    ChangeKind::from_code(c)
}
