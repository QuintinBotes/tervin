//! Git status, diffs, branches, and worktrees.
//!
//! Implemented over the `git` CLI rather than a linked library. That is a
//! deliberate trade: it inherits the user's own git configuration, credential
//! helpers, hooks, aliases, signing setup, and `includeIf` rules exactly, so what
//! Tervin reports always matches what the user's own `git` reports. A linked
//! library would diverge from that in ways that are hard to see and worse to
//! debug.
//!
//! Machine-readable formats (`--porcelain=v2 -z`) are used everywhere so paths
//! containing spaces, quotes, or newlines parse exactly.

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

pub mod diff;
pub mod model;

pub use diff::parse_unified_diff;
pub use model::{
    Branch, ChangeKind, Commit, DiffLine, DiffLineKind, DiffMode, FileDiff, FileStatus, Hunk,
    RepoStatus, StageState, Worktree,
};

use model::change_kind_from_code;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("`git` was not found on PATH")]
    GitMissing,
    #[error("not a git repository: {0}")]
    NotARepository(String),
    #[error("git {args:?} failed: {stderr}")]
    CommandFailed { args: Vec<String>, stderr: String },
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, GitError>;

/// Runs git commands against a repository.
///
/// Every method is blocking. Callers on a UI path must move these onto a
/// blocking thread — a `git status` on a cold, large repository is slow enough to
/// drop frames.
#[derive(Debug, Clone, Default)]
pub struct GitService;

impl GitService {
    pub fn new() -> Self {
        Self
    }

    /// Whether a usable `git` exists, for capability reporting.
    pub fn is_available(&self) -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let output = Command::new("git")
            // Never prompt: a hung credential prompt inside a background status
            // refresh would silently wedge the panel.
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .current_dir(cwd)
            .args(args)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    GitError::GitMissing
                } else {
                    GitError::Io(e)
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.contains("not a git repository") {
                return Err(GitError::NotARepository(cwd.display().to_string()));
            }
            return Err(GitError::CommandFailed {
                args: args.iter().map(|s| s.to_string()).collect(),
                stderr,
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// The repository root containing `path`, if any.
    pub fn repo_root(&self, path: &Path) -> Option<PathBuf> {
        let out = self
            .run(path, &["rev-parse", "--show-toplevel"])
            .ok()?
            .trim()
            .to_string();
        if out.is_empty() {
            None
        } else {
            Some(PathBuf::from(out))
        }
    }

    /// Full repository status.
    pub fn status(&self, root: &Path) -> Result<RepoStatus> {
        let raw = self.run(
            root,
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "--untracked-files=normal",
                "-z",
            ],
        )?;

        let mut status = RepoStatus {
            root: root.display().to_string(),
            ..Default::default()
        };

        // Records are NUL-separated; a rename record is followed by a second
        // NUL-terminated field holding the original path.
        let mut records = raw.split('\0').peekable();
        while let Some(record) = records.next() {
            if record.is_empty() {
                continue;
            }

            if let Some(header) = record.strip_prefix("# ") {
                parse_branch_header(header, &mut status);
                continue;
            }

            let mut chars = record.chars();
            let tag = chars.next().unwrap_or(' ');
            let rest = chars.as_str().trim_start();

            match tag {
                '1' => {
                    if let Some(file) = parse_ordinary_entry(rest) {
                        status.files.push(file);
                    }
                }
                '2' => {
                    // Rename or copy: the original path is the next record.
                    let original = records.next().map(|s| s.to_string());
                    if let Some(mut file) = parse_ordinary_entry(rest) {
                        file.original_path = original;
                        status.files.push(file);
                    }
                }
                'u' => {
                    if let Some(path) = rest.split(' ').nth(8) {
                        status.files.push(FileStatus {
                            path: path.to_string(),
                            original_path: None,
                            stage: StageState::Conflicted,
                            index_change: Some(ChangeKind::Unmerged),
                            worktree_change: Some(ChangeKind::Unmerged),
                        });
                    }
                }
                '?' => status.files.push(FileStatus {
                    path: rest.to_string(),
                    original_path: None,
                    stage: StageState::Untracked,
                    index_change: None,
                    worktree_change: Some(ChangeKind::Untracked),
                }),
                '!' => status.files.push(FileStatus {
                    path: rest.to_string(),
                    original_path: None,
                    stage: StageState::Ignored,
                    index_change: None,
                    worktree_change: None,
                }),
                _ => {}
            }
        }

        // Dirty means tracked changes exist. Untracked files alone are not
        // "dirty" — reporting them as such makes the indicator useless in repos
        // with build output lying around.
        status.dirty = status
            .files
            .iter()
            .any(|f| !matches!(f.stage, StageState::Untracked | StageState::Ignored));

        status.operation_in_progress = self.detect_operation(root);
        Ok(status)
    }

    /// Detect a merge, rebase, cherry-pick, bisect, or revert in progress.
    ///
    /// Surfaced because a commit means something different mid-rebase, and a user
    /// must not be told "3 changed files" without that context.
    fn detect_operation(&self, root: &Path) -> Option<String> {
        let git_dir = self
            .run(root, &["rev-parse", "--absolute-git-dir"])
            .ok()
            .map(|s| PathBuf::from(s.trim()))?;

        let checks: [(&str, &str); 6] = [
            ("rebase-merge", "Rebase in progress"),
            ("rebase-apply", "Rebase in progress"),
            ("MERGE_HEAD", "Merge in progress"),
            ("CHERRY_PICK_HEAD", "Cherry-pick in progress"),
            ("REVERT_HEAD", "Revert in progress"),
            ("BISECT_LOG", "Bisect in progress"),
        ];
        checks
            .iter()
            .find(|(marker, _)| git_dir.join(marker).exists())
            .map(|(_, label)| label.to_string())
    }

    /// Diff for the requested change set.
    pub fn diff(&self, root: &Path, mode: DiffMode) -> Result<Vec<FileDiff>> {
        let mut args = vec!["diff", "--no-color", "--no-ext-diff", "--find-renames"];
        match mode {
            DiffMode::Unstaged => {}
            DiffMode::Staged => args.push("--cached"),
            DiffMode::WorkingTree => args.push("HEAD"),
        }
        // A repository with no commits has no HEAD to diff against.
        let raw = match self.run(root, &args) {
            Ok(raw) => raw,
            Err(GitError::CommandFailed { stderr, .. })
                if stderr.contains("unknown revision")
                    || stderr.contains("ambiguous argument 'HEAD'") =>
            {
                self.run(root, &["diff", "--no-color", "--no-ext-diff"])?
            }
            Err(e) => return Err(e),
        };
        Ok(parse_unified_diff(&raw))
    }

    /// Diff for one path only, which is what the review pane fetches on demand.
    pub fn diff_file(&self, root: &Path, mode: DiffMode, path: &str) -> Result<Option<FileDiff>> {
        let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
        match mode {
            DiffMode::Unstaged => {}
            DiffMode::Staged => args.push("--cached"),
            DiffMode::WorkingTree => args.push("HEAD"),
        }
        args.push("--");
        args.push(path);
        let raw = self.run(root, &args)?;
        Ok(parse_unified_diff(&raw).into_iter().next())
    }

    /// Untracked files have no diff against the index, so their content is shown
    /// as an all-added diff instead of an empty one.
    pub fn diff_untracked(&self, root: &Path, path: &str) -> Result<Option<FileDiff>> {
        // `--no-index` exits 1 when the files differ, which is the normal case
        // here, so a non-zero status is not an error.
        let output = Command::new("git")
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(root)
            .args([
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-index",
                "--",
                "/dev/null",
                path,
            ])
            .output()?;
        let raw = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(parse_unified_diff(&raw).into_iter().next())
    }

    pub fn branches(&self, root: &Path) -> Result<Vec<Branch>> {
        let raw = self.run(
            root,
            &[
                "for-each-ref",
                "--format=%(refname:short)%09%(HEAD)%09%(upstream:short)%09%(objectname)%09%(committerdate:iso8601)%09%(contents:subject)",
                "refs/heads",
            ],
        )?;

        Ok(raw
            .lines()
            .filter_map(|line| {
                let mut f = line.split('\t');
                let name = f.next()?.to_string();
                let is_head = f.next().unwrap_or("") == "*";
                let upstream = f.next().unwrap_or("");
                let sha = f.next().unwrap_or("");
                let date = f.next().unwrap_or("");
                let subject = f.next().unwrap_or("");
                Some(Branch {
                    name,
                    is_head,
                    upstream: none_if_empty(upstream),
                    last_commit_sha: none_if_empty(sha),
                    last_commit_subject: none_if_empty(subject),
                    last_commit_date: none_if_empty(date),
                })
            })
            .collect())
    }

    pub fn worktrees(&self, root: &Path) -> Result<Vec<Worktree>> {
        let raw = self.run(root, &["worktree", "list", "--porcelain"])?;
        let current = self.repo_root(root).map(|p| p.display().to_string());

        let mut out = Vec::new();
        let mut path: Option<String> = None;
        let mut head: Option<String> = None;
        let mut branch: Option<String> = None;
        let mut locked = false;

        fn flush(
            current: &Option<String>,
            path: &mut Option<String>,
            head: &mut Option<String>,
            branch: &mut Option<String>,
            locked: &mut bool,
            out: &mut Vec<Worktree>,
        ) {
            if let Some(p) = path.take() {
                let is_current = current.as_deref() == Some(p.as_str());
                out.push(Worktree {
                    path: p,
                    branch: branch.take(),
                    head_sha: head.take(),
                    is_current,
                    locked: *locked,
                });
            }
            *locked = false;
        }

        for line in raw.lines() {
            if let Some(rest) = line.strip_prefix("worktree ") {
                flush(
                    &current,
                    &mut path,
                    &mut head,
                    &mut branch,
                    &mut locked,
                    &mut out,
                );
                path = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("HEAD ") {
                head = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("branch ") {
                branch = Some(rest.trim_start_matches("refs/heads/").to_string());
            } else if line.starts_with("locked") {
                locked = true;
            }
        }
        flush(
            &current,
            &mut path,
            &mut head,
            &mut branch,
            &mut locked,
            &mut out,
        );
        Ok(out)
    }

    /// Recent commits. `tervin_marker` identifies commits Tervin made itself, so
    /// external and agent-created commits can be shown as such.
    pub fn log(
        &self,
        root: &Path,
        limit: usize,
        tervin_marker: Option<&str>,
    ) -> Result<Vec<Commit>> {
        let raw = self.run(
            root,
            &[
                "log",
                &format!("-{limit}"),
                "--format=%H%x09%h%x09%an%x09%ae%x09%aI%x09%s%x09%b%x1e",
            ],
        )?;

        Ok(raw
            .split('\u{1e}')
            .filter(|r| !r.trim().is_empty())
            .filter_map(|record| {
                let mut f = record.trim_start_matches('\n').split('\t');
                let sha = f.next()?.to_string();
                let short_sha = f.next()?.to_string();
                let author_name = f.next().unwrap_or("").to_string();
                let author_email = f.next().unwrap_or("").to_string();
                let date = f.next().unwrap_or("").to_string();
                let subject = f.next().unwrap_or("").to_string();
                let body = f.next().unwrap_or("");
                let external = match tervin_marker {
                    Some(marker) => !body.contains(marker) && !subject.contains(marker),
                    None => false,
                };
                Some(Commit {
                    sha,
                    short_sha,
                    subject,
                    author_name,
                    author_email,
                    date,
                    external,
                })
            })
            .collect())
    }

    pub fn stage(&self, root: &Path, paths: &[String]) -> Result<()> {
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(|s| s.as_str()));
        self.run(root, &args)?;
        Ok(())
    }

    pub fn unstage(&self, root: &Path, paths: &[String]) -> Result<()> {
        let mut args = vec!["restore", "--staged", "--"];
        args.extend(paths.iter().map(|s| s.as_str()));
        self.run(root, &args)?;
        Ok(())
    }

    /// Apply a patch, used for hunk-level staging and reverting.
    ///
    /// Applying always goes through `git apply` so its safety checks and
    /// whitespace handling apply, rather than Tervin editing files itself.
    pub fn apply_patch(&self, root: &Path, patch: &str, reverse: bool, cached: bool) -> Result<()> {
        use std::io::Write as _;
        use std::process::Stdio;

        let mut args: Vec<&str> = vec!["apply"];
        if reverse {
            args.push("--reverse");
        }
        if cached {
            args.push("--cached");
        }

        let mut child = Command::new("git")
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(root)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    GitError::GitMissing
                } else {
                    GitError::Io(e)
                }
            })?;

        // `.stdin(Stdio::piped())` is set on the builder above, and `spawn` fills
        // `child.stdin` in exactly that case. Nothing has taken it since: `child` is
        // the previous statement's local and has not left this function.
        #[allow(
            clippy::expect_used,
            reason = "`.stdin(Stdio::piped())` is on this builder"
        )]
        let stdin = child.stdin.as_mut().expect("stdin was piped");
        stdin.write_all(patch.as_bytes())?;

        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(GitError::CommandFailed {
                args: args.iter().map(|s| s.to_string()).collect(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(())
    }

    /// Detect a browser URL for the repository's remote, for pull-request links.
    pub fn remote_web_url(&self, root: &Path) -> Option<String> {
        let raw = self.run(root, &["remote", "get-url", "origin"]).ok()?;
        normalise_remote_url(raw.trim())
    }
}

fn none_if_empty(s: &str) -> Option<String> {
    if s.trim().is_empty() {
        None
    } else {
        Some(s.trim().to_string())
    }
}

/// `# branch.oid …`, `# branch.head …`, `# branch.upstream …`, `# branch.ab …`
fn parse_branch_header(header: &str, status: &mut RepoStatus) {
    if let Some(rest) = header.strip_prefix("branch.oid ") {
        if rest != "(initial)" {
            status.head_sha = Some(rest.trim().to_string());
        }
    } else if let Some(rest) = header.strip_prefix("branch.head ") {
        let name = rest.trim();
        if name == "(detached)" {
            status.detached = true;
        } else {
            status.branch = Some(name.to_string());
        }
    } else if let Some(rest) = header.strip_prefix("branch.upstream ") {
        status.upstream = none_if_empty(rest);
    } else if let Some(rest) = header.strip_prefix("branch.ab ") {
        for token in rest.split_whitespace() {
            if let Some(v) = token.strip_prefix('+') {
                status.ahead = v.parse().unwrap_or(0);
            } else if let Some(v) = token.strip_prefix('-') {
                status.behind = v.parse().unwrap_or(0);
            }
        }
    }
}

/// Porcelain v2 ordinary/rename entry:
/// `<XY> <sub> <mH> <mI> <mW> <hH> <hI> [<X><score> ]<path>`
fn parse_ordinary_entry(rest: &str) -> Option<FileStatus> {
    let mut fields = rest.split(' ');
    let xy = fields.next()?;
    let x = xy.chars().next()?;
    let y = xy.chars().nth(1)?;

    // Skip <sub> <mH> <mI> <mW> <hH> <hI>.
    for _ in 0..6 {
        fields.next()?;
    }

    let remainder: Vec<&str> = fields.collect();
    let mut remainder = remainder.join(" ");

    // Rename and copy entries carry a score field such as `R100` before the path.
    if remainder.starts_with('R') || remainder.starts_with('C') {
        if let Some((score, path)) = remainder.split_once(' ') {
            if score.len() > 1 && score[1..].chars().all(|c| c.is_ascii_digit()) {
                remainder = path.to_string();
            }
        }
    }

    if remainder.is_empty() {
        return None;
    }

    // '.' means no change on that side.
    let index_change = (x != '.').then(|| change_kind_from_code(x));
    let worktree_change = (y != '.').then(|| change_kind_from_code(y));

    let stage = match (x != '.', y != '.') {
        (true, true) => StageState::Both,
        (true, false) => StageState::Staged,
        _ => StageState::Unstaged,
    };

    Some(FileStatus {
        path: remainder,
        original_path: None,
        stage,
        index_change,
        worktree_change,
    })
}

/// Turn an SSH or git remote into a browsable https URL.
fn normalise_remote_url(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches(".git");
    if url.is_empty() {
        return None;
    }
    if let Some(rest) = url.strip_prefix("git@") {
        // git@github.com:owner/repo
        let rest = rest.replacen(':', "/", 1);
        return Some(format!("https://{rest}"));
    }
    if let Some(rest) = url.strip_prefix("ssh://git@") {
        return Some(format!("https://{rest}"));
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return Some(url.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway repository with a deterministic identity, so tests never
    /// depend on the developer's own git configuration.
    struct TempRepo {
        dir: tempfile::TempDir,
    }

    impl TempRepo {
        fn new() -> Self {
            let repo = Self {
                dir: tempfile::tempdir().unwrap(),
            };
            repo.git(&["init", "--initial-branch=main"]);
            repo.git(&["config", "user.email", "test@tervin.local"]);
            repo.git(&["config", "user.name", "Tervin Test"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            repo
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn write(&self, name: &str, content: &str) {
            let p = self.path().join(name);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(p, content).unwrap();
        }

        fn git(&self, args: &[&str]) {
            let out = Command::new("git")
                .current_dir(self.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        /// Run a command that is expected to fail, such as a conflicting merge.
        fn git_allow_failure(&self, args: &[&str]) {
            let _ = Command::new("git")
                .current_dir(self.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .args(args)
                .output()
                .unwrap();
        }

        fn commit_file(&self, name: &str, content: &str, message: &str) {
            self.write(name, content);
            self.git(&["add", "."]);
            self.git(&["commit", "-m", message]);
        }
    }

    fn svc() -> GitService {
        GitService::new()
    }

    #[test]
    fn reports_branch_and_clean_state() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "one\n", "initial");

        let status = svc().status(repo.path()).unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert!(!status.dirty);
        assert!(status.files.is_empty());
        assert!(status.head_sha.is_some());
    }

    #[test]
    fn distinguishes_staged_from_unstaged_changes() {
        let repo = TempRepo::new();
        repo.write("a.txt", "one\n");
        repo.write("b.txt", "one\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "initial"]);

        repo.write("a.txt", "staged\n");
        repo.git(&["add", "a.txt"]);
        repo.write("b.txt", "unstaged\n");

        let status = svc().status(repo.path()).unwrap();
        assert!(status.dirty);
        assert_eq!(status.staged_count(), 1);
        assert_eq!(status.unstaged_count(), 1);
    }

    #[test]
    fn untracked_files_alone_do_not_mark_the_repo_dirty() {
        // Build output lying around must not make the dirty indicator useless.
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "one\n", "initial");
        repo.write("scratch.log", "noise\n");

        let status = svc().status(repo.path()).unwrap();
        assert!(!status.dirty, "untracked files should not count as dirty");
        assert!(status
            .files
            .iter()
            .any(|f| f.stage == StageState::Untracked && f.path == "scratch.log"));
    }

    #[test]
    fn handles_paths_with_spaces() {
        // The reason for `-z`: a naive split would break this path in two.
        let repo = TempRepo::new();
        repo.commit_file("my dir/a file.txt", "one\n", "initial");
        repo.write("my dir/a file.txt", "two\n");

        let status = svc().status(repo.path()).unwrap();
        assert!(
            status.files.iter().any(|f| f.path == "my dir/a file.txt"),
            "got {:?}",
            status.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn produces_diffs_with_hunks() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "one\ntwo\nthree\n", "initial");
        repo.write("a.txt", "one\nTWO\nthree\n");

        let diffs = svc().diff(repo.path(), DiffMode::Unstaged).unwrap();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "a.txt");
        assert_eq!(diffs[0].added_lines, 1);
        assert_eq!(diffs[0].removed_lines, 1);
        assert_eq!(diffs[0].hunks.len(), 1);
    }

    #[test]
    fn diffs_a_repository_with_no_commits() {
        // `HEAD` does not resolve before the first commit; this must not error.
        let repo = TempRepo::new();
        repo.write("a.txt", "one\n");
        repo.git(&["add", "."]);

        let diffs = svc().diff(repo.path(), DiffMode::WorkingTree);
        assert!(diffs.is_ok(), "unborn HEAD should not error: {diffs:?}");
    }

    #[test]
    fn detects_renames() {
        let repo = TempRepo::new();
        let content = "content long enough for rename detection\n".repeat(5);
        repo.commit_file("old.txt", &content, "initial");
        repo.git(&["mv", "old.txt", "new.txt"]);

        let status = svc().status(repo.path()).unwrap();
        let renamed = status
            .files
            .iter()
            .find(|f| f.path == "new.txt")
            .expect("rename not reported");
        assert_eq!(renamed.original_path.as_deref(), Some("old.txt"));
    }

    #[test]
    fn applies_a_single_hunk_patch() {
        // The hunk-level accept path, end to end through real git.
        let repo = TempRepo::new();
        let original: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        repo.commit_file("a.txt", &original, "initial");

        let mut edited: Vec<String> = original.lines().map(|l| format!("{l}\n")).collect();
        edited[1] = "CHANGED 2\n".to_string();
        edited[17] = "CHANGED 18\n".to_string();
        repo.write("a.txt", &edited.concat());

        let diffs = svc().diff(repo.path(), DiffMode::Unstaged).unwrap();
        let file = &diffs[0];
        assert_eq!(file.hunks.len(), 2, "expected two separate hunks");

        // Stage only the second hunk.
        let patch = file.patch_for_hunks(&[1]).unwrap();
        svc().apply_patch(repo.path(), &patch, false, true).unwrap();

        let staged = svc().diff(repo.path(), DiffMode::Staged).unwrap();
        let staged_added: Vec<String> = staged[0]
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .filter(|l| l.kind == DiffLineKind::Added)
            .map(|l| l.content.clone())
            .collect();
        assert_eq!(staged_added, vec!["CHANGED 18".to_string()]);
    }

    #[test]
    fn lists_branches_and_marks_head() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "one\n", "initial");
        repo.git(&["branch", "feature/x"]);

        let branches = svc().branches(repo.path()).unwrap();
        assert_eq!(branches.len(), 2);
        let head = branches.iter().find(|b| b.is_head).unwrap();
        assert_eq!(head.name, "main");
        assert!(branches.iter().any(|b| b.name == "feature/x"));
    }

    #[test]
    fn lists_the_main_worktree() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "one\n", "initial");

        let worktrees = svc().worktrees(repo.path()).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn flags_commits_not_made_through_tervin() {
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "one\n", "outside commit");

        let log = svc().log(repo.path(), 5, Some("Tervin-Session:")).unwrap();
        assert_eq!(log.len(), 1);
        assert!(
            log[0].external,
            "a commit without the marker must be shown as external"
        );
        assert_eq!(log[0].subject, "outside commit");
    }

    #[test]
    fn reports_a_merge_in_progress_with_conflicts() {
        // A commit means something different mid-merge, so the state must surface.
        let repo = TempRepo::new();
        repo.commit_file("a.txt", "base\n", "base");
        repo.git(&["checkout", "-b", "other"]);
        repo.write("a.txt", "other\n");
        repo.git(&["commit", "-am", "other change"]);
        repo.git(&["checkout", "main"]);
        repo.write("a.txt", "main\n");
        repo.git(&["commit", "-am", "main change"]);
        repo.git_allow_failure(&["merge", "other"]);

        let status = svc().status(repo.path()).unwrap();
        assert_eq!(
            status.operation_in_progress.as_deref(),
            Some("Merge in progress")
        );
        assert!(status.conflicted_count() > 0, "expected a conflicted file");
    }

    #[test]
    fn returns_none_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        assert!(svc().repo_root(dir.path()).is_none());
    }

    #[test]
    fn normalises_remote_urls_for_browsing() {
        assert_eq!(
            normalise_remote_url("git@github.com:owner/repo.git").as_deref(),
            Some("https://github.com/owner/repo")
        );
        assert_eq!(
            normalise_remote_url("https://gitlab.com/owner/repo.git").as_deref(),
            Some("https://gitlab.com/owner/repo")
        );
        assert_eq!(normalise_remote_url("").as_deref(), None);
    }
}
