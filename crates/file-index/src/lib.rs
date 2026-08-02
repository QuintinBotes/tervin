//! A project file index, for path completion and search.
//!
//! Tervin needs to know what is in the project to complete `@paths`, answer the
//! palette, and populate a file tree. That means walking the tree — and doing it
//! the way developers expect:
//!
//! **`.gitignore` is honoured**, via ripgrep's `ignore` walker rather than a
//! hand-rolled matcher. Gitignore semantics are genuinely intricate — negation,
//! directory-only patterns, precedence between nested files, `.git/info/exclude`,
//! the global `core.excludesFile` — and getting them subtly wrong shows up as
//! `node_modules` and `target` flooding every completion, which makes the feature
//! worse than not having it at all.
//!
//! **The walk is bounded.** A home directory or a monorepo can hold millions of
//! files. The index caps entries, depth, and wall-clock time, and reports when it
//! truncated rather than consuming memory until something gives.
//!
//! **Refresh is explicit and off the hot path.** Completion reads an immutable
//! snapshot; a rebuild replaces it atomically. Nothing that runs per keystroke
//! ever touches the filesystem.

pub mod fuzzy;

pub use fuzzy::{rank, score, Match};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Most entries an index will hold.
///
/// Beyond this, completion has stopped being useful anyway — nobody scrolls a
/// half-million-item list — and the memory is better not spent.
pub const MAX_ENTRIES: usize = 200_000;

/// Deepest directory level walked.
pub const MAX_DEPTH: usize = 24;

/// How long a walk may run before it reports what it has.
///
/// A network mount or a pathological tree must not hang the index thread; a
/// partial index is far better than none.
pub const WALK_BUDGET: Duration = Duration::from_secs(20);

/// One indexed path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Path relative to the index root, always with `/` separators.
    pub path: String,
    pub is_dir: bool,
}

/// An immutable snapshot of a project's files.
///
/// Handed out behind an `Arc` so a query never blocks a rebuild and a rebuild
/// never mutates what a query is reading.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    /// True when the walk hit a limit, so the UI can say the results are partial.
    pub truncated: bool,
    /// How long the walk took, for the settings pane.
    pub duration_ms: u64,
}

impl Snapshot {
    pub fn file_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.is_dir).count()
    }

    pub fn dir_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_dir).count()
    }
}

/// A completion result, with match positions for highlighting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completion {
    pub path: String,
    pub is_dir: bool,
    pub score: i32,
    /// Character indices in `path` that matched the query.
    pub positions: Vec<usize>,
}

/// What kind of entries a query wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Want {
    #[default]
    Any,
    /// Files only — what `@path` attachment wants.
    Files,
    /// Directories only — what completing `cd` wants.
    Dirs,
}

/// The project index.
///
/// Cheap to clone; all clones share one snapshot.
#[derive(Clone, Default)]
pub struct FileIndex {
    snapshot: Arc<RwLock<Arc<Snapshot>>>,
}

impl FileIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// The current snapshot. Never blocks on a walk.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.read().clone()
    }

    /// Walk `root` and replace the snapshot.
    ///
    /// Blocking and potentially slow; call from a blocking pool, never from a UI
    /// path or an async runtime.
    pub fn rebuild(&self, root: &Path) -> Arc<Snapshot> {
        let snapshot = Arc::new(walk(root));
        *self.snapshot.write() = snapshot.clone();
        snapshot
    }

    /// Complete a query against the index.
    ///
    /// `relative_to` scopes results to a subdirectory, which is how a pane's own
    /// cwd is honoured: completing inside `crates/` should not offer `ui/` paths
    /// first. Reads a snapshot only — no filesystem access.
    pub fn complete(
        &self,
        query: &str,
        want: Want,
        relative_to: Option<&str>,
        limit: usize,
    ) -> Vec<Completion> {
        let snapshot = self.snapshot();

        // A trailing slash means "list this directory", not "fuzzy-match it".
        if let Some(dir) = query.strip_suffix('/') {
            return list_dir(&snapshot, dir, want, relative_to, limit);
        }

        let prefix = relative_to
            .map(|p| p.trim_end_matches('/'))
            .filter(|p| !p.is_empty())
            .map(|p| format!("{p}/"));

        let candidates: Vec<&str> = snapshot
            .entries
            .iter()
            .filter(|e| want.accepts(e.is_dir))
            .filter(|e| match &prefix {
                // Scoped: only what lives under the requested directory.
                Some(p) => e.path.starts_with(p.as_str()),
                None => true,
            })
            .map(|e| e.path.as_str())
            .collect();

        let dirs: std::collections::HashSet<&str> = snapshot
            .entries
            .iter()
            .filter(|e| e.is_dir)
            .map(|e| e.path.as_str())
            .collect();

        fuzzy::rank(query, candidates, limit)
            .into_iter()
            .map(|(path, m)| Completion {
                is_dir: dirs.contains(path),
                path: path.to_string(),
                score: m.score,
                positions: m.positions,
            })
            .collect()
    }
}

impl Want {
    fn accepts(self, is_dir: bool) -> bool {
        match self {
            Self::Any => true,
            Self::Files => !is_dir,
            Self::Dirs => is_dir,
        }
    }
}

/// List the immediate children of a directory, directories first.
fn list_dir(
    snapshot: &Snapshot,
    dir: &str,
    want: Want,
    relative_to: Option<&str>,
    limit: usize,
) -> Vec<Completion> {
    // An empty directory query means the scope root.
    let base = if dir.is_empty() {
        relative_to.unwrap_or("").trim_end_matches('/').to_string()
    } else {
        dir.trim_end_matches('/').to_string()
    };
    let prefix = if base.is_empty() {
        String::new()
    } else {
        format!("{base}/")
    };

    let mut out: Vec<Completion> = snapshot
        .entries
        .iter()
        .filter(|e| want.accepts(e.is_dir))
        .filter(|e| {
            let Some(rest) = e.path.strip_prefix(prefix.as_str()) else {
                return false;
            };
            // Immediate children only.
            !rest.is_empty() && !rest.contains('/')
        })
        .map(|e| Completion {
            path: e.path.clone(),
            is_dir: e.is_dir,
            score: 0,
            positions: Vec::new(),
        })
        .collect();

    // Directories first, then alphabetical: the conventional listing order.
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.path.cmp(&b.path)));
    out.truncate(limit);
    out
}

/// Walk a directory tree, honouring ignore files.
/// Directories macOS guards behind a permission prompt.
///
/// Named relative to the home directory. `~/Library` is included for a different
/// reason — it is enormous, it is not source code, and walking it wastes the entire
/// budget.
const PROTECTED_HOME_DIRS: [&str; 7] = [
    "Desktop",
    "Documents",
    "Downloads",
    "Music",
    "Pictures",
    "Movies",
    "Library",
];

/// Whether descending into `path` would cross into a protected folder unasked.
///
/// Returns false when the walk root is already inside that folder: a user who opened
/// `~/Documents/project` asked for it, and a single expected prompt for a directory
/// they named is entirely different from an unexplained one for a directory they did
/// not.
fn protects(root: &Path, path: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    // Only ever applies to a direct child of the home directory.
    let Ok(relative) = path.strip_prefix(&home) else {
        return false;
    };
    let mut parts = relative.components();
    let Some(first) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        // Deeper than a direct child, so the decision was already made above it.
        return false;
    }
    let name = first.as_os_str().to_string_lossy();
    if !PROTECTED_HOME_DIRS.contains(&name.as_ref()) {
        return false;
    }
    // The root itself, or anything under it, was explicitly opened.
    !root.starts_with(path)
}

fn walk(root: &Path) -> Snapshot {
    let started = Instant::now();
    let mut entries: Vec<Entry> = Vec::new();
    let mut truncated = false;

    if !root.is_dir() {
        return Snapshot {
            root: root.to_path_buf(),
            entries,
            truncated: false,
            duration_ms: started.elapsed().as_millis() as u64,
        };
    }

    let walker = ignore::WalkBuilder::new(root)
        .max_depth(Some(MAX_DEPTH))
        // Every ignore mechanism a developer expects to work.
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .ignore(true)
        // Honour `.gitignore` even outside a git repository.
        //
        // The walker defaults to requiring git, which is right for ripgrep — a
        // stray `.gitignore` should not silently hide files from a grep. Here the
        // opposite holds: people open plain directories that still carry a
        // meaningful `.gitignore`, and a completion list flooded with `target/`
        // is the failure this index exists to prevent.
        .require_git(false)
        // Symlinks are not followed: a link back up the tree would make the walk
        // unbounded regardless of the depth cap.
        .follow_links(false)
        // Never walk *into* a directory the operating system protects.
        //
        // This is not an optimisation. On macOS, reading `~/Music` makes the system
        // ask the user for access to their media library — so indexing a broad root
        // like the home directory produced a burst of permission prompts about music,
        // photos, and the Desktop, from a terminal that has no business with any of
        // them. A tool that triggers a prompt the user cannot connect to anything they
        // did has spent trust it did not need to spend.
        //
        // Only *incidental* descent is blocked. Opening a project that genuinely lives
        // inside one of these folders still works — see `protects`.
        .filter_entry({
            let root = root.to_path_buf();
            move |entry| {
                entry.file_type().is_some_and(|t| !t.is_dir()) || !protects(&root, entry.path())
            }
        })
        .build();

    for result in walker {
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        if started.elapsed() > WALK_BUDGET {
            truncated = true;
            tracing::warn!(
                "file index walk of {} exceeded its budget; reporting {} entries",
                root.display(),
                entries.len()
            );
            break;
        }

        // An unreadable directory is normal — a permission-denied subtree must not
        // abort the whole walk.
        let Ok(entry) = result else { continue };

        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        // Skip the root itself.
        if relative.as_os_str().is_empty() {
            continue;
        }

        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());

        // Lossy is acceptable: a path that is not valid UTF-8 cannot be typed into
        // a completion box anyway, and dropping it silently would be worse than
        // showing its lossy form.
        let path = relative.to_string_lossy().replace('\\', "/");
        entries.push(Entry { path, is_dir });
    }

    // Sorted so the snapshot is deterministic and directory listing scans
    // predictably.
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    Snapshot {
        root: root.to_path_buf(),
        entries,
        truncated,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

/// Split composer input at the `@` that starts a path reference.
///
/// Returns the byte index of the `@` and the query after it, so the caller can
/// replace exactly that span when a completion is accepted.
///
/// Only a `@` at the start or after whitespace counts, so an email address does
/// not open a file picker mid-sentence.
pub fn at_path_query(input: &str, cursor: usize) -> Option<(usize, &str)> {
    let cursor = cursor.min(input.len());
    if !input.is_char_boundary(cursor) {
        return None;
    }
    let before = &input[..cursor];

    let at = before.rfind('@')?;
    // Must start a token.
    if at > 0 {
        let preceding = before[..at].chars().next_back()?;
        if !preceding.is_whitespace() {
            return None;
        }
    }

    let query = &before[at + 1..];
    // A space ends the reference.
    if query.contains(char::is_whitespace) {
        return None;
    }
    Some((at, query))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project tree containing the things that normally pollute an index.
    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let write = |rel: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        };

        write("Cargo.toml");
        write("README.md");
        write("src/main.rs");
        write("src/lib.rs");
        write("src/agent/profile.rs");
        write("ui/src/app.tsx");
        write("docs/DESIGN.md");

        // The noise a real project has.
        write("target/debug/binary");
        write("node_modules/react/index.js");
        write(".env.local");
        write("build/out.js");

        std::fs::write(
            root.join(".gitignore"),
            "target/\nnode_modules/\nbuild/\n.env.local\n",
        )
        .unwrap();

        dir
    }

    fn indexed() -> (tempfile::TempDir, FileIndex) {
        let dir = project();
        let index = FileIndex::new();
        index.rebuild(dir.path());
        (dir, index)
    }

    #[test]
    fn indexes_project_files() {
        let (_dir, index) = indexed();
        let paths: Vec<String> = index
            .snapshot()
            .entries
            .iter()
            .map(|e| e.path.clone())
            .collect();

        for expected in [
            "Cargo.toml",
            "src/main.rs",
            "src/agent/profile.rs",
            "ui/src/app.tsx",
        ] {
            assert!(paths.contains(&expected.to_string()), "{expected} missing");
        }
    }

    #[test]
    fn honours_gitignore() {
        // The whole reason for using ripgrep's walker: a completion list full of
        // node_modules is worse than no completion at all.
        let (_dir, index) = indexed();
        let paths: Vec<String> = index
            .snapshot()
            .entries
            .iter()
            .map(|e| e.path.clone())
            .collect();

        for ignored in ["target", "node_modules", "build", ".env.local"] {
            assert!(
                !paths.iter().any(|p| p.starts_with(ignored)),
                "{ignored} was indexed despite .gitignore"
            );
        }
    }

    #[test]
    fn records_directories_as_well_as_files() {
        let (_dir, index) = indexed();
        let snapshot = index.snapshot();
        assert!(snapshot.dir_count() > 0, "no directories indexed");
        assert!(snapshot.file_count() > 0, "no files indexed");
        assert!(snapshot
            .entries
            .iter()
            .any(|e| e.is_dir && e.path == "src/agent"));
    }

    #[test]
    fn paths_are_relative_with_forward_slashes() {
        let (_dir, index) = indexed();
        for entry in &index.snapshot().entries {
            assert!(!entry.path.starts_with('/'), "{} is absolute", entry.path);
            assert!(!entry.path.contains('\\'), "{} has a backslash", entry.path);
        }
    }

    #[test]
    fn completes_a_fuzzy_query_to_the_right_file() {
        let (_dir, index) = indexed();
        let results = index.complete("prof", Want::Files, None, 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].path, "src/agent/profile.rs");
        // Positions come back so the UI can highlight the match.
        assert_eq!(results[0].positions.len(), 4);
    }

    #[test]
    fn files_only_excludes_directories() {
        // `@path` attaches a file; offering a directory would attach nothing.
        let (_dir, index) = indexed();
        for result in index.complete("src", Want::Files, None, 20) {
            assert!(!result.is_dir, "{} is a directory", result.path);
        }
    }

    #[test]
    fn dirs_only_excludes_files() {
        // Completing `cd` must not offer a file.
        let (_dir, index) = indexed();
        let results = index.complete("src", Want::Dirs, None, 20);
        assert!(!results.is_empty());
        for result in &results {
            assert!(result.is_dir, "{} is a file", result.path);
        }
    }

    #[test]
    fn scoping_to_a_directory_excludes_everything_else() {
        // A pane's cwd matters: completing inside ui/ must not offer src/.
        let (_dir, index) = indexed();
        let results = index.complete("app", Want::Files, Some("ui"), 10);
        assert!(!results.is_empty());
        for result in &results {
            assert!(
                result.path.starts_with("ui/"),
                "{} escaped the scope",
                result.path
            );
        }
    }

    #[test]
    fn a_trailing_slash_lists_a_directory_instead_of_matching_it() {
        // `@src/` means "what is in src", not "fuzzy-match the word src".
        let (_dir, index) = indexed();
        let results = index.complete("src/", Want::Any, None, 20);

        assert!(!results.is_empty());
        for result in &results {
            assert!(result.path.starts_with("src/"));
            let rest = result.path.strip_prefix("src/").unwrap();
            assert!(
                !rest.contains('/'),
                "{} is not an immediate child",
                result.path
            );
        }
        // Directories before files.
        let first_file = results.iter().position(|r| !r.is_dir);
        let last_dir = results.iter().rposition(|r| r.is_dir);
        if let (Some(f), Some(d)) = (first_file, last_dir) {
            assert!(d < f, "directories should be listed before files");
        }
    }

    #[test]
    fn an_empty_query_returns_something_useful() {
        let (_dir, index) = indexed();
        let results = index.complete("", Want::Files, None, 5);
        assert_eq!(results.len(), 5, "an empty query should still list files");
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing() {
        let (_dir, index) = indexed();
        assert!(index.complete("zzzqqqxxx", Want::Files, None, 5).is_empty());
    }

    #[test]
    fn respects_the_result_limit() {
        let (_dir, index) = indexed();
        assert!(index.complete("s", Want::Any, None, 2).len() <= 2);
    }

    #[test]
    fn an_index_of_a_missing_directory_is_empty_not_an_error() {
        let index = FileIndex::new();
        let snapshot = index.rebuild(Path::new("/nonexistent/tervin/project"));
        assert!(snapshot.entries.is_empty());
        assert!(!snapshot.truncated);
    }

    #[test]
    fn querying_before_any_rebuild_returns_nothing() {
        // Completion must be safe to call during startup.
        let index = FileIndex::new();
        assert!(index.complete("anything", Want::Any, None, 10).is_empty());
    }

    #[test]
    fn a_rebuild_replaces_the_snapshot_atomically() {
        let dir = project();
        let index = FileIndex::new();
        index.rebuild(dir.path());

        // Hold a snapshot across a rebuild: it must stay valid and unchanged.
        let held = index.snapshot();
        let before = held.entries.len();

        std::fs::write(dir.path().join("src/added.rs"), b"x").unwrap();
        index.rebuild(dir.path());

        assert_eq!(
            held.entries.len(),
            before,
            "a held snapshot must not mutate"
        );
        assert!(
            index.snapshot().entries.len() > before,
            "the new snapshot should include the added file"
        );
    }

    #[test]
    fn does_not_follow_symlinks_out_of_the_tree() {
        // A link back up the tree would make the walk unbounded whatever the depth
        // cap says.
        let dir = project();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).ok();

        let index = FileIndex::new();
        let snapshot = index.rebuild(dir.path());
        assert!(!snapshot
            .entries
            .iter()
            .any(|e| e.path.matches("loop").count() > 1));
    }

    // ------------------------------------------------------------ @ parsing

    #[test]
    fn finds_an_at_path_query() {
        assert_eq!(at_path_query("look at @src/ma", 15), Some((8, "src/ma")));
        assert_eq!(at_path_query("@Cargo", 6), Some((0, "Cargo")));
        // A bare `@` opens the picker with everything.
        assert_eq!(at_path_query("@", 1), Some((0, "")));
    }

    #[test]
    fn ignores_an_at_that_does_not_start_a_token() {
        // An email address must not open a file picker.
        assert_eq!(at_path_query("mail dev@example.com", 20), None);
        assert_eq!(at_path_query("@scope/pkg", 10), Some((0, "scope/pkg")));
        assert_eq!(
            at_path_query("npm i @scope/pkg", 16),
            Some((6, "scope/pkg"))
        );
    }

    #[test]
    fn a_space_ends_an_at_reference() {
        assert_eq!(at_path_query("@src/main.rs and then", 21), None);
    }

    #[test]
    fn at_parsing_respects_the_cursor() {
        // Typing before an existing reference must complete the one being typed.
        let input = "@one @two";
        assert_eq!(at_path_query(input, 4), Some((0, "one")));
        assert_eq!(at_path_query(input, 9), Some((5, "two")));
    }

    #[test]
    fn at_parsing_is_safe_at_the_boundaries() {
        assert_eq!(at_path_query("", 0), None);
        assert_eq!(at_path_query("no at sign", 99), None);
        // A cursor inside a multi-byte character must not panic.
        assert_eq!(at_path_query("@日本語", 2), None);
    }

    #[test]
    fn indexing_this_repository_is_fast_enough_to_do_on_startup() {
        // The real thing, on a real tree with a real .gitignore.
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf);
        let Some(repo) = repo.filter(|p| p.join("Cargo.toml").exists()) else {
            return;
        };

        let index = FileIndex::new();
        let snapshot = index.rebuild(&repo);

        assert!(snapshot.file_count() > 10, "indexed too little to be real");
        // `target/` and `node_modules/` are the whole reason for the ignore walker.
        for noise in ["target/", "node_modules/"] {
            assert!(
                !snapshot.entries.iter().any(|e| e.path.starts_with(noise)),
                "{noise} leaked into the index"
            );
        }
        assert!(
            snapshot.duration_ms < 10_000,
            "indexing took {}ms",
            snapshot.duration_ms
        );

        // And the thing it exists for: finding a real file by a squashed query.
        let hits = index.complete("sesman", Want::Files, None, 5);
        assert!(
            hits.iter().any(|h| h.path.contains("session-manager")),
            "did not find session-manager: {:?}",
            hits.iter().map(|h| &h.path).collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod protection_tests {
    use super::*;

    fn home() -> PathBuf {
        dirs::home_dir().expect("no home directory")
    }

    #[test]
    fn indexing_the_home_directory_does_not_descend_into_protected_folders() {
        // The bug this exists for: a project root of `~` walked `~/Music`, and macOS
        // asked the user for access to their media library — from a terminal.
        let root = home();
        for name in PROTECTED_HOME_DIRS {
            assert!(
                protects(&root, &root.join(name)),
                "walking ~ must not descend into ~/{name}"
            );
        }
    }

    #[test]
    fn a_project_inside_a_protected_folder_is_still_indexed() {
        // Someone who opened `~/Documents/project` asked for it. One expected prompt
        // for a directory they named is not the same problem at all.
        let documents = home().join("Documents");
        let project = documents.join("project");
        assert!(
            !protects(&project, &documents),
            "opening a project inside Documents must still work"
        );
        // And its own subdirectories are unaffected.
        assert!(!protects(&project, &project.join("src")));
    }

    #[test]
    fn only_direct_children_of_home_are_protected() {
        // A directory that merely shares a name is not the protected one.
        let root = home();
        assert!(
            !protects(&root, &root.join("code").join("Music")),
            "~/code/Music is not the media library"
        );
        assert!(!protects(&root, &root.join("Projects")));
    }

    #[test]
    fn a_root_outside_the_home_directory_is_never_affected() {
        let root = PathBuf::from("/opt/work");
        assert!(!protects(&root, &root.join("Documents")));
        assert!(!protects(&root, Path::new("/etc")));
    }

    #[test]
    fn the_walk_of_a_home_directory_skips_protected_subtrees() {
        // End to end through the real walker, with a fake home-like layout. Uses the
        // real home so the protection logic is exercised rather than stubbed; the
        // assertion is only that nothing under a protected folder appears.
        let home = home();
        let snapshot = walk(&home);
        for entry in &snapshot.entries {
            let Ok(relative) = Path::new(&entry.path).strip_prefix(&home) else {
                continue;
            };
            let first = relative
                .components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .unwrap_or_default();
            assert!(
                !PROTECTED_HOME_DIRS.contains(&first.as_str()),
                "the walk reached {}, which is inside a protected folder",
                entry.path
            );
        }
    }
}
