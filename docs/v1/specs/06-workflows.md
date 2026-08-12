# Spec 06 — Workflows & notebooks: a local Warp Drive

`COMPETITIVE-SPEC.md` §4.4, P2. *"Close the local half, refuse the cloud half."*

## Context

Warp Drive stores workflows, notebooks, environment profiles and MCP server lists, synced
across a team. The sync half is refused permanently in §5 — team accounts, SSO, seats and
a hosted backend all require a server holding user data and an organisation to run it,
and Tervin's proposition is that it works against your own subscriptions with no account.

The stated alternative is `.tervin/` **committed to the repository**. That covers the real
team need — "everyone working on this repo should have these commands" — without a
backend, and it is reviewable in the same pull request as the code it describes.

Saved commands already exist (`crates/block-engine/src/saved.rs`) with `{{name}}` and
`{{name:default}}` templating and a deliberately strict parser so `${HOME}` and
`awk '{print $1}'` survive untouched. That is the single-command case. This spec is the
multi-step and prose cases.

## Slices

### 06.1 — `.tervin/` as a committed workspace directory
Define the layout: workflows, launch configurations (from spec 05.2), saved commands,
project instructions. Read on project open, with **consent before anything runs** — a
repository you cloned must not be able to execute on open.

*Exit:* opening a project with `.tervin/` lists what it found and runs nothing until
asked.

### 06.2 — Workflows
Multi-step named command sequences with typed parameters. Warp uses YAML and has an open
format; reading Warp's own workflow YAML is worth doing — it is a real corpus and costs
little, and `mcp.json` already sets the precedent of adopting someone else's format
rather than inventing a Tervin-shaped one users would hand-translate.

Each step is a command; each runs as a Block; a failed step stops the sequence and says
which. Steps go through Tervin Rules like anything else that runs.

*Exit:* a three-step workflow runs, produces three Blocks, and halts on the failing one.
A Warp-format YAML workflow loads.

### 06.3 — Notebooks
A document interleaving prose and runnable commands, with the output of each run kept.
This is Warp's notebook block, minus the share-as-a-link half.

The Block model already carries command, output, exit code, duration, diagnostics and
tests, so a notebook is a saved ordering of Blocks plus markdown between them.

*Exit:* a notebook can be authored, run cell by cell, saved to `.tervin/`, and reopened
with prior output intact.

### 06.4 — Markdown viewer
Needed by 06.3 and listed separately in Warp's feature set. Render markdown in a panel —
`README.md`, `AGENTS.md`, a notebook. No editor; this is a viewer.

`DESIGN.md` governs: no decorative cards, body text at 14px minimum, monospace for code.

*Exit:* a markdown file opens from the file explorer and renders.

### 06.5 — Export and share, honestly
A workflow or notebook exports as a file. **Not** as a link — there is no service to host
it, and §5 refuses building one. Say that plainly in the UI rather than offering a share
button that produces a file path and calls it sharing.

*Exit:* export produces a file. No copy in the UI implies a hosted share.

### 06.6 — Palette and search integration
Workflows and notebooks appear in the command palette (`ui/src/components/CommandPalette.tsx`,
which already fuses panes, surfaces, settings, agent profiles, Blocks, aliases and file
results) and in universal search.

*Exit:* typing a workflow name in the palette offers to run it.

## Refused, and stated here so it is not re-proposed

Team sync, a shared Drive, an account, a hosted share link. All of §5.

## Verification

```sh
cargo test --workspace
pnpm exec vitest run
```

Manual: create `.tervin/` in `~/tervin-testbed`, reopen, confirm nothing ran; then run a
workflow and confirm the Blocks.
