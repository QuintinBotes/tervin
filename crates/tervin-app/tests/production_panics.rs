//! Why a panic is allowed to remain in production code, and what stops a new one.
//!
//! `panic = "abort"` in the release profile turns any panic, on any thread, into the
//! end of the process. A PTY reader thread that panics does not lose a pane; it loses
//! every pane, and the scrollback in them, with no chance to write anything down. So
//! the nineteen `unwrap`s and `expect`s that survive in `crates/*/src` are not
//! oversights that nobody has got round to — each one is a claim that a particular
//! thing cannot happen, and each now carries the argument for that claim next to it.
//!
//! The enforcement is a lint, not a grep: every crate root carries
//! `#![cfg_attr(not(test), deny(...))]`, so a new `unwrap` in production code is a
//! compile error until someone writes an `#[allow]` with a `reason`. That is the
//! point of choosing `allow_attributes_without_reason` alongside it — without it the
//! escape hatch is a bare `#[allow]`, which is exactly the unjustified panic the deny
//! was meant to stop, only quieter. `not(test)` keeps unit tests unaffected, and
//! integration tests compile as separate crates, so neither is touched.
//!
//! Two things the lint cannot do, which is why this file exists as well:
//!
//! - **A new crate can skip it.** Nothing makes `crates/whatever/src/lib.rs` carry the
//!   attribute, and clippy cannot miss what was never written.
//!   `every_crate_root_denies_an_unjustified_panic` is that check.
//! - **`clippy::unreachable` is not usable in this workspace.** `#[tauri::command]`
//!   expands to an `unreachable!` per command and clippy attributes it to the
//!   command's own return type, so denying it produces fifty errors pointing at the
//!   word `Result` in code Tervin did not write. Silencing those would need a
//!   file-wide `allow` over the whole IPC surface, which is the blanket suppression
//!   this slice removes. `nothing_in_production_reaches_for_unreachable` covers that
//!   one route textually instead.
//!
//! Stated plainly, because a silent gap is worse than a named one: this guards the
//! panics that are spelled out as calls. It does not guard indexing (`v[i]`,
//! `caps["name"]`, both of which this codebase still uses), slicing, arithmetic
//! overflow in debug builds, or a panic inside a dependency. Those need different
//! lints and, for indexing, a real change to the code rather than an attribute.
//!
//! It lives in `tervin-app` for the same reason `capability_surface.rs` does: this is
//! the crate at the top, and a claim about the whole workspace has to be asserted from
//! somewhere that can see all of it.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Every lint the crate roots deny, with the route it closes.
///
/// The list is the audit. `every_crate_root_denies_an_unjustified_panic` compares it
/// against the attribute in both directions, so adding a lint to the crate roots fails
/// until someone writes down what it stops, and removing one fails until the stale
/// entry goes.
const GUARDED: &[(&str, &str)] = &[
    ("clippy::unwrap_used", "`.unwrap()` and `.unwrap_err()`"),
    ("clippy::expect_used", "`.expect()` and `.expect_err()`"),
    ("clippy::panic", "a hand-written `panic!`"),
    ("clippy::todo", "a `todo!` reaching a release build"),
    (
        "clippy::unimplemented",
        "an `unimplemented!` reaching a release build",
    ),
    (
        "clippy::allow_attributes_without_reason",
        "silencing any of the above without saying why",
    ),
];

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/tervin-app, so the workspace is two levels up.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| panic!("{} has no grandparent", manifest.display()))
        .to_path_buf()
}

/// The `members` of the workspace, in declaration order.
///
/// Read out of the manifest's own text rather than through a TOML crate: this test
/// exists to notice a crate that was added, and the manifest is where adding one
/// happens.
fn workspace_members() -> Vec<String> {
    let path = workspace_root().join("Cargo.toml");
    let manifest = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let list = manifest
        .split_once("members = [")
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| panic!("{} has no workspace members list", path.display()));
    let list = list
        .split_once(']')
        .map(|(inside, _)| inside)
        .unwrap_or_else(|| panic!("{} has an unterminated members list", path.display()));
    // Odd-indexed fields of a split on `"` are the quoted contents.
    list.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// The files a crate's production code is rooted in. A crate has a library, a binary,
/// or both, and each is a separate compilation with its own lint levels.
fn crate_roots(member: &str) -> Vec<PathBuf> {
    let src = workspace_root().join(member).join("src");
    ["lib.rs", "main.rs"]
        .iter()
        .map(|name| src.join(name))
        .filter(|path| path.exists())
        .collect()
}

/// The lint names inside a root's `#![cfg_attr(not(test), deny(...))]`.
///
/// Whitespace is stripped first so the assertion survives whatever `rustfmt` decides
/// to do with a long attribute.
fn denied_lints(root: &Path) -> BTreeSet<String> {
    let text = fs::read_to_string(root).unwrap_or_else(|e| panic!("{}: {e}", root.display()));
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();

    const MARKER: &str = "#![cfg_attr(not(test),deny(";
    let after = compact
        .split_once(MARKER)
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| {
            panic!(
                "{} does not deny anything outside cfg(test). A crate without the \
                 attribute is a crate where `let _ = None::<u8>.unwrap();` compiles \
                 clean, and `panic = \"abort\"` makes that the whole window.",
                root.display()
            )
        });
    let inside = after
        .split_once("))]")
        .map(|(lints, _)| lints)
        .unwrap_or_else(|| panic!("{}: the deny list is unterminated", root.display()));

    inside
        .split(',')
        .filter(|lint| !lint.is_empty())
        .map(str::to_string)
        .collect()
}

/// Everything in a source file that is compiled outside `cfg(test)`, best-effort.
///
/// The split is the file's first `#[cfg(test)]`, because every file in this repository
/// keeps its unit tests in one module at the end. A file that put production code
/// *after* its test module would hide from this — which is why this is a backstop for
/// the one macro clippy cannot see, and not the guard itself. The guard is the lint.
fn production_prefix(text: &str) -> &str {
    match text.split_once("#[cfg(test)]") {
        Some((before, _)) => before,
        None => text,
    }
}

/// Every `.rs` file under `crates/*/src`.
fn production_sources() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let crates = workspace_root().join("crates");
    let entries = fs::read_dir(&crates).unwrap_or_else(|e| panic!("{}: {e}", crates.display()));
    for entry in entries.flatten() {
        collect_rs(&entry.path().join("src"), &mut found);
    }
    found.sort();
    found
}

fn collect_rs(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn every_crate_root_denies_an_unjustified_panic() {
    let members = workspace_members();
    // Guard first: an empty read would make every assertion below vacuously true and
    // report a clean workspace while checking nothing at all.
    assert!(
        members.len() >= 10,
        "parsed {} workspace members out of Cargo.toml — the scan is broken",
        members.len()
    );

    // A crate on disk that is not a member is never compiled by `--workspace`, so no
    // lint in it is ever checked and no test in it is ever run.
    let declared: BTreeSet<&str> = members.iter().map(String::as_str).collect();
    let crates_dir = workspace_root().join("crates");
    let on_disk: BTreeSet<String> = fs::read_dir(&crates_dir)
        .unwrap_or_else(|e| panic!("{}: {e}", crates_dir.display()))
        .flatten()
        .filter(|entry| entry.path().join("Cargo.toml").exists())
        .map(|entry| format!("crates/{}", entry.file_name().to_string_lossy()))
        .collect();
    let orphans: Vec<&String> = on_disk
        .iter()
        .filter(|path| !declared.contains(path.as_str()))
        .collect();
    assert!(
        orphans.is_empty(),
        "{orphans:?} exist but are not workspace members, so `cargo clippy --workspace` \
         never looks at them"
    );

    let expected: BTreeSet<String> = GUARDED.iter().map(|(lint, _)| lint.to_string()).collect();
    for member in &members {
        let roots = crate_roots(member);
        assert!(
            !roots.is_empty(),
            "{member} has neither src/lib.rs nor src/main.rs, so nothing here checked it"
        );
        for root in roots {
            let denied = denied_lints(&root);

            for (lint, stops) in GUARDED {
                assert!(
                    denied.contains(*lint),
                    "{} does not deny {lint}, so {stops} would compile there with no \
                     word of justification anywhere",
                    root.display()
                );
            }

            let undocumented: Vec<&String> = denied.difference(&expected).collect();
            assert!(
                undocumented.is_empty(),
                "{} denies {undocumented:?}, which GUARDED does not explain. Say what \
                 the lint stops, so the next person reads a reason rather than a name.",
                root.display()
            );
        }
    }
}

#[test]
fn nothing_in_production_reaches_for_unreachable() {
    // The split is a heuristic, so prove it on a fixture before trusting it on the
    // tree. Without this the test would pass just as happily if the split returned
    // nothing at all.
    let fixture =
        "fn a() { unreachable!() }\n#[cfg(test)]\nmod tests {\n    fn b() { unreachable!() }\n}\n";
    assert_eq!(
        production_prefix(fixture).matches("unreachable!").count(),
        1,
        "the cfg(test) split is broken: it should keep the production one and drop the \
         one in the test module"
    );

    let sources = production_sources();
    assert!(
        sources.len() > 30,
        "walked {} source files under crates/*/src — the walk is broken",
        sources.len()
    );

    for path in sources {
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(
            !production_prefix(&text).contains("unreachable!"),
            "{}: `unreachable!` in production code. It aborts the process like any \
             other panic, and it is the one route the crate-root deny cannot cover, \
             because `#[tauri::command]` expands to one per command. Return an error \
             the caller can act on instead.",
            path.display()
        );
    }
}
