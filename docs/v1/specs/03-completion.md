# Spec 03 — CLI flag & subcommand completion

**The largest single Warp gap** (`COMPETITIVE-SPEC.md` §3.2, P1).

## Context

The design question is settled and a spike has already been run. Do not re-open either.

Three approaches were considered: ship spec data (rejected — a corpus to vendor, useless
for internal tools), execute `--help` and parse it (rejected as a default — it executes
an arbitrary binary from `PATH` to learn its flags; fish parses man pages precisely to
avoid this), and ask the user's shell. **Chosen: ask the shell, with `--help` parsing as
an explicit opt-in.**

The spike worked. `git commit -` returned all 114 real flags out of the user's own
compsys, with nothing guessed and `git --help` never executed.

Branch `completion-driver` exists, is pushed, has no PR, and is **called from nowhere**.
Decoding and parsing are done and tested. Driving zsh is not.

## What the spike established — do not rediscover

Verbatim from `COMPETITIVE-SPEC.md` §3.2 and `HANDOFF.md`:

- **`zsh/zpty`, not a plain subshell.** Completion functions only exist inside a widget
  context, so there must be an interactive zsh on a pty to send keystrokes to.
- **`zsh -f -i`.** `-f` skips the user's rc files — their startup code must not run to
  answer a keystroke. The cost is that `compinit` must be loaded explicitly.
- **`compinit -u -D`.** Without `-u` it prompts about insecure directories and hangs;
  without `-D` it rebuilds the dump.
- **`LISTMAX` large and `list-prompt ''`.** Otherwise zsh replies `do you wish to see
  all 114 possibilities (38 lines)?` and lists nothing. During the spike this produced
  an empty result that looked exactly like the technique failing.
- **`COLUMNS=1`** for one candidate per line. A wide terminal packs into columns and
  splitting on whitespace breaks on any candidate or description containing a space.
- **The output needs decoding.** At `COLUMNS=1` zsh writes each character followed by a
  space and a backspace, so `--all` arrives as `-·-·a·l·l·`. Strip every
  space-backspace pair, then remove CSI sequences separately.
- **`TERM=dumb` disables ZLE**, and with no ZLE there is no completion system at all:
  Tab inserts a literal tab and the reply is empty — indistinguishable from a shell with
  no completions installed.
- **`Read` on a pty blocks uninterruptibly.** A deadline checked between reads is not a
  deadline. Read on a thread behind a channel and use `recv_timeout`.

## The known blocker

From `HANDOFF.md`, and this is slice 03.1:

> **Do this first, before writing any code:** open a real zsh and run the setup by hand,
> then inspect `bindkey ^T`. The `^T` widget is meant to run `expand-or-complete` and
> then print its own marker, and no marker ever arrives. The prime suspect is the
> function body not surviving definition on a single written line. Establish it
> interactively; the last session lost several rounds to patching blind.

`TERVIN_COMP_DEBUG=1` dumps the raw and decoded streams. Every failure in this module is
silent and looks identical to "this shell has no completions", so use it.

## Slices

### 03.1 — Establish the `^T` widget interactively
No code. Open a real zsh, run the setup by hand, inspect `bindkey ^T`, and find why the
marker never arrives. Write down what was established before writing anything.

*Exit:* a written finding explaining the failure, verified in a live shell.

### 03.2 — The zsh driver and marker protocol
The shape is right and worth keeping: **a distinguishable second event, not a count of
the first.** After setup, both the explicit `print -n` and the redrawn prompt emit the
ready marker, so counting cannot tell "setup done" from "listing done".

Build the driver on the established finding: `zpty`, setup, a distinguishable
completion-finished marker, a read loop that waits for it, and a timeout.

*Exit:* `git commit -` returns all 114 flags through the driver, in a test that drives
real zsh.

### 03.3 — Cache
Keyed on the command prefix, invalidated on `PATH` change and on a TTL. Completion must
feel instantaneous; spawning a zpty per keystroke will not.

*Exit:* a second identical prefix returns from cache; a benchmark shows the cached path
within one frame.

### 03.4 — bash and fish paths
Separate drivers. bash has `complete -p` and programmable completion; fish has
`complete -C`, which is markedly easier than either. An unknown shell falls back to path
and history completion, which already work.

*Exit:* `git checkout ` offers branches under both bash and fish.

### 03.5 — The completion menu
Render candidates in Tervin's own menu rather than letting the shell draw them.
Descriptions where compsys supplies them. Keyboard navigation. **Not bound to Tab** —
`README.md` and `keymap.ts:78-82` record that decision: zsh and fish completion is better
than anything Tervin would write for arbitrary commands, and taking Tab would replace
something good with something worse. Bind it to something else and say so.

*Exit:* the menu appears, navigates, and inserts. Tab still reaches the shell.

### 03.6 — Degradation, said once
A shell Tervin cannot drive degrades silently to today's behaviour **and says so once in
the Bridge panel** — that is the §3.2 exit criterion verbatim. Not a toast per keystroke.

*Exit:* running under an unsupported shell produces exactly one Bridge notice.

### 03.7 — `--help` parsing as opt-in
Off by default, with the reason stated in the setting: enabling it means Tervin executes
binaries from `PATH` to learn their flags. Only for commands the shell had nothing for.

*Exit:* the setting exists, defaults off, and its copy states the cost.

## Also in scope: the command inspector

Warp shows a command's parsed structure inline. The pieces exist — `rules-engine`'s
classifier already splits compound commands, and completion now knows subcommands and
flags. A small surface showing what a command will do before `Return` is a Warp parity
item that costs little on top of this spec. Add it if 03.1–03.7 land clean; drop it if
they do not.

## Verification

```sh
cargo test -p tervin-app --  completion
cargo test --workspace
TERVIN_COMP_DEBUG=1 pnpm app   # then type `git commit -`
```

Exit criteria for the spec, from §3.2: *"`git ` offers subcommands, `git commit -` offers
flags, both sourced from the shell. A shell Tervin cannot drive degrades silently to
today's behaviour and says so once in the Bridge panel."*
