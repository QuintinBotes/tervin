# Handoff

Written for whoever picks this up next. It assumes you have read `README.md` and
`docs/COMPETITIVE-SPEC.md` and skips anything they already say.

Two things this document is for: telling you what is true right now, and stopping
you rediscovering things that cost hours. Where something was verified against a
real binary or a real run, it says so. Treat anything not marked as verified as a
claim you should check.

---

## Where the repository is

`main` is green: **705 Rust tests, 342 vitest**, clippy and fmt clean. Everything
from the last session is merged (#33, #35, #36, #37, #38).

| Branch | State |
| --- | --- |
| `main` | current, green |
| `completion-driver` | §3.2 in progress, pushed, **no PR, called from nowhere** |
| `linux-ci` (#31) | open, parked. Linux is deferred; macOS is the focus |

`v0.1.0` is released. `curl` and Homebrew work. **`npx tervin` does not** — the
package is not on the registry. The README says so honestly; keep it that way
until it is true.

---

## Traps in this repository

Each of these cost real time. None are obvious from reading the code.

**Do not edit Rust while `pnpm app` is running.** The Tauri watcher rebuilds and
restarts the app, killing the user's Threads mid-run. **Switching branches does
the same thing** — the watcher sees the changed files. Use a `git worktree` for
branch work while the app is up.

**`gh pr merge --delete-branch` auto-closes any PR based on that branch**, and a
closed PR cannot be reopened or have its base changed. That is how #34 was lost
and became #37. Retarget dependent PRs *before* merging their base.

**This repository allows squash merges only.** A branch of sixteen commits lands
as one, so put the narrative in the PR body — that becomes the commit message.

**The ruleset requires up-to-date branches.** Every merge makes the next PR
`BEHIND`. `gh pr update-branch <n>`, one at a time, then re-arm `--auto`.

**Squash merges do not appear in `git branch --merged`.** Find merged branches
with `gh pr list --state merged --json headRefName`.

**Never `git push --force --all`.** It destroyed three merged PRs earlier in this
project. `--force-with-lease` on one named branch.

**zsh does not word-split unquoted variables**, and `grep --include=*.rs` needs
the pattern quoted or the glob expands first.

**An agent running inside Tervin can commit to whatever repo it is pointed at.**
Check `git log` for commits you did not write. Point test agents at
`~/tervin-testbed` (`./scripts/testbed.sh`), never at Tervin's own source.

---

## Facts already established

Do not re-derive these.

**The gate must print nothing when it does not object.** Claude Code accepts
`allow`, `deny`, `ask`. Given anything else — including Tervin's own `defer` — it
**ends the turn immediately and reports success**. Because the gate sees every
tool call, that killed every Thread at its first action. Silence plus exit 0 is
"no opinion"; a denial is a reason on stderr with exit 2. Verified by substituting
a hook that printed only the `defer` line and reproducing it exactly.

**Model aliases resolve; do not pin identifiers.** `--model sonnet` resolved to
`claude-sonnet-5` on the work account, verified. A pinned `claude-opus-4-1` rots
in the worst way — the old name still resolves, so it fails by quietly running
last year's model rather than erroring.

**`--effort` exists on the shipped 2.1.220 binary**, taking `low`, `medium`,
`high`, `xhigh`, `max`. An unrecognised value is a **warning**, not an error: the
CLI falls back to the default and runs anyway, so a wrong list produces a session
that reports one effort and spends another. Verified against the binary.

**A plan only exists if the Thread started in plan mode.** `plan.proposed` comes
from `ExitPlanMode`, which an agent calls only when planning was its starting
mode. Switching mode afterwards produces nothing.

**`TERM=dumb` disables zsh's ZLE**, and with no ZLE there is no completion system:
Tab inserts a literal tab and the reply is empty, which looks exactly like a shell
with no completions installed.

**`Read` on a pty blocks uninterruptibly.** A deadline checked between reads is
not a deadline. Read on a thread behind a channel and use `recv_timeout`.

**A shell's line editor calls `tcsetattr` with `TCSAFLUSH` when it starts**, which
*discards* queued input. Input written a moment too early is destroyed, not
delayed. `PtySession` gates writes until `ICANON` clears; do not remove that.

**OSC 7 carries a hostname, not nothing.** zsh sets `$HOST`, so a bare "is it
empty" test classifies every local `cd` as remote. `Some(host)` must mean
genuinely elsewhere.

**npm:** the token authenticates as `quintinbotes` and is then refused with 403 —
the account requires 2FA for writes. npm is **deprecating** tokens that bypass
2FA, so the token route is closing. Publish v0.1.0 by hand from a real terminal
(`npm login` needs a TTY), then configure trusted publishing for
`QuintinBotes/tervin/release.yml` and delete `NPM_TOKEN`.

---

## Two patterns worth knowing

Nearly every bug found in the last session was one of these.

**Built in Rust, unreachable from the UI.** Six in one day: `LaunchConfig.model`
emitted `--model` and was never set; `permission_mode` was plumbed and never sent,
so the Plan surface could never fill; `task_progress` was parsed nowhere while a
`subagents` capability was advertised; `SessionMetadata` had no `cwd` though the
normalizer tracked it; `set_project_root` was correct and called from nowhere; the
dialog plugin was a dependency with permission granted and never invoked. All 63
Tauri commands are now called — but check new fields, not just commands.

**The app does something and does not say so.** A plan proposed with no
indication. A button that appears dead because the agent already moved on. A
message queued silently while the agent is busy. A divider that looked
unresizable because the target was 5px. Each was fixed individually; the pattern
was not. This is what the user meant by "unclear what to do where and when".

---

## Next steps, in order

### 1. Finish §3.2, CLI completion via the shell

Branch `completion-driver`. Decoding and parsing are done and tested. Driving zsh
is not.

**Do this first, before writing any code:** open a real zsh and run the setup by
hand, then inspect `bindkey ^T`. The `^T` widget is meant to run
`expand-or-complete` and then print its own marker, and no marker ever arrives.
The prime suspect is the function body not surviving definition on a single
written line. Establish it interactively; the last session lost several rounds to
patching blind.

The shape is right and worth keeping: a *distinguishable second event*, not a
count of the first. After setup, the explicit `print -n` and the redrawn prompt
both emit the ready marker, so counting cannot tell "setup done" from "listing
done".

`TERVIN_COMP_DEBUG=1` dumps the raw and decoded streams. Every failure in this
module is silent and looks identical to "this shell has no completions", so use it.

### 2. Say when the agent is busy (task #15)

The highest-value item from the user's UX complaints. Sending a turn while the
agent is mid-turn queues it with no acknowledgement — no busy state, no "queued",
nothing changes. The user reported this as "the agent is useless in responding".
Same root as the Plan surface's Approve button appearing to do nothing.

### 3. Surface unmodelled runtime messages (task #18)

`runtime.unclassified` is filtered out of the timeline entirely, so anything the
normalizer does not model is invisible rather than plain. That is exactly how
subagent `task_progress` hid — the data was never missing, it was in the discard
bucket. A count in the Bridge panel turns the next such discovery from a bug
report into a glance.

### 4. Smaller, well-understood

- **#10** Block output captures the next prompt's bytes past its `133;D`. Escapes
  are stripped for display now, so it is invisible rather than absent.
- **#8** bash never emits `PromptEnd` (`133;B`) in the injected login shell: the
  marker goes on `PS1` and anything setting `PS1` afterwards drops it. Blocks
  still form; anything keyed on `PromptEnd` gets nothing from bash.
- **#17** `session-manager`'s `a_closed_port_reads_as_refused_rather_than_as_a_timeout`
  failed once under full-workspace load and passes in isolation. An
  intermittently red suite trains people to re-run rather than read.

### 5. Parked, and the user's call to unpark

- **#16** the UX pass. The user's verdict was that the interface "does not behave
  well at all". They chose to finish the roadmap first. Do not start this without
  asking.
- **#31** Linux CI. It found three real defects before being parked, two of them
  unsound tests that had never tested what they claimed.

### 6. Last: npm (task #5)

Blocked on the user's account, not on code. See the facts above.

---

## How to test

```sh
./scripts/testbed.sh    # ~/tervin-testbed: a real bug, a dirty tree, an AGENTS.md
pnpm app
```

`docs/MANUAL-TEST.md` is ordered so an early failure tells you not to trust what
follows. Start a **fresh Thread after any rebuild** — one launched by the previous
binary holds a socket that no longer answers and will keep failing regardless of
what was fixed.
