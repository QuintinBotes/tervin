//! What the webview is allowed to do, asserted rather than reviewed.
//!
//! `capabilities/default.json` is compiled into the binary by `tauri-build`, and
//! nothing at runtime narrows it: a permission listed there is granted to the web
//! content for the life of the process. That makes it the one file in this repository
//! where a careless line is a security change, and also the one file where nothing
//! checks the *meaning* — an over-broad scope builds perfectly and ships.
//!
//! It carried `opener:allow-open-path` scoped to `{"path": "**"}`, which let the
//! webview hand any path on disk to the system's default application. A scope cannot
//! express "inside the project", because it is static JSON and the project root is
//! chosen at runtime, so the grant moved to `commands::open_path` — which runs where
//! the root is known and asks before leaving it.
//!
//! The other half of the same surface is the content security policy in
//! `tauri.conf.json`. `capabilities/default.json` decides which Tauri commands the web
//! content may call; the policy decides where it may send bytes on its own, without
//! going through a command at all. "Tervin never sends anything the user did not
//! attach" is a promise about the Rust side; `connect-src` is the only thing that
//! makes it true of the JavaScript side as well, and nothing else in the repository
//! reads it.
//!
//! What the policy cannot do belongs next to what it does: it governs subresource
//! fetches and form submissions, not top-level navigation. Nothing asserted here stops
//! the webview being navigated away to a remote page. `activateLink` hands external
//! urls to the system browser rather than the webview, and Tauri offers a navigation
//! handler that this app does not yet install — a gap, stated rather than implied.
//!
//! Deliberately a static check on both files' own text. It reads the same bytes
//! `tauri-build` reads, which is what makes it an assertion about the shipped binary
//! rather than about a copy of the intent.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Every permission this capability grants, with the reason it is here.
///
/// The list is the audit. A grant with no caller is either dead or a feature nobody
/// wired up, and both are worth failing a build over — the same argument
/// `components/reachable.test.ts` makes about components, applied to the surface that
/// has security consequences.
const AUDITED: &[(&str, &str)] = &[
    (
        "core:default",
        "the frontend baseline: invoke, the IPC channel, path and event plumbing",
    ),
    (
        "core:event:allow-listen",
        "api.on() in ui/src/lib/api.ts, which every pushed event goes through",
    ),
    (
        "core:event:allow-unlisten",
        "the unlisten function api.on() returns, called on unmount",
    ),
    (
        "core:window:allow-start-dragging",
        "data-tauri-drag-region on the header in ui/src/App.tsx",
    ),
    (
        "opener:allow-open-url",
        "activateLink in ui/src/components/TerminalPane.tsx",
    ),
    (
        "dialog:allow-open",
        "chooseProject in ui/src/App.tsx, the only file picker in the app",
    ),
];

/// The schemes `activateLink` can actually produce.
///
/// `http` and `https` from the url pattern in ui/src/lib/links.ts, `http` again for a
/// detected port, and `mailto` for an email match. Anything else — `file:`, a custom
/// scheme registered by some other application — is not something Tervin asks for, so
/// it is not something the webview may ask for.
const EXPECTED_URL_SCOPE: &[&str] = &["http://*", "https://*", "mailto:*"];

fn capability_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("capabilities/default.json")
}

fn capability() -> serde_json::Value {
    let path = capability_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} could not be read: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

/// Each entry of `permissions`, which Tauri accepts as either a bare identifier or an
/// object carrying a scope.
fn permissions() -> Vec<serde_json::Value> {
    capability()["permissions"]
        .as_array()
        .unwrap_or_else(|| panic!("{} has no permissions array", capability_path().display()))
        .clone()
}

fn identifiers() -> BTreeSet<String> {
    permissions()
        .iter()
        .map(|entry| match entry {
            serde_json::Value::String(id) => id.clone(),
            other => other["identifier"]
                .as_str()
                .unwrap_or_else(|| panic!("a permission entry has no identifier: {other}"))
                .to_string(),
        })
        .collect()
}

#[test]
fn the_capability_file_grants_no_unbounded_path_glob() {
    let permissions = permissions();
    // Guard first: an empty read would make every assertion below vacuously true and
    // report success while checking nothing at all.
    assert!(
        !permissions.is_empty(),
        "{} parsed to no permissions — the scan is broken",
        capability_path().display()
    );

    assert!(
        !identifiers().contains("opener:allow-open-path"),
        "opener:allow-open-path is granted again. A capability scope is static and the \
         project root is not, so this grant cannot be narrowed to the project — it can \
         only be `**` or a fixed path. Use commands::open_path instead."
    );

    for entry in &permissions {
        let identifier = entry["identifier"].as_str().unwrap_or("<bare identifier>");
        for scope in entry["allow"].as_array().into_iter().flatten() {
            let Some(path) = scope["path"].as_str() else {
                continue;
            };
            assert!(
                path != "**" && path != "*",
                "{identifier} allows path {path:?}, which is every file the user can \
                 read. Name the paths, or move the decision to a command that knows \
                 the project root."
            );
        }
    }
}

#[test]
fn every_granted_permission_has_a_caller() {
    let granted = identifiers();
    let audited: BTreeSet<String> = AUDITED.iter().map(|(id, _)| id.to_string()).collect();

    let unaudited: Vec<&String> = granted.difference(&audited).collect();
    assert!(
        unaudited.is_empty(),
        "{:?} is granted to the webview and is not in the audited list in this test. \
         Add it with the file and function that calls it, or take the grant out.",
        unaudited
    );

    let stale: Vec<&String> = audited.difference(&granted).collect();
    assert!(
        stale.is_empty(),
        "{:?} is listed here but no longer granted. Remove it from AUDITED so the list \
         keeps describing the file.",
        stale
    );
}

#[test]
fn the_url_scope_admits_only_the_schemes_the_ui_produces() {
    let permissions = permissions();
    let open_url = permissions
        .iter()
        .find(|entry| entry["identifier"].as_str() == Some("opener:allow-open-url"))
        .expect(
            "opener:allow-open-url is granted without a scope, or not at all. Without a \
             scope the plugin denies every url, so `openUrl` silently stops working; \
             with `opener:default` it would quietly allow tel: as well.",
        );

    let allowed: Vec<&str> = open_url["allow"]
        .as_array()
        .expect("opener:allow-open-url has no allow list")
        .iter()
        .map(|scope| {
            scope["url"]
                .as_str()
                .unwrap_or_else(|| panic!("a url scope entry has no url: {scope}"))
        })
        .collect();

    assert_eq!(allowed, EXPECTED_URL_SCOPE);
}

// ---------------------------------------------------------------------------
// The content security policy.
// ---------------------------------------------------------------------------

/// The policy a packaged build is served.
const PRODUCTION: &str = "csp";

/// The policy that applies while the app is pointed at the Vite dev server. It never
/// reaches a user, which is exactly why it is the one that drifts.
const DEVELOPMENT: &str = "devCsp";

/// Sources that name somewhere on this machine, and the reason each one is that.
///
/// Anything not on this list and not a quoted keyword is a host, and a host is a way
/// off the machine — which is the whole question this file exists to answer.
const LOCAL_SOURCES: &[(&str, &str)] = &[
    (
        "ipc:",
        "Tauri's own IPC scheme. The webview resolves it through a custom scheme \
         handler registered by this process, so a request on it is answered inside the \
         process rather than on the network",
    ),
    (
        "http://ipc.localhost",
        "the same IPC channel, spelled the way platforms that cannot register a custom \
         scheme spell it. RFC 6761 reserves .localhost for loopback, so even a request \
         that got as far as the resolver could not leave the machine",
    ),
];

/// What the development policy adds, and the whole of what it may add.
const DEV_ONLY_SOURCES: &[(&str, &str)] = &[
    (
        "http://localhost:5173",
        "the Vite dev server, which is build.devUrl in this same file",
    ),
    (
        "ws://localhost:5173",
        "its hot-reload socket, on the port vite.config.ts pins with strictPort",
    ),
];

/// Every source in either policy that relaxes it, and what stops working without it.
///
/// JSON takes no comments and Tauri's config schema rejects an unknown key, so a `"//"`
/// sibling in `tauri.conf.json` would fail the build rather than document it. The
/// reasons live here instead, which is where a reader with a question about the policy
/// is already standing, and which fails when a token is added without one.
///
/// `'unsafe-eval'` used to sit in the development policy and does not any more.
/// Nothing in the dev pipeline evaluates a string: Vite's dev client, the React
/// Refresh runtime shipped by `@vitejs/plugin-react`, and React's own development
/// build contain no `eval` and no `new Function`. It was removed rather than
/// explained, because a token nobody can name a use for is a token nobody is
/// defending.
const UNSAFE_SOURCES: &[(&str, &str, &str, &str)] = &[
    (
        PRODUCTION,
        "script-src",
        "'wasm-unsafe-eval'",
        "@xterm/addon-image decodes sixel and iTerm2 inline images with a WebAssembly \
         module it compiles from bytes inlined in its own bundle. Under script-src \
         'self' alone the webview refuses to compile it and every image a program \
         draws in the terminal silently fails to appear. TerminalPane.tsx loads that \
         addon on every renderer but dom, and nothing else in the bundle touches \
         WebAssembly, so this token buys inline images and nothing else.",
    ),
    (
        PRODUCTION,
        "style-src",
        "'unsafe-inline'",
        "xterm.js styles itself with two <style> elements it creates at runtime: one \
         holding the row dimensions, one holding the theme's foreground, cursor, bold \
         and italic rules, which it rewrites whenever applyAppearance in \
         TerminalPane.tsx changes term.options.theme. \
         Terminal.open() builds a DomRenderer before any addon can replace it, so both \
         appear on every pane. Blocked, the dom renderer loses its default foreground \
         colour, its cursor and its bold and italic rules — and dom is where the \
         webgl → canvas → dom fallback puts a machine whose GPU has already crashed. \
         The alternative is a nonce, and xterm has no way to stamp one onto elements \
         it creates itself.",
    ),
    (
        DEVELOPMENT,
        "script-src",
        "'unsafe-inline'",
        "@vitejs/plugin-react injects the React Refresh preamble into index.html as an \
         inline <script type=module>. Blocked, window.$RefreshReg$ is never defined and \
         the first transformed module throws \"can't detect preamble\", so the dev app \
         opens a blank window. A packaged build is served the production policy, which \
         has no such token.",
    ),
    (
        DEVELOPMENT,
        "script-src",
        "'wasm-unsafe-eval'",
        "the same inline images as the production policy: the dev server serves the \
         same addon, and images that work in a build but not in development are a bug \
         report nobody can reproduce.",
    ),
    (
        DEVELOPMENT,
        "style-src",
        "'unsafe-inline'",
        "the same two xterm elements, plus Vite's dev client, which serves every \
         stylesheet by creating a <style> element rather than linking one. Blocked, \
         the dev app starts with no styling at all.",
    ),
];

fn config_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json")
}

fn policy(field: &str) -> String {
    let path = config_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} could not be read: {e}", path.display()));
    let config: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()));
    config["app"]["security"][field]
        .as_str()
        .unwrap_or_else(|| panic!("{} has no app.security.{field}", path.display()))
        .to_string()
}

/// `directive source source; directive source` split into directive and sources.
fn directives(field: &str) -> BTreeMap<String, Vec<String>> {
    let mut parsed = BTreeMap::new();
    for clause in policy(field).split(';') {
        let mut words = clause.split_whitespace();
        let Some(name) = words.next() else {
            continue;
        };
        // Directive names are case-insensitive; source expressions are not, so only
        // the name is folded.
        parsed.insert(
            name.to_ascii_lowercase(),
            words.map(|source| source.to_string()).collect(),
        );
    }

    // Guard first: a renamed field or a policy that stopped being one would make every
    // assertion below vacuously true and report success while checking nothing.
    assert!(
        !parsed.is_empty(),
        "app.security.{field} parsed to no directives — the scan is broken"
    );
    parsed
}

/// The sources in `directives` that name a host `local` does not vouch for.
///
/// Returned rather than asserted so a failure can name the directive too: `img-src
/// https://example.com` is a different mistake from the same host in `connect-src`,
/// and it is fixed in a different place.
fn hosts_reached(directives: &BTreeMap<String, Vec<String>>, local: &[&str]) -> Vec<String> {
    let mut reached = Vec::new();
    for (directive, sources) in directives {
        for source in sources {
            // 'self' is the document's own origin, which is the app's asset protocol in
            // a packaged build and the dev server under `tauri dev` — both on this
            // machine. Every other quoted expression ('none', 'unsafe-inline', a nonce,
            // a hash) names no host at all.
            if source.starts_with('\'') {
                continue;
            }
            // Bytes that are already inside the document. Neither opens a connection.
            if source == "data:" || source == "blob:" {
                continue;
            }
            if local.contains(&source.as_str()) {
                continue;
            }
            reached.push(format!("{directive} {source}"));
        }
    }
    reached
}

#[test]
fn the_production_csp_cannot_reach_off_this_machine() {
    let directives = directives(PRODUCTION);

    // The named claim first, so it stays exercised even if the sweep below is ever
    // narrowed. connect-src is what fetch, XHR, WebSocket, EventSource and sendBeacon
    // are checked against — every way the web content could send something by itself.
    let connect = directives.get("connect-src").unwrap_or_else(|| {
        panic!(
            "the production policy has no connect-src, so connections fall back to \
             default-src and this test is proving something weaker than its name"
        )
    });
    assert!(!connect.is_empty(), "connect-src has no sources at all");

    let local: Vec<&str> = LOCAL_SOURCES.iter().map(|(source, _)| *source).collect();
    let reached = hosts_reached(&directives, &local);
    assert!(
        reached.is_empty(),
        "the shipped policy lets the webview reach {reached:?}. Tervin sends nothing \
         the user did not attach, and this policy is where that stops being a promise \
         about the Rust side and becomes true of the JavaScript side."
    );

    // form-action is the one directive here that does not fall back to default-src:
    // leave it out and a submitted form posts wherever it likes, policy or no policy.
    // Nothing in ui/src renders a <form>, so 'none' costs nothing to hold.
    let form_action = directives
        .get("form-action")
        .map(|sources| sources.join(" "));
    assert_eq!(
        form_action.as_deref(),
        Some("'none'"),
        "form-action is missing or widened. A form submission is a way off this \
         machine that no other directive in this policy covers."
    );
}

/// The `connect-src` sources of one policy, as a set so two can be compared.
fn connect_sources(parsed: &BTreeMap<String, Vec<String>>, which: &str) -> BTreeSet<String> {
    parsed
        .get("connect-src")
        .unwrap_or_else(|| panic!("the {which} policy has no connect-src"))
        .iter()
        .cloned()
        .collect()
}

#[test]
fn the_development_csp_only_adds_the_vite_dev_server() {
    let development = directives(DEVELOPMENT);
    let production = directives(PRODUCTION);

    let added: BTreeSet<String> = connect_sources(&development, "development")
        .difference(&connect_sources(&production, "production"))
        .cloned()
        .collect();
    let expected: BTreeSet<String> = DEV_ONLY_SOURCES
        .iter()
        .map(|(source, _)| (*source).to_string())
        .collect();
    assert_eq!(
        added, expected,
        "development connect-src reaches somewhere production does not, and it is not \
         the dev server. A third origin here is a decision, not a detail: name it in \
         DEV_ONLY_SOURCES with what it is for."
    );

    let mut local: Vec<&str> = LOCAL_SOURCES.iter().map(|(source, _)| *source).collect();
    local.extend(DEV_ONLY_SOURCES.iter().map(|(source, _)| *source));
    let reached = hosts_reached(&development, &local);
    assert!(
        reached.is_empty(),
        "the development policy reaches {reached:?}. Development is where a host gets \
         added to try something and then stays."
    );
}

#[test]
fn every_unsafe_source_says_what_breaks_without_it() {
    let mut present: BTreeSet<(String, String, String)> = BTreeSet::new();
    for field in [PRODUCTION, DEVELOPMENT] {
        for (directive, sources) in directives(field) {
            for source in sources {
                // Matched on the whole word rather than an 'unsafe- prefix, because
                // 'wasm-unsafe-eval' does not start with one and is the token most
                // likely to be added without an argument for it.
                if source.contains("unsafe") {
                    present.insert((field.to_string(), directive.clone(), source));
                }
            }
        }
    }

    let documented: BTreeSet<(String, String, String)> = UNSAFE_SOURCES
        .iter()
        .map(|(field, directive, source, _)| {
            (
                (*field).to_string(),
                (*directive).to_string(),
                (*source).to_string(),
            )
        })
        .collect();

    assert_eq!(
        present, documented,
        "the relaxations in tauri.conf.json and the reasons in UNSAFE_SOURCES have \
         drifted apart. Each of these weakens the policy for the whole app, so each \
         one owes the next reader a sentence saying what stops working without it."
    );

    for (field, directive, source, reason) in UNSAFE_SOURCES {
        assert!(
            !reason.trim().is_empty(),
            "{field} {directive} {source} carries no reason. Nothing here can check \
             that the prose is true, but an empty one is checkable and is always wrong."
        );
    }
}
