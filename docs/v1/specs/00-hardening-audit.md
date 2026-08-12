# Spec 00 — Security & hardening audit

**Runs first.** Every later spec builds on this surface, so a weakness fixed here is
fixed once rather than multiplied.

## Context

Tervin's threat model is already written (`SECURITY.md`) and unusually candid: the hook
socket is owner-only, there is no listening network port, shell integration never writes
a file the user owns, and ACP filesystem access is confined after symlink resolution.
What has *not* happened is a deliberate audit of the surfaces that were inherited rather
than designed — the Tauri capability allowlist, the CSP, dependency provenance, and the
handful of places production code can panic.

One finding is already concrete: `crates/tervin-app/capabilities/default.json` grants

```json
{ "identifier": "opener:allow-open-path", "allow": [{ "path": "**" }] }
```

The file's own description says Tauri 2 denies every unlisted command, "so this file is
the app's actual capability surface — not documentation of it." A `**` path glob means
any compromised or buggy frontend path can ask the OS to open any file on disk.

## Slices

### 00.1 — Tighten the Tauri capability surface
`crates/tervin-app/capabilities/default.json`

Replace the `**` opener glob with the narrowest set that keeps link activation working.
`ui/src/components/TerminalPane.tsx:864-903` is the only caller: it opens URLs, file
paths detected in output, and `http://localhost:N` for ports. Scope `allow-open-path` to
the project root and the user's home, and route anything outside through a confirmation.
Audit every other entry for whether it is actually invoked — `core:webview:allow-internal-toggle-devtools`
in a release build deserves a decision, not an inheritance.

*Exit:* a test asserts the capability file grants no unbounded path glob; opening a
detected path inside the project still works; opening one outside prompts.

### 00.2 — Review the CSP
`crates/tervin-app/tauri.conf.json`

`style-src 'self' 'unsafe-inline'` is present in the production CSP. Establish whether
it is load-bearing (xterm.js injects styles; themes are applied as CSS variables) and
either remove it or record why it must stay. `script-src` is already clean. Confirm
`connect-src` cannot reach anything but IPC — this is what mechanically enforces the
"nothing leaves your machine" promise, so it deserves a test, not a reading.

*Exit:* a test parses `tauri.conf.json` and asserts `connect-src` contains no external
origin. Any `unsafe-*` that survives carries a comment explaining what breaks without it.

### 00.3 — Audit production panics
19 `.unwrap()`/`.expect()` calls exist outside test modules across ~27k lines of
production Rust. That is already low. Classify each: a genuine invariant gets a comment
saying why it cannot fail; anything reachable from untrusted input — PTY bytes, agent
JSON, an SSH config, a parsed transcript — becomes a handled error.

`panic = "abort"` is set in the release profile, so a reachable panic in a PTY reader
thread takes the whole app down with it.

*Exit:* every surviving unwrap in `crates/*/src` carries a justification comment. Add a
clippy lint or a test that fails on a new unjustified one.

### 00.4 — Secret handling review, and the one secret already stored in plaintext

**A concrete finding, not a review item.** `ui/src/components/SettingsPanel.tsx:707`
collects a model-endpoint API key with `type="password"` — the interface is explicitly
declaring it a secret — and `crates/agent-runtime/src/profile.rs:204-219` writes profiles
to `~/Library/Application Support/tervin/agents.toml` with `toml::to_string_pretty`, in
the clear, at whatever the umask gives.

So the current position is not "Tervin refuses to hold credentials". It holds one, in
plaintext, in a file created without an explicit mode.

The fix, which strengthens `SECURITY.md` §5 rather than rewriting it:

- **Any secret the user hands Tervin goes to the macOS Keychain**, with only a reference
  in `agents.toml`. That is "reference the keychain, do not become one" applied
  correctly.
- **SSH passphrases stay refused.** `ssh-agent` with `--apple-use-keychain` already owns
  that, natively and better. Reporting whether a key is loaded is the right behaviour and
  stays.
- **No vault surface.** Tervin does not become a credential manager. §5 holds.
- Set `0600` explicitly on `agents.toml` and any other config carrying user input,
  rather than inheriting the umask — the same reasoning `paths.rs:61` already applies to
  `runtime_dir()`.

*Exit:* a test asserts no API key appears in `agents.toml` after one is entered. A test
asserts the config file's mode bits. `SECURITY.md` describes the Keychain use.

Three further places handle material that should never be persisted or exported:

- `crates/agent-runtime/src/mcp.rs` — an MCP entry routinely holds an API key in `env`.
  Discovery already reports server *names* only; assert that with a test rather than
  relying on it.
- `crates/agent-runtime/src/handoff.rs` — Context Bundles must not carry environment
  values. The omission list already exists; extend it to prove env is excluded.
- `crates/agent-runtime/src/profile.rs` — `ACCOUNT_SELECTING_VARS` and
  `INHERITED_SESSION_VARS` are scrubbed. Confirm the scrub is applied on every launch
  path, including the pane-agent and Codex ones.

Also confirm `TERVIN_LOG` at `debug` cannot print an env value or a prompt body.

*Exit:* a test per site, each asserting a known secret-shaped value does not appear in
the output.

### 00.5 — SQL and FTS5 review
`crates/block-engine/src/store.rs`

Two `format!`-built statements exist (`store.rs:270` interpolating a table name into a
`pragma_table_info` call, and `store.rs:1030` building `?` placeholders). Both look
benign; confirm the table name is a compile-time constant and the placeholder count is
derived from a slice length, then add a comment so neither is "simplified" into a
vulnerability later. Re-verify FTS5 input sanitisation with an adversarial test — an
FTS5 query is its own grammar and a bare `"` is a syntax error at best.

*Exit:* an adversarial-input test over the search path with quotes, `NEAR`, `*`, and a
`--` comment sequence.

### 00.6 — Dependency and supply-chain posture
Add `cargo audit` and `cargo deny` to `.github/workflows/ci.yml`, plus `pnpm audit`.
Pin GitHub Actions to commit SHAs rather than tags — a moving tag on a third-party
action is arbitrary code execution in CI with repository credentials. Dependabot is
already configured (`.github/dependabot.yml`); make sure it covers all three ecosystems.

*Exit:* CI fails on a known-vulnerable dependency. Every `uses:` in every workflow is a
SHA.

### 00.7 — Hook socket and IPC hardening
`crates/agent-runtime/src/claude/hooks.rs`

The socket is already `0600` in a `0700` directory, set explicitly rather than left to
the umask, and unparseable input is refused rather than allowed. Add the tests that
prove it: assert the mode bits after creation, assert a malformed payload is refused,
and assert the socket is removed on shutdown so a stale path cannot be squatted.

Separately, audit the 73 Tauri IPC commands for ones that take a path and do not
validate it against the project root.

*Exit:* mode-bit and malformed-payload tests pass. A written list of every
path-accepting IPC command and its validation.

### 00.8 — Protected-folder and index bounds re-check
`crates/file-index` already caps at 200,000 entries and depth 24, and
`default_project_root` avoids `~` because rooting there triggered macOS permission
prompts. Confirm the bounds are enforced on every entry point and that a symlink loop
cannot defeat the depth cap.

*Exit:* a test with a symlink cycle terminates. A test asserts the entry cap holds.

## Verification for the whole spec

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo audit && cargo deny check
pnpm exec vitest run && pnpm audit
```

Plus one manual pass: launch the app, click a file link inside the project (opens), and
one outside it (prompts).
