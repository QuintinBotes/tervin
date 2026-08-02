<div align="center">

<img src="assets/tervin-mark-dark.svg" alt="Tervin" width="72">

# Tervin

**The agent-native terminal workspace.**

A terminal that treats coding agents as first-class inhabitants, and never lies to
you about what they are doing.

</div>

---

<a id="status"></a>

> ## ⚠️ In development, not released
>
> **There is no release yet.** No version has been tagged, nothing is published to npm or
> Homebrew, and the install commands below describe how distribution *will* work rather
> than something you can run today. They are in the README because the pipeline that
> performs them is written and reviewable, not because it has run.
>
> **Nothing here has been used in anger.** The test suite is substantial and the awkward
> paths are covered deliberately, but a test suite is not a user. Expect to find things
> that are wrong in ways no test anticipated.
>
> **Local data formats are not stable.** Blocks, Threads, prompt history and saved
> sessions live in a SQLite database that gains columns as features land. Migrations are
> written and tested, but a pre-1.0 schema is a moving target: do not treat that database
> as an archive of anything you cannot lose.
>
> **macOS only, in practice.** The code is written for Unix generally and the PTY layer has
> no macOS-specific assumptions, but macOS is the only platform anything has been run on.
> "Should work on Linux" is not a claim, it is a guess.
>
> What *is* true is the documentation. Where Tervin cannot do something (gate Codex,
> guarantee a permission stop, produce an exit code no runtime reported) it says so, and
> that is the part this project takes most seriously.

---

## What it is

Tervin is a desktop terminal for the way people actually work now: a shell in one
pane, an agent in another, and a real need to know which of them just changed your
files.

It is a **real terminal first**. `vim`, `less`, `tmux`, `ssh`, oh-my-zsh,
powerlevel10k, Sixel images, bracketed paste, mouse reporting: all of it works,
because a terminal that is 95% correct is a widget that looks like a terminal. There
are tests that drive real `vim` and real `less` through a real PTY to prove it.

On top of that it adds three things a normal terminal cannot:

**Blocks.** Every command becomes a unit with its command, output, exit code,
duration, diagnostics, and test results: searchable months later.

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
  Installing a hook is not evidence it works: Claude Code silently ignores settings
  files that fail validation, so a broken gate looks exactly like no gate.
- An event the adapter cannot classify becomes `runtime.unclassified` and keeps its
  raw payload. Dropping it would make the timeline quietly incomplete; guessing would
  make it quietly wrong.
- `bypassPermissions` is deliberately absent from the offered modes. A one-click way
  to disable every check cannot be reconciled with telling you your actions are
  reviewable.
- Nothing leaves your machine that you did not attach. There is no code path that
  ships scrollback, files, or environment to a provider: the privacy promise is
  enforced by there being no other way in.

## Agents

| Runtime | How | Real permission gate? |
| --- | --- | --- |
| **Claude Code** | `stream-json` on stdio, plus a `PreToolUse` hook | **Yes**, verified against the real CLI |
| **Any ACP agent**, Gemini CLI, GitHub Copilot CLI, Claude Code via bridge, and 25+ others | Agent Client Protocol over stdio | **Yes**, the agent blocks waiting for the answer |
| **LM Studio, Ollama, vLLM, llama.cpp** | OpenAI-compatible HTTP | N/A: answers, cannot act |
| **Codex, Aider, OpenCode, Cursor Agent** | Managed pane, full terminal fidelity | No, and it says so |

Anything that speaks ACP or the OpenAI dialect can be added from Settings without a
release. That is the point of integrating with protocols rather than vendors.

### The two gates are not equally strong, and Tervin says which is which

Under **ACP**, the agent sends `session/request_permission` and *waits*. Deny actually
denies.

Under **Claude Code's hooks**, Tervin registers a `PreToolUse` hook and answers over a
Unix socket. A refusal blocks the tool before it runs, but any exit code other than 2
is non-blocking, so if Tervin becomes unreachable the action proceeds. The gate fails
open, the session's permission text says so, and the hook prints
`This tool call was NOT checked against Tervin Rules` rather than failing silently.

Tervin also **never answers `allow`** through a hook: only `deny` or `defer`. `allow`
would skip the runtime's *own* checks, and a safety feature that quietly disables
another safety feature is not one.

## `cd` knows where you have been

⌘J opens a picker over every directory a pane has sat in, ranked by how often you go there
and how recently, then by what you typed. It fills in `cd` and leaves the newline to you.

Not bound to Tab: zsh and fish completion is better than anything Tervin would write for
arbitrary commands, and taking Tab would replace something good with something worse.

## It reopens where you left off

Tabs, splits, each pane's directory and its recent output come back on launch. The
processes do not: they exited with the app, so each pane starts a fresh shell below its
old output, under a line saying exactly that. A restored screen that looked live would be
worse than no restore at all.

Saved output is only returned to a pane running the same program, so a local shell's
history can never reappear inside an SSH session. It ages out on the same retention window
as agent history and is deleted as soon as you switch the setting off.

## Agents you start yourself

Open a pane, type `claude`, and it becomes a Thread: titled after your prompt, with
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
established fact), full command output, and anything not in the event stream, and it
*says* what it left out, so nothing is assumed.

## Installing

Either of these, whichever you already have:

```sh
curl -fsSL https://raw.githubusercontent.com/QuintinBotes/tervin/main/packaging/install.sh | sh
```

```sh
npx tervin
```

Both open with **no Gatekeeper dialog at all**, and that is not luck. Tervin is not signed
with an Apple Developer ID, because that costs $99 a year and this is an open-source project.
It turns out not to matter for most routes, for a reason worth understanding:

macOS applies the `com.apple.quarantine` attribute in the **downloading application**, not in
the kernel. A browser sets it. `curl` and Node do not. So the identical bytes fetched by the
installer script or by `npx` carry only `com.apple.provenance` and launch normally, while the
same file downloaded through a browser is quarantined and refuses to open on first launch.

Verified rather than assumed: `xattr` on a `curl` download of a release asset shows
`com.apple.provenance` and nothing else.

Neither route ever strips quarantine on your behalf. Doing that is how people learn to wave
away a warning that matters, and it is unnecessary here because nothing was quarantined.

`npx tervin --install` copies it into `/Applications`; `--where` prints the cached
bundle; `--clean` removes it. The installer script takes `--version`, `--prefix` and
`--uninstall`, and never uses `sudo`.

### Homebrew

This repository is its own tap, so the URL is given explicitly. The `homebrew-`
repository prefix that `brew tap user/repo` looks for is only that shortcut's
assumption; the two-argument form takes any URL:

```sh
brew tap quintinbotes/tervin https://github.com/QuintinBotes/tervin

brew install --formula tervin   # compiles locally, nothing to approve
brew install --cask tervin      # prebuilt, but see below
```

Prefer the formula if you have a Rust and Node toolchain: it builds locally, so nothing is
quarantined and there is nothing to approve. It costs a few minutes.

The cask is prebuilt and faster, but **Homebrew quarantines cask downloads itself**, so macOS
will ask you to approve it once. Either approve it in System Settings, Privacy & Security, or
install with `brew install --cask --no-quarantine tervin` if you would rather make that
decision up front. Tervin does not make it for you.

One repository rather than two, so the packaging is reviewed in the same pull request
as the code it packages.

### Every route, and what each costs you

Ordered best to worst:

| Route | Gatekeeper prompt? | Notes |
| --- | --- | --- |
| `curl … install.sh \| sh` | **No** | Verifies the published checksum and refuses to install without it. Needs nothing but `curl`. |
| `npx tervin` | **No** | Checksums are baked into the npm package. Needs Node 20+. |
| `brew install --formula tervin` | **No** | Compiles locally. Needs a Rust and Node toolchain and a few minutes. |
| Build from source | **No** | Same reason. |
| `cargo install --git …` | **No** | Same reason. |
| `brew install --cask tervin` | Yes, once | Homebrew quarantines cask downloads. `--no-quarantine` skips it if you prefer. |
| `.dmg` from a browser | Yes, once | The worst route, and only there because people expect it. A browser sets quarantine, so macOS shows the unidentified-developer wall. Use one of the rows above instead. |

**Tervin will not be signed with an Apple Developer ID.** $99 a year to remove a one-time
dialog from the two least-recommended rows is not a good trade for an open-source project, and
five of the seven routes above have no dialog at all. That is not luck: macOS applies
`com.apple.quarantine` in the *downloading application* rather than in the kernel, so `curl`
and Node never set it. The release tooling reports plainly that a build is unsigned rather
than leaving you to discover it at launch.

So verify what you downloaded, because that checksum is doing the job a signature would:

```sh
shasum -a 256 -c SHA256SUMS.txt
```

If you specifically want the `.dmg` and the dialog gone, the honest answer is to use the
installer script instead. If you want to know why the dialog appeared, it is macOS working
correctly on an unsigned application, and it is not something Tervin should quietly defeat on
your behalf.

What a checksum does not give you that a signature would is stated plainly in
[SECURITY.md](SECURITY.md#why-unsigned-is-a-decision-not-a-gap), rather than left implied.

<a id="building-it-yourself"></a>

## Building it yourself

This is currently the only way to run Tervin.

Requires Rust 1.82+, Node 20+, and a Unix-like OS (macOS is the tested platform).

```sh
npm install
npm run app          # development, with hot reload
npm run app:build    # a .app and .dmg
```

Run the tests. All of them are real, and none of them mock the thing under test:

```sh
cargo test --workspace   # 456 tests, including real PTYs and real subprocesses
npx vitest run           # 80 UI tests
```

A few need a live dependency and are opt-in:

```sh
TERVIN_LIVE_CLAUDE=1 cargo test -p agent-runtime -- the_real_cli_honours_a_refusal
```

## Documentation

- **[ARCHITECTURE.md](docs/ARCHITECTURE.md)**: the crate graph, why the boundaries
  are where they are, and the decisions that were hard to get right.
- **[DESIGN.md](docs/DESIGN.md)** covers the design system: tokens, geometry, and the
  rules about what the interface may not do.
- **[CONTRIBUTING.md](CONTRIBUTING.md)**: how to work on this, including the
  testing standard, which is stricter than usual for a reason.
- **[AGENTS.md](docs/AGENTS.md)** covers using agents: picking a runtime, multiple
  accounts, reading the permission status, MCP, and handing work between agents.
- **[PERFORMANCE.md](docs/PERFORMANCE.md)**: measured throughput for the three hot
  paths, and the one limit that is still real.
- **[SECURITY.md](SECURITY.md)**: the threat model, and how to report something.
- **[COMPETITIVE-SPEC.md](docs/COMPETITIVE-SPEC.md)**: an in-depth review of every terminal
  people actually use, what Tervin lacks against each, and the specification for what comes
  next. Includes what Tervin should refuse to build.

## Status

**In development. No release, no published package, no stable data format.** See the notice
at the top: it is not boilerplate, it lists the specific things that are not finished.

What is genuinely done and tested: the terminal core, the Block engine, the agent adapters
(Claude Code, ACP, Codex, local models), session restore, prompt history, and the
permission model including the parts where Tervin admits it cannot enforce anything.

What is deliberately incomplete, and tracked rather than hidden:

- **CLI flag completions**: the largest remaining gap against Warp. The design question is
  open, not the implementation: shipping spec data, executing `--help`, and asking the
  user's shell all have real costs, and picking wrong is worse than waiting.
- **SSH latency and reconnect indicators.** SSH exposes no round-trip time, so a number
  here would be a measurement of something else wearing a latency label.
- **Linux and Windows.** Not claimed, because not exercised.

The commit history is the honest record: several commits exist because a test caught the
implementation, and a few because a test was itself wrong. Both are labelled as such.

## Licence

MIT. See [LICENSE](LICENSE).
