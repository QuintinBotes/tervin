//! Recovering structure from terminal output.
//!
//! Everything here is best-effort and clearly labelled as such. The parsed
//! results drive affordances — open this file at this line, follow this port,
//! jump to this error — while the raw output remains authoritative. When a
//! pattern does not match, the correct outcome is no affordance, never a guess
//! presented as fact.

use crate::model::{ParsedDiagnostic, ParsedOutput, PathHit, TestSummary};
use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use tervin_core::events::Severity;

/// Cap on filesystem checks per Block, so parsing a huge log cannot turn into
/// thousands of stat calls on the UI's critical path.
const MAX_EXISTENCE_CHECKS: usize = 200;

/// Cap on how much output we scan for structure. Beyond this the raw text is
/// still kept in full; only the affordance extraction stops.
const MAX_PARSE_BYTES: usize = 2 * 1024 * 1024;

/// Remove ANSI escape sequences, yielding plain text suitable for parsing,
/// search indexing, and plain-text export.
///
/// Deliberately conservative: it drops escape sequences and leaves every other
/// byte alone, including UTF-8 multi-byte sequences, so CJK text and emoji
/// survive intact.
pub fn strip_ansi(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != 0x1B {
            out.push(bytes[i]);
            i += 1;
            continue;
        }

        // ESC — decide which sequence family and skip it entirely.
        i += 1;
        if i >= bytes.len() {
            break;
        }
        match bytes[i] {
            b'[' => {
                // CSI: params/intermediates until a final byte 0x40..=0x7E.
                i += 1;
                while i < bytes.len() && !(0x40..=0x7E).contains(&bytes[i]) {
                    i += 1;
                }
                i += 1;
            }
            b']' | b'P' | b'_' | b'^' | b'X' => {
                // String sequences: run to BEL or ST.
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            // Two-byte escapes, plus charset designators like `ESC ( B`.
            b'(' | b')' | b'*' | b'+' => i += 2,
            _ => i += 1,
        }
    }

    String::from_utf8_lossy(&out).to_string()
}

static RE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s"'<>)\]}\\]+"#).unwrap());

/// `path:line[:col]` — the form most toolchains and editors agree on.
static RE_PATH_LOC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<path>(?:[A-Za-z]:)?[~.]?[\w./\-+@]*[\w\-+]\.[A-Za-z][\w]*):(?P<line>\d{1,7})(?::(?P<col>\d{1,7}))?")
        .unwrap()
});

/// A bare path with a directory separator, anchored so prose is not mistaken for
/// a filename.
static RE_BARE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|[\s'"(\[])(?P<path>(?:\./|\.\./|/|~/)[\w./\-+@]*[\w\-+/])"#).unwrap()
});

static RE_HOST_PORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]|::1):(?P<port>\d{2,5})").unwrap()
});

static RE_PORT_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bport\s+:?(?P<port>\d{2,5})\b").unwrap());

/// rustc / cargo: `error[E0433]: message` and `warning: message`.
static RE_RUSTC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(?P<sev>error|warning)(?:\[(?P<code>E\d+)\])?(?:\([^)]*\))?: (?P<msg>.+)$")
        .unwrap()
});

/// rustc's location line: `  --> src/main.rs:10:5`.
static RE_RUSTC_LOC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*-->\s*(?P<path>[^\s:]+):(?P<line>\d+):(?P<col>\d+)").unwrap()
});

/// TypeScript: `src/a.ts(12,3): error TS2304: message`.
static RE_TSC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(?P<path>[^\s(]+)\((?P<line>\d+),(?P<col>\d+)\):\s*(?P<sev>error|warning)\s+(?P<code>TS\d+):\s*(?P<msg>.+)$")
        .unwrap()
});

/// clang / gcc / eslint: `path:line:col: error: message`.
static RE_GCC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?P<path>[^\s:]+):(?P<line>\d+):(?P<col>\d+):\s*(?P<sev>error|warning|note):\s*(?P<msg>.+)$")
        .unwrap()
});

/// Python tracebacks.
static RE_PY_TRACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^\s*File "(?P<path>[^"]+)", line (?P<line>\d+)"#).unwrap());

/// `test result: ok. 17 passed; 0 failed; 0 ignored` (cargo).
static RE_CARGO_TESTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"test result:\s*(?P<outcome>\w+)\.\s*(?P<passed>\d+) passed;\s*(?P<failed>\d+) failed;\s*(?P<ignored>\d+) ignored")
        .unwrap()
});

/// `Tests:  1 failed, 2 passed, 3 total` (jest / vitest).
static RE_JEST_TESTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)tests:\s*(?:(?P<failed>\d+) failed,\s*)?(?:(?P<skipped>\d+) skipped,\s*)?(?P<passed>\d+) passed")
        .unwrap()
});

/// `=== 3 passed, 1 failed in 0.42s ===` (pytest).
static RE_PYTEST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?P<passed>\d+) passed(?:,\s*(?P<failed>\d+) failed)?(?:,\s*(?P<skipped>\d+) skipped)?",
    )
    .unwrap()
});

/// Extract structure from a Block's output.
///
/// `cwd` is used to resolve relative paths for existence checks; nothing is read,
/// only stat'ed, and only up to [`MAX_EXISTENCE_CHECKS`] times.
pub fn extract(raw_output: &str, cwd: &str) -> ParsedOutput {
    let text = strip_ansi(raw_output);
    let scan: &str = if text.len() > MAX_PARSE_BYTES {
        // Cut on a char boundary so we never slice a multi-byte sequence.
        let mut end = MAX_PARSE_BYTES;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    } else {
        &text
    };

    let mut parsed = ParsedOutput::default();
    let mut seen_paths: BTreeSet<(String, Option<u32>)> = BTreeSet::new();
    let mut checks = 0usize;

    let mut push_path = |parsed: &mut ParsedOutput,
                         path: &str,
                         line: Option<u32>,
                         column: Option<u32>,
                         checks: &mut usize| {
        let path = path.trim_end_matches([':', ',', '.', ')', ']', '"', '\'']);
        if path.is_empty() || path.len() > 512 {
            return;
        }
        let key = (path.to_string(), line);
        if !seen_paths.insert(key) {
            return;
        }
        let exists = if *checks < MAX_EXISTENCE_CHECKS {
            *checks += 1;
            resolve(path, cwd).map(|p| p.exists()).unwrap_or(false)
        } else {
            false
        };
        parsed.paths.push(PathHit {
            path: path.to_string(),
            line,
            column,
            exists,
        });
    };

    // URLs.
    let mut seen_urls = BTreeSet::new();
    for m in RE_URL.find_iter(scan) {
        let url = m.as_str().trim_end_matches(['.', ',', ')', ']', '"', '\'']);
        if seen_urls.insert(url.to_string()) {
            parsed.urls.push(url.to_string());
        }
    }

    // Ports, both `host:port` and prose like "listening on port 3000".
    let mut seen_ports = BTreeSet::new();
    for caps in RE_HOST_PORT
        .captures_iter(scan)
        .chain(RE_PORT_WORD.captures_iter(scan))
    {
        if let Some(port) = caps
            .name("port")
            .and_then(|m| m.as_str().parse::<u16>().ok())
        {
            if port > 0 && seen_ports.insert(port) {
                parsed.ports.push(port);
            }
        }
    }

    // Structured diagnostics, most specific patterns first.
    for caps in RE_TSC.captures_iter(scan) {
        let severity = severity_from(&caps["sev"]);
        parsed.diagnostics.push(ParsedDiagnostic {
            severity,
            message: format!("{} {}", &caps["code"], &caps["msg"]),
            path: Some(caps["path"].to_string()),
            line: caps["line"].parse().ok(),
            column: caps["col"].parse().ok(),
            source: Some("tsc".to_string()),
        });
        push_path(
            &mut parsed,
            &caps["path"],
            caps["line"].parse().ok(),
            caps["col"].parse().ok(),
            &mut checks,
        );
    }

    for caps in RE_GCC.captures_iter(scan) {
        let severity = severity_from(&caps["sev"]);
        parsed.diagnostics.push(ParsedDiagnostic {
            severity,
            message: caps["msg"].to_string(),
            path: Some(caps["path"].to_string()),
            line: caps["line"].parse().ok(),
            column: caps["col"].parse().ok(),
            source: None,
        });
    }

    // rustc reports the message and the location on separate lines; pair each
    // message with the next location that follows it.
    let rustc_locs: Vec<(usize, &str, u32, u32)> = RE_RUSTC_LOC
        .captures_iter(scan)
        .filter_map(|c| {
            let m = c.get(0)?;
            Some((
                m.start(),
                c.name("path")?.as_str(),
                c.name("line")?.as_str().parse().ok()?,
                c.name("col")?.as_str().parse().ok()?,
            ))
        })
        .collect();

    for caps in RE_RUSTC.captures_iter(scan) {
        let whole = caps.get(0).unwrap();
        let severity = severity_from(&caps["sev"]);
        let loc = rustc_locs.iter().find(|(pos, ..)| *pos > whole.start());
        let message = match caps.name("code") {
            Some(code) => format!("[{}] {}", code.as_str(), &caps["msg"]),
            None => caps["msg"].to_string(),
        };
        parsed.diagnostics.push(ParsedDiagnostic {
            severity,
            message,
            path: loc.map(|(_, p, ..)| p.to_string()),
            line: loc.map(|(_, _, l, _)| *l),
            column: loc.map(|(.., c)| *c),
            source: Some("rustc".to_string()),
        });
    }

    for caps in RE_PY_TRACE.captures_iter(scan) {
        push_path(
            &mut parsed,
            &caps["path"],
            caps["line"].parse().ok(),
            None,
            &mut checks,
        );
    }

    // Paths with locations, then bare paths.
    for caps in RE_PATH_LOC.captures_iter(scan) {
        push_path(
            &mut parsed,
            &caps["path"],
            caps.name("line").and_then(|m| m.as_str().parse().ok()),
            caps.name("col").and_then(|m| m.as_str().parse().ok()),
            &mut checks,
        );
    }
    for caps in RE_BARE_PATH.captures_iter(scan) {
        push_path(&mut parsed, &caps["path"], None, None, &mut checks);
    }

    parsed.tests = extract_tests(scan);

    parsed.error_count = parsed
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count() as u32;
    parsed.warning_count = parsed
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count() as u32;

    parsed
}

fn severity_from(s: &str) -> Severity {
    match s.to_ascii_lowercase().as_str() {
        "error" => Severity::Error,
        "warning" => Severity::Warning,
        "note" => Severity::Info,
        _ => Severity::Info,
    }
}

/// Resolve a possibly relative or `~`-prefixed path against a working directory.
fn resolve(path: &str, cwd: &str) -> Option<PathBuf> {
    if let Some(rest) = path.strip_prefix("~/") {
        return dirs_home().map(|h| h.join(rest));
    }
    let p = Path::new(path);
    if p.is_absolute() {
        Some(p.to_path_buf())
    } else {
        Some(Path::new(cwd).join(p))
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Recognise a test-runner summary line.
fn extract_tests(text: &str) -> Option<TestSummary> {
    // cargo prints one summary per test target; sum them so a workspace run
    // reports its true total rather than only the last target's.
    let mut cargo: Option<TestSummary> = None;
    for caps in RE_CARGO_TESTS.captures_iter(text) {
        let entry = cargo.get_or_insert(TestSummary {
            runner: "cargo".to_string(),
            passed: 0,
            failed: 0,
            skipped: 0,
        });
        entry.passed += caps["passed"].parse::<u32>().unwrap_or(0);
        entry.failed += caps["failed"].parse::<u32>().unwrap_or(0);
        entry.skipped += caps["ignored"].parse::<u32>().unwrap_or(0);
    }
    if cargo.is_some() {
        return cargo;
    }

    if let Some(caps) = RE_JEST_TESTS.captures(text) {
        return Some(TestSummary {
            runner: "jest".to_string(),
            passed: caps
                .name("passed")
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0),
            failed: caps
                .name("failed")
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0),
            skipped: caps
                .name("skipped")
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0),
        });
    }

    // pytest's summary lives inside a `===` banner; requiring that avoids
    // matching the words "3 passed" in arbitrary prose.
    if text.contains("passed")
        && text
            .lines()
            .any(|l| l.starts_with("===") || l.contains("= "))
    {
        if let Some(caps) = RE_PYTEST.captures(text) {
            return Some(TestSummary {
                runner: "pytest".to_string(),
                passed: caps
                    .name("passed")
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0),
                failed: caps
                    .name("failed")
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0),
                skipped: caps
                    .name("skipped")
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0),
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_colour_but_keeps_text() {
        assert_eq!(strip_ansi("\x1b[1;31merror\x1b[0m: bad"), "error: bad");
    }

    #[test]
    fn strips_osc_sequences_including_shell_marks() {
        assert_eq!(strip_ansi("\x1b]133;A\x07$ ls\x1b]133;C\x07"), "$ ls");
    }

    #[test]
    fn preserves_multibyte_text() {
        // Terminal correctness includes not mangling CJK or emoji on the way to
        // the search index.
        let input = "\x1b[32m日本語テスト 🚀\x1b[0m";
        assert_eq!(strip_ansi(input), "日本語テスト 🚀");
    }

    #[test]
    fn finds_rust_diagnostics_with_locations() {
        let out = "error[E0433]: failed to resolve\n  --> src/main.rs:10:5\n   |\nwarning: unused import\n  --> src/lib.rs:2:1\n";
        let p = extract(out, "/tmp");
        assert_eq!(p.error_count, 1);
        assert_eq!(p.warning_count, 1);
        let err = p
            .diagnostics
            .iter()
            .find(|d| d.severity == Severity::Error)
            .unwrap();
        assert!(err.message.contains("E0433"));
        assert_eq!(err.path.as_deref(), Some("src/main.rs"));
        assert_eq!(err.line, Some(10));
    }

    #[test]
    fn finds_typescript_diagnostics() {
        let out = "src/app.ts(12,3): error TS2304: Cannot find name 'foo'.";
        let p = extract(out, "/tmp");
        assert_eq!(p.error_count, 1);
        let d = &p.diagnostics[0];
        assert_eq!(d.source.as_deref(), Some("tsc"));
        assert_eq!(d.line, Some(12));
        assert_eq!(d.column, Some(3));
    }

    #[test]
    fn finds_urls_and_trims_trailing_punctuation() {
        let p = extract(
            "see https://example.com/a/b. also (http://localhost:5173)",
            "/tmp",
        );
        assert!(p.urls.contains(&"https://example.com/a/b".to_string()));
        assert!(p.urls.contains(&"http://localhost:5173".to_string()));
    }

    #[test]
    fn finds_ports_from_addresses_and_prose() {
        let p = extract(
            "Server listening on port 8080\nready at http://127.0.0.1:5173/",
            "/tmp",
        );
        assert!(p.ports.contains(&8080));
        assert!(p.ports.contains(&5173));
    }

    #[test]
    fn sums_cargo_test_summaries_across_targets() {
        // A workspace run prints one line per target; reporting only the last
        // would understate the result.
        let out = "test result: ok. 17 passed; 0 failed; 0 ignored\n\
                   test result: FAILED. 3 passed; 2 failed; 1 ignored\n";
        let t = extract(out, "/tmp").tests.unwrap();
        assert_eq!(t.runner, "cargo");
        assert_eq!(t.passed, 20);
        assert_eq!(t.failed, 2);
        assert_eq!(t.skipped, 1);
    }

    #[test]
    fn finds_jest_summary() {
        let t = extract(
            "Tests:       1 failed, 2 skipped, 9 passed, 12 total",
            "/tmp",
        )
        .tests
        .unwrap();
        assert_eq!(t.failed, 1);
        assert_eq!(t.skipped, 2);
        assert_eq!(t.passed, 9);
    }

    #[test]
    fn does_not_invent_a_test_summary_from_prose() {
        // "passed" in ordinary output must not fabricate a test result.
        assert!(extract("the deadline passed without incident", "/tmp")
            .tests
            .is_none());
    }

    #[test]
    fn marks_paths_that_actually_exist() {
        // Existence is checked so the UI only offers to open real files.
        let dir = std::env::temp_dir();
        let name = format!("tervin-parse-{}.txt", std::process::id());
        let file = dir.join(&name);
        std::fs::write(&file, b"x").unwrap();

        let out = format!("wrote ./{name} and ./definitely-missing-{name}");
        let p = extract(&out, dir.to_str().unwrap());

        let found = p.paths.iter().find(|h| h.path.ends_with(&name) && h.exists);
        assert!(
            found.is_some(),
            "existing file was not marked: {:?}",
            p.paths
        );
        assert!(p
            .paths
            .iter()
            .any(|h| h.path.contains("definitely-missing") && !h.exists));

        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn handles_output_larger_than_the_parse_cap() {
        // Parsing stops, but must not panic or slice a char boundary.
        let mut big = "x".repeat(MAX_PARSE_BYTES + 10);
        big.push_str("日本語");
        let p = extract(&big, "/tmp");
        assert_eq!(p.error_count, 0);
    }
}
