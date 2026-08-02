# Verifying Tervin yourself

> **In development.** Tervin v0.1.0 is the first release. This document is how to check it
> for yourself, and it is honest about what the automated suite does and does not prove. See
> the [status notice](../README.md#status) in the README.

Every command below was run on a clean checkout before being written down. None of it is
copied from memory.

**The thing worth understanding first:** 999 tests pass, and that is not the same as the
product being correct. Tervin is a GUI driving real PTYs. The suite deliberately avoids
mocking the thing under test, which is why it catches a great deal, but a test cannot tell
you whether the terminal *feels* right, whether an agent Thread reads clearly, or whether a
theme is legible. §3 is the part only you can do.

---

## 1. Setup

```sh
git clone git@github.com:QuintinBotes/tervin.git
cd tervin
pnpm install --frozen-lockfile
```

What you need, with the versions this was verified against:

| Tool | Verified with | Why |
| --- | --- | --- |
| Node | 22.21.1 | The UI build and test runner. 20+ should work. |
| pnpm | 10.32.1 | Pinned in `packageManager`; `npm` will not reproduce the lockfile. |
| Rust | 1.97.1 | The workspace. Stable, nothing nightly. |
| Xcode CLI tools | any recent | Tauri links against system frameworks. |

`--frozen-lockfile` matters. Without it a resolver difference can hide a real breakage.

---

## 2. The automated suite

Run all of it. Roughly two minutes cold, well under one warm.

```sh
cargo test --workspace          # 680 tests
pnpm exec vitest run            # 319 tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
pnpm exec tsc --noEmit
```

Expected, verified on a clean checkout at v0.1.0:

```
rust: 680 passed, 0 failed
Test Files  9 passed (9)
     Tests  319 passed (319)
```

If any number is *lower* than that, something is wrong: a suite has been skipped or a file
failed to load. A silently smaller total is the failure mode that caught me out during
development, when a merge dropped a whole `describe` block and every remaining test still
passed.

### What these tests actually exercise

This is not a mocked suite, which is why it is worth running:

- **Real PTYs driving real programs.** The terminal tests spawn actual `vim` and `less` and
  assert on what comes back, so a change that breaks a full-screen program fails here rather
  than in your hands.
- **Real temporary git repositories** for the git service, not a fake.
- **A real Python ACP agent over a real pipe** for the ACP adapter.
- **A hand-written chunked-SSE server** for the local-model adapter, because a mocked HTTP
  client would not have caught the keep-alive bug that one did.
- **Real captured bytes** from `claude` and from `codex exec --json` for the agent parsers,
  rather than fixtures shaped to fit the parser.
- **A real TCP listener and a real closed port** for SSH reachability.
- **Real generated SSH keys** for the agent-key fingerprint comparison, because a fixture
  cannot prove `ssh-keygen` and `ssh-add` agree on a fingerprint.
- **A database built with the old schema**, then opened with current code, for the migration.
- **The shipped `claude` binary itself** for the instruction-file readership table. Which
  files a runtime reads is a claim about someone else's software, so it was read out of the
  binary rather than taken from documentation. The table then exists twice, in Rust and in
  the UI, and a generated fixture both sides assert against means neither can drift
  silently. Verified by breaking each side in turn and confirming the test fails.

### The one test that needs a real Claude Code

The permission gate has a live test that is skipped by default because it invokes the real
CLI:

```sh
TERVIN_LIVE_CLAUDE=1 cargo test -p agent-runtime the_real_cli_honours_a_refusal
```

This is the test that proves the `PreToolUse` hook genuinely blocks a tool rather than being
politely ignored. If you only run one thing from this document, and you have `claude`
installed, run this.

### Benchmarks

```sh
cargo bench --workspace
```

Three targets: `osc_scan` (terminal-core), `fuzzy_match` (file-index), `classify`
(rules-engine). Numbers and the budgets they are measured against are in
[PERFORMANCE.md](PERFORMANCE.md). Expect a few minutes.

---

## 3. Running it, which is the part that matters

### Development build, with hot reload

```sh
pnpm app
```

Vite serves the UI and Tauri opens a window against it. UI edits reload without restarting
the shell in your panes. This is the mode to use for poking at things.

### Release build

```sh
pnpm app:build
```

Verified: finishes in about a minute warm, and produces `target/release/tervin` plus
`target/release/bundle/macos/Tervin.app`. Run the binary directly, or open the `.app`.

To skip the DMG and just get a runnable app:

```sh
pnpm exec tauri build --bundles app --no-bundle
```

**On Gatekeeper:** a build you made yourself is not quarantined and opens without any
dialog. A `.dmg` downloaded through a browser *is* quarantined, because macOS applies
`com.apple.quarantine` in the downloading application, not in the kernel.

That last detail is the whole reason Tervin is not signed and will not be. Quarantine comes
from the downloader, so `curl`, the installer script, and `npm` never set it: they set only
`com.apple.provenance`, which Gatekeeper does not act on. Verified with `xattr` on real
downloads through each route. So the routes Tervin recommends already open without a dialog,
and a $99/year Apple Developer ID would buy a better experience for exactly one route, the
browser-downloaded `.dmg`, which is the route the README names as the worst one. Signing is
listed in [COMPETITIVE-SPEC.md](COMPETITIVE-SPEC.md) §5 among the things to refuse to build.

Verify a download yourself, which is the actual security boundary:

```sh
shasum -a 256 -c SHA256SUMS.txt
```

---

## 4. The manual checklist

Nothing here is covered by the automated suite. Each item says what to do and what correct
looks like, so a wrong result is unambiguous.

### 4.1 The terminal is a real terminal

- [ ] `vim`, then edit and `:q`. Cursor, colours and redraw all correct.
- [ ] `htop` or `top`. Full-screen redraw is smooth and resizing the pane reflows it.
- [ ] `less /usr/share/dict/words`, scroll with the keyboard and the mouse wheel.
- [ ] `yes | head -c 20000000` then Ctrl-C. Large output does not lock the window, and the
      interrupt is honoured.
- [ ] A prompt with glyphs (powerlevel10k, starship). No boxes or gaps.
- [ ] `printf 'héllo 日本語 🎉\n'`. Wide characters and emoji align correctly.

### 4.2 Blocks

- [ ] Run `ls`. A Block appears with exit 0.
- [ ] Run `false`. It appears as failed.
- [ ] Run `cargo test` in a Rust project. Test counts are parsed and shown.
- [ ] Run something that prints an error with a file and line. The path is clickable.
- [ ] Bookmark a Block, then find it again in History.
- [ ] Search History for a command from an hour ago.
- [ ] **Then quit and relaunch.** The Blocks are still there. This is the one that proves
      persistence rather than in-memory state.

### 4.3 Session restore

- [ ] Split into three panes, `cd` somewhere different in each, quit, relaunch.
- [ ] Layout, directories and recent output all return.
- [ ] Each pane shows *"restored from your last session; nothing above is running"*. If that
      line is missing, that is a bug: a restored screen must never look live.
- [ ] Settings, turn restore off. It says it deleted what was saved. Relaunch gives one
      empty pane.

### 4.4 Agents, and the honesty claims

- [ ] Agents surface, pick Claude Code, send a prompt. Events stream into the Thread.
- [ ] The capability strip states whether Tervin can gate the session. Read it. It should say
      Tervin only ever adds a refusal and never approves on the runtime's behalf.
- [ ] Ask the agent to run something a Tervin Rule forbids. **It should be stopped before it
      runs**, not after. This is the central claim of the whole product.
- [ ] A command the agent runs appears as a Block, marked `agent`.
- [ ] With Claude Code, that Block shows **no exit code**, and says *"no exit status
      reported"*. That is correct: Claude Code reports success or failure and never a status,
      so a number there would be invented.
- [ ] **Open a pane and type `claude` yourself.** A Thread appears, titled after your first
      prompt. It has no composer, and explains that Tervin cannot drive a session it did not
      start. Type in the pane instead.
- [ ] History, Prompts. Your pane-typed prompt is searchable.

### 4.4b Instructions this project already carries

- [ ] Settings, Agents. The top block lists any `AGENTS.md`, `CLAUDE.md`, Cursor or Copilot
      instructions in the project, with the directory each governs and its size.
- [ ] Switch *Reading as* between Claude Code and Codex. **`CLAUDE.md` changes from "in
      force" to "not read".** That is the whole point of the panel: the file did not move,
      the runtime did.
- [ ] Hover "in force". It states how that is known, naming the version it was verified
      against, rather than simply asserting it.
- [ ] Pick *A local model*. Everything becomes "Tervin can pass in", because a
      conversational endpoint reads no files. Nothing says "in force", because Tervin has
      not passed anything in yet.
- [ ] In a project with no instruction files, it says so plainly and suggests `AGENTS.md`
      rather than showing an empty box.

### 4.5 The pickers

- [ ] ⌘J after visiting a few directories. Ranked by how often and how recently.
- [ ] ⌘R. Every command you have run. One that failed last time is flagged.
- [ ] ⌘⇧S, save `echo {{name}}`, use it. You are asked for `name` and shown the filled line.
- [ ] In all three: **Enter fills the pane and does not run.** Verify by pressing Enter and
      confirming nothing executed until you pressed Return again.
- [ ] ⌘K palette finds actions, panes and Blocks.

### 4.6 Connections

- [ ] ⌘⇧O. Your `~/.ssh/config` hosts are listed.
- [ ] A host whose key is not in the agent shows **key not loaded**. Check with `ssh-add -l`.
- [ ] Press *check* on a real host. It reports "N ms to connect", never "latency", because
      SSH does not report round-trip time.
- [ ] A host behind `ProxyJump` says **not checkable**, not unreachable.
- [ ] Open an SSH host. The pane connects and is marked remote.

### 4.7 The smaller things

- [ ] Scroll three screens up in a long build log. A header pins the command that produced
      it. Scroll back down and it disappears.
- [ ] Open `vim`. The header does not appear over it.
- [ ] Paste a multi-line command. You are warned before it runs.
- [ ] Switch theme while `vim` or a TUI is running. A program that subscribes to colour-scheme
      changes restyles itself.
- [ ] Settings, all 15 themes. Every one is legible, including the light ones.
- [ ] Tab strip on each of the four sides. Nothing overlaps.
- [ ] `⌘=` / `⌘-` / `⌘0` for font size.
- [ ] Toggle the file explorer, click a file, its path is typed into the pane.

### 4.8 Things that should fail gracefully

Worth doing, because this is where most software is careless:

- [ ] Point an agent profile at a binary that does not exist. It should say which binary,
      not throw.
- [ ] Kill a pane's shell from outside (`kill -9`). The pane reports it exited.
- [ ] Open Tervin in a directory that is not a git repository. Review says so; nothing errors.
- [ ] Open it in a very large repository. It stays responsive while the index builds.
- [ ] Revoke network access, then use an agent. The failure explains itself.

---

## 5. If you find something

The most useful report includes which of the above it was, what you expected, and what
happened. If it is a terminal-fidelity problem, the program and its exact output matter more
than a screenshot.

Two things to check first, because both have bitten during development:

1. **Is the shell hook loaded?** Settings, Shell integration says so per pane. Without it
   there are no Blocks, and that looks like a bug in Blocks.
2. **Which agent profile is active?** Two profiles can drive the same runtime against
   different installs and different accounts. The Thread header says which.
