# Spec 17 — Universal search & palette completion

`spec_file.md:838-891`.

## Context

The command palette is real and already fuses more than most: panes, splits, surfaces,
settings, agent profiles, plus live Blocks, shell aliases and file-index results
(`ui/src/components/CommandPalette.tsx:174,188,209`). The fuzzy matcher behind it is
measured and fast — 0.65ms for a selective query over 20,000 files, after a 17.5×
optimisation.

What the original spec asks for and does not exist: the rest of the sources, and filters.

`spec_file.md:867-891` specifies search across scrollback, persisted Blocks, commands,
output, errors, files, agent prompts and summaries, tasks, commits and sessions — with
filters on project, branch, host, date, status, agent, tag and content type.

## Slices

### 17.1 — The missing palette sources
`spec_file.md:842-864` lists fifteen. Present: actions, settings, panes, workflows (after
spec 06), command history, files, Threads. Missing: **keybindings** (needs spec 02),
**tabs**, **workspaces**, **SSH profiles** (needs spec 14.4), **git branches and commits**,
**tasks** (needs spec 16), **help**.

`git-service` already reads branches and commits, so that one is wiring.

*Exit:* every source in the spec's list returns results, or is absent for a stated reason.

### 17.2 — Universal search across every store
Distinct from the palette: the palette is for *doing*, search is for *finding*. Blocks are
already FTS5-indexed with diagnostics, tests and ports parsed out; agent prompts are
already searchable with reasoning deliberately excluded (`store.rs:1339`, tested at
`:1544`).

Missing sources: live scrollback (xterm's `SearchAddon` covers one pane; this is across
panes), commits, sessions, and tasks.

**Keep the reasoning exclusion.** It is deliberate and tested — a predecessor's thinking
read as established fact is the failure it prevents.

*Exit:* one query returns matches from Blocks, prompts, files and commits, labelled by
kind.

### 17.3 — Filters
Eight from the spec: project, branch, host, date, status, agent, tag, content type. Blocks
already carry project, host, git context, tags and exit status, so most of this is query
surface over data that exists.

*Exit:* each filter narrows results. Combining two narrows further.

### 17.4 — Ranking, categories, empty states
The spec's requirements: immediate response, keyboard navigation, context-aware ranking,
clear categories, useful empty states.

"Context-aware" means what you are doing changes what ranks: in a git repository with
changes, branches rank higher; with a Thread waiting, its actions rank higher.

The mockup's empty state is the model: *"No matches. ⌘⏎ runs it in the focused pane."* —
it tells you what to do next rather than saying nothing was found. `DESIGN.md` requires
useful empty states and rejects silent dead ends.

*Exit:* every result list has a written empty state that offers an action.

### 17.5 — Performance under the real corpus
`PERFORMANCE.md` states the one limit honestly: a single-character query is 2.87ms per
20,000 files and extrapolates to ~29ms at the 200,000-entry cap — *"a perceptible hitch on
the first keystroke in a very large repository"*, resolving as soon as a second character
is typed.

Adding sources multiplies that. Measure with every source live, and if the budget breaks,
either fix it or state the new number in `PERFORMANCE.md` in the same register. Do not let
a documented, bounded hitch quietly become an undocumented, unbounded one.

*Exit:* a measured number for a one-character query across all sources, published.

## Verification

```sh
cargo test --workspace
cargo bench -p file-index
pnpm exec vitest run
```

Manual: `docs/MANUAL-TEST.md` §4.5 — in all pickers, **Enter fills the pane and does not
run**. That rule extends to every new source here.
