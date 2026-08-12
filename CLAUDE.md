# CLAUDE.md

Guidance for Claude Code and other coding agents working in this repository.

Tervin is an agent-native terminal workspace: a correct, fast terminal first, with
Blocks (structured command units), Threads (provider-neutral agent event streams),
and Tervin Rules (risk classification and permission gating) on top.

**Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) before any substantial change.**
It explains why the boundaries are where they are, which will stop a well-intentioned
refactor from removing a constraint that exists on purpose.

## Commands

```sh
npm install
npm run app              # dev app, hot reload (tauri dev)
npm run app:build        # .app and .dmg

cargo test --workspace   # Rust tests, including real PTYs and real subprocesses
npx vitest run           # UI tests
npm run typecheck        # tsc --noEmit
cargo fmt && cargo clippy
```

Requires Rust 1.82+, Node 20+, Unix-like OS. macOS is the only tested platform.

Tests needing network, credentials, or a paid API are gated behind an environment
variable and skipped by default:

```sh
TERVIN_LIVE_CLAUDE=1 cargo test -p agent-runtime -- the_real_cli_honours_a_refusal
```

## Layout

Ten Rust crates and one React app. The dependency direction is strictly one-way: no
crate below `tervin-app` knows about the UI, and no crate knows which agent is
running except the adapter for it.

```
crates/tervin-core        ids, the event vocabulary, capabilities, risk, paths
crates/terminal-core      PTY, OSC/CSI scanning, alternate-screen detection
crates/shell-integration  zsh/bash/fish/pwsh hooks, alias expansion, injection
crates/block-engine       commands → Blocks, diagnostics, tests, SQLite + FTS5
crates/git-service        porcelain v2, hunk-level diffs and apply
crates/file-index         gitignore-aware walk, fzf-style fuzzy matching
crates/session-manager    shells, SSH config, tmux/zellij, serial
crates/rules-engine       risk classification, policy, approvals, audit
crates/agent-runtime      the AgentRuntime interface and every adapter
crates/tervin-app         Tauri host, IPC, application state
ui/                       React 19 + xterm.js workspace
```

`tervin-core` depends on none of the others by design. Everything shared — the event
names, `Tier`, `CapabilityLevel`, `RiskAssessment` — lives there, so a change to the
vocabulary is a compile error everywhere it matters rather than a runtime mismatch.

## The honesty rule

> **Tervin never claims a capability it does not have.**

This is not a style preference; it is the product. Code that makes Tervin look more
capable than it is gets declined even when the code is good.

- **Do not upgrade a capability on the strength of configuration.** A capability
  becomes `Supported` when something has been *observed working*, not when it has been
  set up. If you cannot observe it, it stays `Partial` with a note saying why.
- **`Unsupported` requires a reason, `Partial` requires a note.** The type enforces
  this. Do not write "not supported": write what the user should do instead.
- **Never present an observation as a gate.** If Tervin can see an action but not stop
  it, `enforceable` is false and the UI says "observed", not "approved".
- **Do not drop what you cannot classify.** Emit `runtime.unclassified` and keep the
  raw payload. Dropping it makes the timeline quietly incomplete; guessing makes it
  quietly wrong.
- **Never answer `allow` through a Claude Code hook** — only `deny` or `defer`. `allow`
  skips the runtime's own checks.
- **Never add a code path that sends anything the user did not attach.** No scrollback,
  no file contents, no environment. This holds for local endpoints too: they feel safe,
  which is what makes the temptation real.

## The testing standard

> **Test against the real thing, or say plainly that you did not.**

Mocks are allowed for things that are genuinely external and slow, but never for the
thing under test. Writing a terminal feature? Drive a PTY. Writing a protocol adapter?
Speak the protocol over a real pipe or socket. In practice that means real `vim`,
`less`, `zsh`, `/bin/sh`; real temporary Git repositories; a real ACP agent over a real
pipe; a hand-written HTTP server with real chunked SSE.

- **Name a test as a claim about behaviour.** `a_denied_permission_is_actually_denied`,
  not `test_permissions`. The name should tell a reader what breaks if it fails.
- **Assert the thing that matters, not the thing that is easy.** "The gate ran" is
  weak. "The command did not execute and the agent was told why" is the claim.
- **Tests that need an absent binary return early rather than failing.** A contributor
  without `vim` installed should not see a red suite.
- **If something could not be verified, write that down.** Unverified is always
  acceptable; discovering it later is not.

## Load-bearing invariants

Breaking one of these looks harmless and is not.

- **State that changes per frame does not live in React.** Scrollback lives inside each
  xterm instance. A store would re-render on every frame of output.
- **Terminal bytes never become JSON.** Output crosses IPC as raw binary and arrives as
  an `ArrayBuffer`.
- **Panes are a tree, not a list.** Splitting one half of an existing split is the
  second thing anyone does in a terminal, and a list cannot express it.
- **Renderer fallback survives a GPU crash.** A marker is written before WebGL is
  created and cleared only after a frame paints; a surviving marker steps down
  `webgl → canvas → dom` on next launch.
- **Unhandled keys reach the terminal.** `runAction` returns a bool, and only a handled
  action calls `preventDefault`. This is what keeps `vim` and `emacs` usable, and it is
  why the keymap is data with context scoping rather than a chain of `if` statements.
- **Overlays take the keyboard and the terminal gives it up.** `overlayOpen()` is the
  single guard. Without it, `Return` in an approval sheet runs a command instead of
  answering a question about running one.
- **The OSC scanner is hand-rolled and non-destructive**, markers can split across
  reads, and the alternate screen pauses Block capture. See ARCHITECTURE.md before
  touching `terminal-core`.
- **`LaunchConfig.env` uses empty values as removals** — an empty var is an empty path,
  not an absent one.

## Style

- `cargo fmt` and `cargo clippy` before pushing.
- **Comments explain why, never what.** `// increment the counter` is noise. `// A
  killed process has no exit status of its own` is the reason the line exists.
- **Constants carry their reasoning.** Every bound in this codebase has a comment
  explaining the number. Adding one without justification will be asked about.
- **Match the surrounding code.** Comment density, naming, and idiom vary a little by
  crate. Follow the file you are in.
- **One concern per pull request.** A refactor and a fix in the same diff cannot be
  reviewed. Say what you verified and how — "tested manually" is not a verification.

## Interface rules

Full system in [docs/DESIGN.md](docs/DESIGN.md). The instant rejects:

> Gradients · Glassmorphism · Purple-blue AI styling · Glowing orbs · **Emoji** · Big
> rounded cards · Icons in coloured circles · Chat bubbles · Dashboard card clutter ·
> Looping animation · Colour as decoration · Fake feature parity · Silent disabled
> controls · Body text below 12px

Copy is precise, candid, calm, technical. "Agent is waiting for approval", not "AI
needs your attention!". "23 passed, 1 failed", not "Almost there!". Never *magical*,
*revolutionary*, *supercharge*, *seamless*, *AI-powered*. Never imply certainty an
agent does not have — show plan, command, files, diff, test result, evidence.

## Status

Pre-1.0 and in development. Local data formats are not stable; the SQLite schema gains
columns as features land. macOS is the only exercised platform — Linux and Windows are
not claimed. There is no compatibility to preserve yet, which makes this the best time
to replace a decision that is wrong.

## Further reading

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — the crate graph and the decisions that were hard to get right
- [docs/DESIGN.md](docs/DESIGN.md) — tokens, geometry, and what the interface may not do
- [docs/AGENTS.md](docs/AGENTS.md) — using agents: runtimes, permission status, MCP, handoff
- [docs/COMPETITIVE-SPEC.md](docs/COMPETITIVE-SPEC.md) — what Tervin lacks against each terminal, and what it should refuse to build
- [docs/PERFORMANCE.md](docs/PERFORMANCE.md) — measured throughput for the hot paths
- [docs/TESTING.md](docs/TESTING.md), [docs/MANUAL-TEST.md](docs/MANUAL-TEST.md) — what the tests prove and what they do not
- [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md)
