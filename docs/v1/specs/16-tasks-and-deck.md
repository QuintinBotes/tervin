# Spec 16 — Tasks, the Deck, and parallel Threads

`spec_file.md` "Tervin Deck" and "Workflows and tasks", plus `COMPETITIVE-SPEC.md` §4.1.

## Context

`spec_file.md`'s acceptance criterion 4 is:

> *Multiple heterogeneous agent tasks can run in parallel while users understand their
> purpose, state, current action, permissions, and output.*

**This is the one acceptance criterion the original spec sets that is not met.** Tervin
speaks ACP to one agent at a time. Zed 1.0 shipped parallel agents as its headline
feature — Codex CLI alongside Claude Agent and Gemini CLI in one window, over ACP, with a
thread each. `COMPETITIVE-SPEC.md` §3.12: *"Tervin has the Threads model and the Deck to
do this and does not yet do it."*

`Activity` types `tasks` and nothing renders it. `AgentDeck.tsx` exists but is a list, not
the overview the spec describes.

## Slices

### 16.1 — N concurrent Threads
Lift the one-at-a-time constraint. The event stream is already provider-neutral and
append-only, and `ThreadState` already has 17 variants including a first-class `Unknown`,
so the model supports this — the runtime layer does not.

Per-runtime concurrency differs and must be declared rather than assumed: Codex cannot run
concurrent turns at all (`crates/agent-runtime/src/codex/runtime.rs:413`), so its
capability says so and the UI does not offer what it cannot do.

*Exit:* three Threads on three different runtimes run simultaneously, each with its own
timeline. Codex's limit is stated, not silently hit.

### 16.2 — Worktree isolation, by default
§4.1's key requirement. Each Thread gets its own git worktree, so two agents editing the
same repository do not corrupt each other's work.

`git-service` already handles worktrees. The hard part is not creating them — it is
cleanup, and what happens when a Thread ends with uncommitted changes in its worktree.
Deleting is destructive; leaving them accumulates. Decide, implement, and say which.

*Exit:* two Threads modify the same file without conflict. A Thread ending dirty has a
defined, stated outcome.

### 16.3 — Cross-Thread diff attribution
Which agent changed which line. The mockup shows exactly this string: *"Uncommitted.
Authored by Claude Code in thread 3f2a."*

The event stream carries `file.changed` and `patch.applied` per Thread, so attribution is
derivable rather than guessed. §4.1 also requires that **conflicts are surfaced, not
resolved** — Tervin shows two agents touched the same region and stops there.

*Exit:* a changed file names the Thread that changed it. A conflict is shown, not merged.

### 16.4 — The Deck
`spec_file.md:57`: *"Overview of active agents and background work."* Not a list of
Threads — an overview of state: what is running, what is waiting on you, what finished,
what failed, and what each is currently doing.

`ThreadState::needs_user()` and `is_working()` already exist for exactly this grouping.

*Exit:* the Deck groups by state and updates live. "1 agent waiting on you" in the top bar
matches what the Deck shows.

### 16.5 — Tasks and background orchestration
`spec_file.md:823-836` specifies workflows and tasks; `Activity` types `tasks`. A task is a
unit of background work that is not a Thread and not a Block — a long build, a test run, a
workflow from spec 06, an indexing pass.

Tasks appear in the rail, in the Deck, in universal search (spec 17), and in the status
rail's "task progress" field. Their progress is reported where a runtime reports it and
absent where it does not — `subagent.progress` already models this honestly.

*Exit:* a running workflow appears as a task with progress, and a task that reports no
progress shows none rather than a fake bar.

### 16.6 — Mission Control
The layout mode from spec 15.4 that depends on this one: two terminal panes, thread
inspector, compact task timeline, Deck summary, *"background activity visible without
taking over the workspace"*.

`DESIGN.md` caps agent UI at ~30% of window width by default. Mission Control is the mode
most likely to breach that; it must not.

*Exit:* Mission Control shows three concurrent Threads and stays within the width cap.

## Verification

```sh
cargo test --workspace
pnpm exec vitest run
./scripts/testbed.sh    # then run three agents against the testbed simultaneously
```

Manual: `docs/MANUAL-TEST.md` §7, extended — three runtimes at once, one denied a
permission, one stopped mid-turn, one finishing normally. Never point them at Tervin's own
source.
