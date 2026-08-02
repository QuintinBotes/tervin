# Architecture

This document is about *why*. The code says what it does; this says which decisions
were load-bearing and what the alternatives cost.

> **In development.** Tervin has not been released and its local data formats are not
> stable. This document describes the code as it stands, including the parts that are
> deliberately incomplete. See the [status notice](../README.md#status) in the README.

## The shape

Ten Rust crates and one React app. The dependency direction is strictly one-way: no
crate below `tervin-app` knows about the UI, and no crate knows which agent is
running except the adapter for it.

```
tervin-core        ids, the event vocabulary, capabilities, risk, paths
  ├── terminal-core       PTY, OSC/CSI scanning, alternate-screen detection
  ├── shell-integration   zsh/bash/fish/pwsh hooks, alias expansion, injection
  ├── block-engine        commands → Blocks, diagnostics, tests, SQLite + FTS5
  ├── git-service         porcelain v2, hunk-level diffs and apply
  ├── file-index          gitignore-aware walk, fzf-style fuzzy matching
  ├── session-manager     shells, SSH config, tmux/zellij, serial
  ├── rules-engine        risk classification, policy, approvals, audit
  └── agent-runtime       the AgentRuntime interface and every adapter
        └── tervin-app    Tauri host, IPC, application state
              └── ui/     React workspace
```

`tervin-core` has no dependencies on the others by design. Everything shared, the 27
event names, `Tier`, `CapabilityLevel`, `RiskAssessment`, lives there so a change to
the vocabulary is a compile error everywhere it matters rather than a runtime
mismatch.

## The event vocabulary is the product

Every runtime normalises into exactly the same events. That is the single decision
everything else follows from:

- **Adding an agent never touches a view.** The ACP adapter and the Claude Code
  adapter emit the same `command.proposed`, `patch.applied`, `test.completed`. The UI
  has never heard of either.
- **Context handoff becomes possible at all.** A Thread's history is
  provider-neutral, so it can be summarised for a different agent. Nothing else in
  the design makes that tractable.
- **Auditing is uniform.** One store, one query, one export: regardless of who acted.

The vocabulary is append-only. Nothing rewrites history: a superseded plan is a new
`plan.proposed`, never an edit of the old one. A timeline you cannot trust to be a
record is not worth having.

Two events exist beyond the specified 27. `thread.state` so the UI can render state
without re-deriving it from every event, and `runtime.unclassified` so an unmodelled
message is *kept and labelled* rather than dropped or guessed at.

### Provider-specific things do not become events

Claude Code hook runs are session metadata, not events: except the two cases that
changed what happened. A hook that *blocked* something becomes a
`permission.denied` attributed to `ProviderNative` (the user's hook decided, not
Tervin). A hook that *failed* becomes a `diagnostic.detected`, because the session is
now running differently than configured and nothing else would say so.

Inventing `hook.ran` would have been easier and would have broken the neutrality that
makes the vocabulary worth having.

## Capability honesty is mechanical, not aspirational

`CapabilityLevel` is `Supported | Partial{note} | Unsupported{reason} | Unknown`: not
a bool. A note is *required* to express a caveat, so the UI can always explain a
limit, and a refusal always carries a reason.

The rule that matters: **a capability is upgraded by evidence, never by
configuration.**

`native_permission_bridge` for Claude Code starts `Partial`. It becomes `Supported`
only when a hook has actually called in or an inbound `can_use_tool` has been
observed. This is not caution for its own sake: `claude --help` states that settings
files failing validation are *silently ignored* in print mode. A gate that was
installed but never consulted is indistinguishable from no gate, and claiming one
would be the exact failure this codebase is organised to prevent.

`RiskAssessment.enforceable` carries the same distinction per action. When it is
false the UI says "observed", and the approval sheet says Tervin could not prevent
this.

## Tier is not a ranking

`Structured` (1), `EnhancedCli` (2), `GenericTerminal` (3), and `Conversational`,
numbered 0 rather than 4. A model endpoint answers and cannot act; numbering it below
a generic terminal would read as "a worse agent" when it is a different kind of thing.
Its label says `Answers · cannot act`, and every capability implying action is
`Unsupported` with a reason.

## Terminal correctness

### The OSC scanner is hand-rolled, and non-destructive

Tervin needs to *observe* escape sequences (OSC 7 cwd, OSC 133 prompt marks, OSC 8
hyperlinks, DEC private modes) while passing the byte stream through to xterm.js
untouched. A VT parser crate would consume the stream and hand back a screen model:
the wrong shape entirely, and a second source of truth about what the terminal shows.

The scanner handles CSI, DCS, APC, and PM specifically so that `]` (0x5D, a legal CSI
final byte) is not mistaken for the start of an OSC sequence. Getting that wrong
corrupts output in a way that only appears with certain prompts.

### Markers can split across reads

A PTY read can end mid-escape-sequence. The first version leaked partial marker bytes
into Block output; the fix is `OscHit.start_offset` plus a `PendingMarker` on each
chunk and a `marker_carry` in the builder. A split-chunk test enforces it, because
this is invisible until it is not.

### The alternate screen pauses capture

When a full-screen program takes over (`vim`, `less`, `htop`), Block capture stops and
the Block notes how much was skipped: *"195 KB of screen redraws were not
captured"*. Storing a megabyte of cursor movement as a command's output would make the
Block useless and the database large.

### `vim` is the test

`crates/terminal-core/tests/editors.rs` drives real `vim -u NONE -N` and real `less`:
alternate-screen enter and leave, editing and writing an actual file, surviving a
resize, enabling mouse reporting on `:set mouse=a`, Escape reaching the program, and
correct geometry. Nothing is scripted: the binaries decide whether it passes.

## Blocks

A PTY merges stdout and stderr into one stream. There is no way to separate them
after the fact, so `block-engine` does not claim to: the model documents it, and the
UI never shows a stderr filter it cannot honour.

Output is inline up to 256 KB and spills to disk beyond that, capped at 64 MB.
Diagnostics, test summaries, ports, and paths are parsed out: with path existence
checked, capped at 200 checks so a pathological line cannot stall a build's output.

FTS5 queries sanitise user input, because typing `foo(` mid-search would otherwise
error rather than simply matching nothing yet.

### The index does not walk protected folders

macOS guards `~/Desktop`, `~/Documents`, `~/Downloads`, `~/Music`, `~/Pictures`,
`~/Movies`, and `~/Library` behind permission prompts. Rooting the file index at the
home directory walked all of them, so launching Tervin produced a burst of prompts:
including one asking for access to Apple Music, from a terminal.

That symptom took five wrong hypotheses to explain, which is worth recording: it is not
in the Info.plist, no media framework is linked or loaded, it happens with the canvas
renderer as well as WebGL, and a hardened runtime with no media entitlements does not
stop it. It was a filesystem read the whole time.

`filter_entry` now refuses incidental descent, while a root that is *inside* one of
those folders still indexes normally: a user who opened `~/Documents/project` asked for
it. `default_project_root` also prefers `~/Projects` and its siblings over `~`.

## Rules

**Compound commands are split before classification.** `echo hi && rm -rf /` is not
judged on `echo`. Splitting happens on `&&`, `||`, `;`, `|`, and inside `$()` and
backticks, before any pattern runs.

**Unparseable is `Moderate` and unenforceable, never `Low`.** A command Tervin cannot
inspect is not a safe command.

**Grants key on the exact normalised action.** Approving `rm -rf build` never licenses
`rm -rf /`. This is the whole reason approvals are keyed on the action rather than a
tool name.

## Agent adapters

`AgentRuntime` + `AgentSession` are the only interface. An adapter translates its
runtime's dialect and reports honestly what it cannot do.

Locks are never held across an `await`. An earlier version held a `parking_lot` guard
across one and produced `!Send` futures and a real deadlock risk; `ThreadRuntime`
holds `Arc<dyn AgentSession>` and the registry hands out snapshots so discovery never
holds a lock while spawning processes.

A related trap, found by a test: two `lock()` calls in one struct literal deadlock,
because the temporaries live to the end of the statement and `parking_lot` is not
reentrant. Read everything under one guard.

### `LaunchConfig.env` uses empty values as removals

A profile clears account-selecting variables so an ambient value cannot decide which
account runs. Passing those pairs to `Command::envs` sets them to empty strings:
`CLAUDE_CONFIG_DIR=""` is an empty *path*, not absence, and it silently selected the
wrong account. `apply_env` calls `env_remove` for empty values and exists so no
adapter can reintroduce the bug.

### The hook client is the same binary

Tervin's `PreToolUse` hook is Tervin itself, invoked as `tervin --tervin-hook <socket>`.
`main()` checks for the flag before any window or database opens. This avoids a second
artefact to install and makes the hook's path exact: `std::env::current_exe()` rather
than a `PATH` lookup that might resolve to something else.

## Shell integration is injected, not requested

Blocks need the shell to report prompt boundaries. Asking a user to edit their rc file
first would mean the product does not work when they open it.

So Tervin injects: `ZDOTDIR` for zsh (with all four rc shims, sourcing the user's own
first and then restoring `ZDOTDIR`), `--init-file` for bash, `vendor_conf.d` for fish,
`-NoExit -Command` for pwsh. **It never modifies a file the user owns**, asserted by
test, and a hook the user sourced themselves is detected so it is not loaded twice.

## The UI

**State that changes per frame does not live in React.** Scrollback lives inside each
xterm instance. Putting bytes into a store would mean re-rendering on every frame of
output.

**Terminal bytes never become JSON.** Output crosses IPC as raw binary and arrives as
an `ArrayBuffer`. Encoding a build log as a JSON string array costs several times the
bytes and a parse per frame.

**Panes are a tree, not a list.** The moment someone splits one half of an existing
split, the second thing anyone does in a terminal, a list cannot express it.

**Renderer fallback survives a GPU crash.** A marker is written before WebGL is
created and cleared only after a frame is painted. A surviving marker on next launch
steps down `webgl → canvas → dom`. Without this, a driver crash makes the app
permanently unopenable.

**Unhandled keys reach the terminal.** `runAction` returns a bool; only a handled
action calls `preventDefault`. This is what keeps `vim` and `emacs` usable, and it is
why the keymap is data with context scoping rather than a chain of `if` statements.

**Overlays take the keyboard, and the terminal gives it up.** A pane holding focus
under a dialog sends the dialog's keystrokes to the shell, and `Return` in an
approval sheet would run a command instead of answering a question about running one.
`overlayOpen()` is the single guard.

## Testing standard

The rule: **test against the real thing, or say plainly that you did not.**

- Real PTYs with real `vim`, `less`, `zsh`, `/bin/sh`.
- Real temporary Git repositories, not a mocked porcelain parser.
- A real Python ACP agent over a real pipe: 15 scenarios including permission denial.
- A hand-written HTTP server with real chunked SSE for the model adapter, because a
  framework would tidy up exactly the awkward framing that breaks parsers.
- The real `claude` CLI for the hook gate, behind `TERVIN_LIVE_CLAUDE=1`.

This standard has repeatedly paid for itself. Bugs found by these tests and not by
review: the `permissions()` double-lock deadlock; `shutdown()` not closing the child's
stdin, leaking the agent process; command endings missing from the timeline when
nobody was watching; a cancelled model turn hanging forever when the server went
quiet; partial escape sequences leaking into Block output.

When something cannot be verified, it is written down as unverified rather than
assumed: see the deferred Codex adapter, which is deferred *because* its event
schema could not be checked against a real binary.
