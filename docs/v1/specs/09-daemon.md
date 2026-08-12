# Spec 09 — Detach & reattach daemon

`COMPETITIVE-SPEC.md` §3.8. Marked **P1 and "the single largest functional gap in this
document"**. The largest piece of work in this roadmap.

## Context

A Tervin pane dies with the app. Session restore replays layout and scrollback but not
processes, and the UI says so honestly — *"restored from your last session; nothing above
is running"*. Per the spec: *"honesty is not the same as capability."*

tmux's entire value is that a long build survives a closed laptop. This is the category
Tervin most needs to answer, because it is why people do not care which terminal they use.

**The approach, from §3.8:** a small supervisor process that owns the PTYs and outlives
the app, with the app as a client attaching over a Unix socket. This is what tmux and
zellij do. It is a significant architectural change: the PTY registry moves out of
`tervin-app` into a daemon, and `terminal-core` grows a client mode.

This spec is sequenced after 01–08 precisely because it rewrites the layer they all sit
on, and rebasing eight specs across it would cost more than sequencing them.

## Staging, verbatim from §3.8

1. Daemon owns PTYs for the current app session, with the app reattaching after a crash
   or reload.
2. Daemon survives app exit; panes reattach on next launch with live processes.
3. Attach from a second window or machine — where it starts competing with tmux directly.

**Until stage 2 lands, "session restore" must keep saying plainly that processes are not
revived.** It does today. Do not weaken that line ahead of the capability, and do not
strengthen it until stage 2 is observed working — a capability is upgraded by evidence,
never by configuration.

## Slices

### 09.1 — Move the PTY registry into a daemon process
`crates/terminal-core/src/registry.rs` is currently a `HashMap<PaneId, Arc<PtySession>>`
inside the app. Extract it into a supervisor binary. The app becomes a client.

Transport: a Unix socket in `runtime_dir()`, which is already created `0700` by
`create_private_dir` (`crates/tervin-core/src/paths.rs:61`) with the comment that "the
permissions are the authentication for the hook socket". The same reasoning applies here,
and this socket carries *more* — every keystroke and every byte of output.

*Exit:* the app spawns the daemon, opens a pane through it, and the pane behaves
identically. All existing PTY tests pass against the client path.

### 09.2 — The wire protocol
Bytes in both directions, resize, close, and pane enumeration. Two constraints from
`ARCHITECTURE.md` and `PERFORMANCE.md` carry over intact:

- **Terminal bytes never become JSON.** They cross this socket as raw binary too.
- **Coalescing stays.** The 6ms/32KiB batching with a 120ms synchronized-output ceiling
  belongs in the daemon, not duplicated on both sides.

The input gate — `tcgetattr` on the master fd, waiting for `ICANON` to clear before
writing, because a shell's line editor calls `tcsetattr` with `TCSAFLUSH` and *discards*
queued input — stays with the PTY, so it stays in the daemon.

*Exit:* a benchmark shows throughput within a stated margin of the in-process path. The
number goes in the PR.

### 09.3 — Stage 1: reattach after an app crash or reload
The app dies or the Tauri watcher restarts it; the daemon keeps the PTYs; the app
reattaches and the panes are live.

This alone fixes a `HANDOFF.md` trap worth fixing: *"Do not edit Rust while `pnpm app` is
running. The Tauri watcher rebuilds and restarts the app, killing the user's Threads
mid-run."* Development on Tervin gets materially easier at this slice.

*Exit:* `kill -9` the app during a running `sleep 60`; relaunch; the sleep is still
running in its pane.

### 09.4 — Stage 2: survive app exit
The daemon outlives a clean quit. Panes reattach on next launch with live processes.
Needs a lifecycle policy: when does the daemon exit? A configurable idle timeout, an
explicit `tervin daemon stop`, and a hard cap so a forgotten daemon is not immortal.

**This is the slice that changes what session restore may claim.** Update the restore
banner only after stage 2 is observed working, and only for panes actually reattached —
a mixed restore where some panes are live and some are replayed must distinguish them.

*Exit:* quit the app during a running build; relaunch; the build is still running and the
banner says so. Panes that could not be reattached still carry the old, honest line.

### 09.5 — Stage 3: attach from a second window
Where this starts competing with tmux directly. A second Tervin window attaches to the
same daemon and sees the same panes.

Scope check: §6 puts stage 3 in "Later, or never" territory by implication. Build it if
09.1–09.4 land clean; if they do not, stop at stage 2 and say so — stage 2 is where the
user-facing value is.

*Exit:* two windows show the same live pane.

### 09.6 — Failure modes, stated
A daemon is a new way to fail. Each needs a defined, tested behaviour:

- Daemon absent at launch → spawn it, or run in-process and say so.
- Daemon unreachable mid-session → the pane says the connection dropped; it does not
  silently show stale output. A frozen pane that looks live is the exact failure the
  session-restore banner exists to prevent.
- Version skew between an old daemon and a new app → refuse and explain, do not guess.
- Daemon killed while panes are live → the app notices and marks them.

*Exit:* a test per failure mode. No path produces a pane that looks live and is not.

## Verification

```sh
cargo test --workspace
cargo bench -p terminal-core
```

Manual: `docs/MANUAL-TEST.md` §4.3 in full, plus kill/relaunch cycles at each stage. This
spec deserves a manual pass of its own added to `MANUAL-TEST.md`.
