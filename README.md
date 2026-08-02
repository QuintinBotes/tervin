<div align="center">

<img src="assets/tervin-mark-dark.svg" alt="Tervin" width="72">

# Tervin

**The agent-native terminal workspace.**

A terminal that treats coding agents as first-class inhabitants — and never lies to
you about what they are doing.

</div>

---

## What it is

Tervin is a desktop terminal for the way people actually work now: a shell in one
pane, an agent in another, and a real need to know which of them just changed your
files.

It is a **real terminal first**. `vim`, `less`, `tmux`, `ssh`, oh-my-zsh,
powerlevel10k, Sixel images, bracketed paste, mouse reporting — all of it works,
because a terminal that is 95% correct is a widget that looks like a terminal. There
are tests that drive real `vim` and real `less` through a real PTY to prove it.

On top of that it adds three things a normal terminal cannot:

**Blocks.** Every command becomes a unit with its command, output, exit code,
duration, diagnostics, and test results — searchable months later.

**Threads.** Every agent, whichever vendor, normalises into one provider-neutral
event stream: plan, files read, files changed, commands run, tests, cost. Adding a
new agent never touches a view.

**Tervin Rules.** Risk classification and, where the runtime allows it, a real
pre-execution gate. Approving `rm -rf build` never approves `rm -rf /`.

## The one promise

> **Tervin never claims a capability it does not have.**

This is the constraint that shapes the whole codebase, and it cuts against making
demos look good.

- A risk assessment carries `enforceable: bool`. When Tervin can *see* an action but
  not *stop* it, the UI says "observed", not "approved".
- `native_permission_bridge` stays `Partial` until a gate has genuinely fired.
  Installing a hook is not evidence it works — Claude Code silently ignores settings
  files that fail validation, so a broken gate looks exactly like no gate.
- An event the adapter cannot classify becomes `runtime.unclassified` and keeps its
  raw payload. Dropping it would make the timeline quietly incomplete; guessing would
  make it quietly wrong.
- `bypassPermissions` is deliberately absent from the offered modes. A one-click way
  to disable every check cannot be reconciled with telling you your actions are
  reviewable.
- Nothing leaves your machine that you did not attach. There is no code path that
  ships scrollback, files, or environment to a provider — the privacy promise is
  enforced by there being no other way in.

## Agents

| Runtime | How | Real permission gate? |
| --- | --- | --- |
| **Claude Code** | `stream-json` on stdio, plus a `PreToolUse` hook | **Yes** — verified against the real CLI |
| **Any ACP agent** — Gemini CLI, GitHub Copilot CLI, Claude Code via bridge, and 25+ others | Agent Client Protocol over stdio | **Yes** — the agent blocks waiting for the answer |
| **LM Studio, Ollama, vLLM, llama.cpp** | OpenAI-compatible HTTP | N/A — answers, cannot act |
| **Codex, Aider, OpenCode, Cursor Agent** | Managed pane, full terminal fidelity | No, and it says so |

Anything that speaks ACP or the OpenAI dialect can be added from Settings without a
release. That is the point of integrating with protocols rather than vendors.

### The two gates are not equally strong, and Tervin says which is which

Under **ACP**, the agent sends `session/request_permission` and *waits*. Deny actually
denies.

Under **Claude Code's hooks**, Tervin registers a `PreToolUse` hook and answers over a
Unix socket. A refusal blocks the tool before it runs — but any exit code other than 2
is non-blocking, so if Tervin becomes unreachable the action proceeds. The gate fails
open, the session's permission text says so, and the hook prints
`This tool call was NOT checked against Tervin Rules` rather than failing silently.

Tervin also **never answers `allow`** through a hook — only `deny` or `defer`. `allow`
would skip the runtime's *own* checks, and a safety feature that quietly disables
another safety feature is not one.

## `cd` knows where you have been

⌘J opens a picker over every directory a pane has sat in, ranked by how often you go there
and how recently — then by what you typed. It fills in `cd` and leaves the newline to you.

Not bound to Tab: zsh and fish completion is better than anything Tervin would write for
arbitrary commands, and taking Tab would replace something good with something worse.

## It reopens where you left off

Tabs, splits, each pane's directory and its recent output come back on launch. The
processes do not — they exited with the app — so each pane starts a fresh shell below its
old output, under a line saying exactly that. A restored screen that looked live would be
worse than no restore at all.

Saved output is only returned to a pane running the same program, so a local shell's
history can never reappear inside an SSH session. It ages out on the same retention window
as agent history and is deleted as soon as you switch the setting off.

## Agents you start yourself

Open a pane, type `claude`, and it becomes a Thread — titled after your prompt, with
the replies, tool calls and file changes recorded, and searchable afterwards. Tervin
reads the escape sequence Claude Code already emits and the transcript it already
writes, so there is nothing to install and nothing to configure.

Such a session is read-only: Tervin cannot send a prompt or answer a permission
request for a process it did not spawn, and says so rather than showing a composer
that does nothing. Launch from the Agents surface if you want Tervin Rules to gate it.

## Context handoff

Because every Thread is the same event stream, work can move between agents. A
**Context Bundle** turns a Thread into a briefing another agent can read: the task,
the plan, files touched, commands and their exit codes, tests, open problems, and
what was refused.

It leaves out reasoning traces (another model reads a predecessor's thinking as
established fact), full command output, and anything not in the event stream — and it
*says* what it left out, so nothing is assumed.

## Installing

```sh
npx tervin
```

That is the route to prefer, and not only for convenience. macOS applies its
`com.apple.quarantine` attribute in the *downloading application* — a browser sets it,
`curl` and Node do not. So a build fetched by `npx` opens normally, while the identical
file downloaded through a browser hits Gatekeeper's "unidentified developer" wall and
needs approving in System Settings. Checksums are baked into the published npm package,
so npm is what vouches for the binary.

`npx tervin --install` copies it into `/Applications`; `--where` prints the cached
bundle; `--clean` removes it.

### Other routes, and what each costs you

| Route | Gatekeeper prompt? | Notes |
| --- | --- | --- |
| `npx tervin` | **No** | Nothing to approve. Needs Node 20+. |
| `brew install --formula QuintinBotes/tervin/tervin` | **No** | Compiles locally, so nothing is quarantined. Needs a Rust and Node toolchain and a few minutes. |
| Build from source | **No** | Same reason. |
| `brew install --cask QuintinBotes/tervin/tervin` | Yes | Homebrew marks cask downloads quarantined. One-time approval. |
| `.dmg` from GitHub Releases | Yes | Browser downloads are quarantined. One-time approval. |

Signing and notarising with an Apple Developer ID would remove the prompt from the last
two rows. It is not required for any of the others, and the release tooling reports
plainly when a build is unsigned rather than leaving you to discover it at launch.

## Building it yourself

Requires Rust 1.82+, Node 20+, and a Unix-like OS (macOS is the tested platform).

```sh
npm install
npm run app          # development, with hot reload
npm run app:build    # a .app and .dmg
```

Run the tests — all of them are real, none of them mock the thing under test:

```sh
cargo test --workspace   # 456 tests, including real PTYs and real subprocesses
npx vitest run           # 80 UI tests
```

A few need a live dependency and are opt-in:

```sh
TERVIN_LIVE_CLAUDE=1 cargo test -p agent-runtime -- the_real_cli_honours_a_refusal
```

## Documentation

- **[ARCHITECTURE.md](docs/ARCHITECTURE.md)** — the crate graph, why the boundaries
  are where they are, and the decisions that were hard to get right.
- **[DESIGN.md](docs/DESIGN.md)** — the design system: tokens, geometry, and the
  rules about what the interface may not do.
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — how to work on this, including the
  testing standard, which is stricter than usual for a reason.
- **[AGENTS.md](docs/AGENTS.md)** — using agents: picking a runtime, multiple
  accounts, reading the permission status, MCP, and handing work between agents.
- **[PERFORMANCE.md](docs/PERFORMANCE.md)** — measured throughput for the three hot
  paths, and the one limit that is still real.
- **[SECURITY.md](SECURITY.md)** — the threat model, and how to report something.

## Status

Pre-1.0 and honest about it. The core is solid and well-tested; the edges are
tracked in the issue list rather than papered over. macOS is the only platform
currently exercised — the code is written for Unix generally, but "should work" is
not the same as "is tested", so it does not claim Linux support yet.

## Licence

MIT. See [LICENSE](LICENSE).
