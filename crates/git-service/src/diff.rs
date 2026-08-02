//! Unified-diff parsing.
//!
//! Parsed into hunks with per-line numbering, which is what lets the same data
//! drive unified and side-by-side views, link a timeline event to an exact hunk,
//! and reconstruct an applyable patch for hunk-level accept or revert.

use crate::model::{ChangeKind, DiffLine, DiffLineKind, FileDiff, Hunk};

/// Parse the output of `git diff`.
pub fn parse_unified_diff(input: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current: Option<FileDiff> = None;
    let mut hunk: Option<Hunk> = None;
    let mut old_lineno = 0u32;
    let mut new_lineno = 0u32;

    for line in input.lines() {
        if line.starts_with("diff --git ") {
            finish_hunk(&mut current, &mut hunk);
            if let Some(f) = current.take() {
                files.push(f);
            }
            let (old_path, path) = parse_diff_git_paths(line);
            current = Some(FileDiff {
                path,
                old_path,
                kind: ChangeKind::Modified,
                binary: false,
                hunks: Vec::new(),
                added_lines: 0,
                removed_lines: 0,
                raw_header: vec![line.to_string()],
            });
            continue;
        }

        let Some(file) = current.as_mut() else {
            // Text before the first `diff --git` is not part of any file.
            continue;
        };

        // Extended header lines, all of which belong to the reconstructable patch.
        if hunk.is_none() && is_header_line(line) {
            file.raw_header.push(line.to_string());

            if let Some(rest) = line.strip_prefix("rename from ") {
                file.old_path = Some(unquote(rest));
                file.kind = ChangeKind::Renamed;
            } else if let Some(rest) = line.strip_prefix("rename to ") {
                file.path = unquote(rest);
                file.kind = ChangeKind::Renamed;
            } else if line.starts_with("new file mode") {
                file.kind = ChangeKind::Added;
            } else if line.starts_with("deleted file mode") {
                file.kind = ChangeKind::Deleted;
            } else if line.starts_with("copy from ") || line.starts_with("copy to ") {
                file.kind = ChangeKind::Copied;
            } else if line.starts_with("old mode") || line.starts_with("new mode") {
                file.kind = ChangeKind::TypeChanged;
            } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                file.binary = true;
            } else if let Some(rest) = line.strip_prefix("--- ") {
                if rest != "/dev/null" {
                    file.old_path = Some(strip_prefix_marker(rest));
                }
            } else if let Some(rest) = line.strip_prefix("+++ ") {
                if rest != "/dev/null" {
                    file.path = strip_prefix_marker(rest);
                }
            }
            continue;
        }

        if line.starts_with("@@") {
            finish_hunk(&mut current, &mut hunk);
            if let Some(parsed) = parse_hunk_header(line) {
                old_lineno = parsed.old_start;
                new_lineno = parsed.new_start;
                hunk = Some(parsed);
            }
            continue;
        }

        let Some(h) = hunk.as_mut() else {
            // A stray "Binary files differ" can appear after headers.
            if line.starts_with("Binary files ") {
                file.binary = true;
            }
            continue;
        };

        // Hunk body. An empty line inside a hunk is a context line whose single
        // leading space git omitted.
        let (marker, content) = match line.chars().next() {
            Some(c) => (c, &line[c.len_utf8()..]),
            None => (' ', ""),
        };

        match marker {
            '+' => {
                h.lines.push(DiffLine {
                    kind: DiffLineKind::Added,
                    content: content.to_string(),
                    old_lineno: None,
                    new_lineno: Some(new_lineno),
                });
                new_lineno += 1;
                file.added_lines += 1;
            }
            '-' => {
                h.lines.push(DiffLine {
                    kind: DiffLineKind::Removed,
                    content: content.to_string(),
                    old_lineno: Some(old_lineno),
                    new_lineno: None,
                });
                old_lineno += 1;
                file.removed_lines += 1;
            }
            '\\' => {
                h.lines.push(DiffLine {
                    kind: DiffLineKind::NoNewline,
                    content: content.to_string(),
                    old_lineno: None,
                    new_lineno: None,
                });
            }
            _ => {
                h.lines.push(DiffLine {
                    kind: DiffLineKind::Context,
                    content: content.to_string(),
                    old_lineno: Some(old_lineno),
                    new_lineno: Some(new_lineno),
                });
                old_lineno += 1;
                new_lineno += 1;
            }
        }
    }

    finish_hunk(&mut current, &mut hunk);
    if let Some(f) = current.take() {
        files.push(f);
    }
    files
}

fn is_header_line(line: &str) -> bool {
    const PREFIXES: [&str; 13] = [
        "index ",
        "--- ",
        "+++ ",
        "old mode",
        "new mode",
        "new file mode",
        "deleted file mode",
        "similarity index",
        "dissimilarity index",
        "rename from ",
        "rename to ",
        "copy from ",
        "copy to ",
    ];
    PREFIXES.iter().any(|p| line.starts_with(p))
        || line.starts_with("Binary files ")
        || line.starts_with("GIT binary patch")
}

fn finish_hunk(file: &mut Option<FileDiff>, hunk: &mut Option<Hunk>) {
    if let (Some(f), Some(h)) = (file.as_mut(), hunk.take()) {
        f.hunks.push(h);
    }
}

/// `@@ -12,7 +12,9 @@ fn example()`
fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@")?;
    let close = rest.find("@@")?;
    let ranges = rest[..close].trim();
    let section = rest[close + 2..].trim();

    let mut parts = ranges.split_whitespace();
    let (old_start, old_lines) = parse_range(parts.next()?.strip_prefix('-')?)?;
    let (new_start, new_lines) = parse_range(parts.next()?.strip_prefix('+')?)?;

    Some(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        section: if section.is_empty() {
            None
        } else {
            Some(section.to_string())
        },
        lines: Vec::new(),
    })
}

/// `12,7` or bare `12`, which means a single line.
fn parse_range(s: &str) -> Option<(u32, u32)> {
    match s.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

/// Pull both paths out of `diff --git a/x b/y`.
///
/// Ambiguous when a path contains " b/", so the authoritative values come from
/// the `---`/`+++` or `rename` lines that follow; this is only the initial guess.
fn parse_diff_git_paths(line: &str) -> (Option<String>, String) {
    let rest = line.trim_start_matches("diff --git ").trim();
    if let Some(idx) = rest.find(" b/") {
        let old = strip_prefix_marker(&rest[..idx]);
        let new = strip_prefix_marker(rest[idx + 1..].trim());
        return (Some(old), new);
    }
    (None, strip_prefix_marker(rest))
}

/// Remove git's `a/` or `b/` prefix, and any surrounding quotes.
fn strip_prefix_marker(s: &str) -> String {
    let s = unquote(s.trim());
    // Trailing tab-separated timestamps appear in some diff dialects.
    let s = s.split('\t').next().unwrap_or(&s).to_string();
    for p in ["a/", "b/", "i/", "w/", "c/", "o/"] {
        if let Some(rest) = s.strip_prefix(p) {
            return rest.to_string();
        }
    }
    s
}

/// Undo git's C-style quoting for paths with unusual characters.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if !(s.starts_with('"') && s.ends_with('"') && s.len() >= 2) {
        return s.to_string();
    }
    let inner = &s[1..s.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/src/main.rs b/src/main.rs
index 83db48f..bf269f4 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,4 +1,5 @@ fn main()
 use std::io;
-let x = 1;
+let x = 2;
+let y = 3;
 println!(\"hi\");
diff --git a/README.md b/README.md
new file mode 100644
index 0000000..e69de29
--- /dev/null
+++ b/README.md
@@ -0,0 +1,2 @@
+# Title
+body
";

    #[test]
    fn parses_multiple_files_with_correct_line_numbers() {
        let files = parse_unified_diff(SAMPLE);
        assert_eq!(files.len(), 2);

        let main = &files[0];
        assert_eq!(main.path, "src/main.rs");
        assert_eq!(main.added_lines, 2);
        assert_eq!(main.removed_lines, 1);
        assert_eq!(main.hunks.len(), 1);
        assert_eq!(main.hunks[0].section.as_deref(), Some("fn main()"));

        let lines = &main.hunks[0].lines;
        // Context keeps both numbers; additions and removals keep only one.
        assert_eq!(lines[0].old_lineno, Some(1));
        assert_eq!(lines[0].new_lineno, Some(1));
        assert_eq!(lines[1].kind, DiffLineKind::Removed);
        assert_eq!(lines[1].old_lineno, Some(2));
        assert_eq!(lines[1].new_lineno, None);
        assert_eq!(lines[2].kind, DiffLineKind::Added);
        assert_eq!(lines[2].new_lineno, Some(2));
        // Trailing context resumes from the right numbers on both sides.
        let last = lines.last().unwrap();
        assert_eq!(last.kind, DiffLineKind::Context);
        assert_eq!(last.old_lineno, Some(3));
        assert_eq!(last.new_lineno, Some(4));
    }

    #[test]
    fn detects_added_files() {
        let files = parse_unified_diff(SAMPLE);
        let readme = &files[1];
        assert_eq!(readme.path, "README.md");
        assert_eq!(readme.kind, ChangeKind::Added);
        assert_eq!(readme.added_lines, 2);
    }

    #[test]
    fn detects_renames() {
        let input = "\
diff --git a/old.rs b/new.rs
similarity index 95%
rename from old.rs
rename to new.rs
index 1111111..2222222 100644
--- a/old.rs
+++ b/new.rs
@@ -1 +1 @@
-a
+b
";
        let files = parse_unified_diff(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].kind, ChangeKind::Renamed);
        assert_eq!(files[0].old_path.as_deref(), Some("old.rs"));
        assert_eq!(files[0].path, "new.rs");
    }

    #[test]
    fn flags_binary_files_rather_than_showing_an_empty_diff() {
        let input = "\
diff --git a/logo.png b/logo.png
index 1111111..2222222 100644
Binary files a/logo.png and b/logo.png differ
";
        let files = parse_unified_diff(input);
        assert!(files[0].binary);
        assert!(files[0].hunks.is_empty());
        // A binary file has no patch to reconstruct.
        assert!(files[0].patch_for_hunks(&[0]).is_none());
    }

    #[test]
    fn treats_a_blank_line_in_a_hunk_as_context() {
        // git omits the leading space on an empty context line. Mis-handling it
        // shifts every subsequent line number in the file.
        let input = "\
diff --git a/a.txt b/a.txt
index 1111111..2222222 100644
--- a/a.txt
+++ b/a.txt
@@ -1,4 +1,4 @@
 first

-third
+THIRD
";
        let files = parse_unified_diff(input);
        let lines = &files[0].hunks[0].lines;
        assert_eq!(lines[1].kind, DiffLineKind::Context);
        assert_eq!(lines[1].content, "");
        assert_eq!(lines[2].old_lineno, Some(3));
    }

    #[test]
    fn handles_single_line_ranges_without_a_count() {
        let input = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -3 +3 @@
-x
+y
";
        let files = parse_unified_diff(input);
        let h = &files[0].hunks[0];
        assert_eq!(h.old_start, 3);
        assert_eq!(h.old_lines, 1);
        assert_eq!(h.new_start, 3);
    }

    #[test]
    fn reconstructs_an_applyable_patch_for_selected_hunks() {
        // Hunk-level accept and revert depend on this being byte-exact.
        let input = "\
diff --git a/a.txt b/a.txt
index 1111111..2222222 100644
--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
-one
+ONE
 two
@@ -10,2 +10,2 @@
-ten
+TEN
 eleven
";
        let files = parse_unified_diff(input);
        let patch = files[0].patch_for_hunks(&[1]).unwrap();

        assert!(patch.starts_with("diff --git a/a.txt b/a.txt\n"));
        assert!(patch.contains("--- a/a.txt"));
        assert!(patch.contains("@@ -10,2 +10,2 @@"));
        assert!(patch.contains("-ten"));
        assert!(patch.contains("+TEN"));
        // The unselected hunk must be absent.
        assert!(!patch.contains("+ONE"));
        // Selecting nothing yields no patch, rather than an empty one that would
        // "succeed" while doing nothing.
        assert!(files[0].patch_for_hunks(&[]).is_none());
    }

    #[test]
    fn preserves_no_newline_markers() {
        let input = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-a
\\ No newline at end of file
+b
";
        let files = parse_unified_diff(input);
        let kinds: Vec<_> = files[0].hunks[0].lines.iter().map(|l| l.kind).collect();
        assert!(kinds.contains(&DiffLineKind::NoNewline));
        // The marker must survive into a reconstructed patch or `git apply` fails.
        let patch = files[0].patch_for_hunks(&[0]).unwrap();
        assert!(patch.contains("\\ No newline at end of file"));
    }

    #[test]
    fn unquotes_paths_with_unusual_characters() {
        let input = "\
diff --git \"a/my dir/f\\ttab.txt\" \"b/my dir/f\\ttab.txt\"
--- \"a/my dir/f\\ttab.txt\"
+++ \"b/my dir/f\\ttab.txt\"
@@ -1 +1 @@
-a
+b
";
        let files = parse_unified_diff(input);
        assert_eq!(files.len(), 1);
        assert!(files[0].path.contains("my dir/f"));
    }

    #[test]
    fn empty_input_yields_no_files() {
        assert!(parse_unified_diff("").is_empty());
        assert!(parse_unified_diff("not a diff at all\n").is_empty());
    }
}
