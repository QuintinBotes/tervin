# Spec 21 — The surfaces the v3 build has and the app does not

Last of the design stage. Spec 15 established the five zones, spec 20 made them Paper
Chrome; this fills the two nav entries that open nothing, corrects one that opens the wrong
thing, and settles two questions of *where a thing lives* that three earlier specs have been
routing around.

## Context

`Tervin Workspace v3.dc.html` navigates seven surfaces: Terminal, Plan, Agents, Review,
Debug, Tasks, History. `ui/src/App.tsx:65-72` has five — Terminal, Plan, Agents, Review,
History — and `ui/src/lib/store.ts:39` types exactly those five.

So the gap is not "a redesign". It is:

- **Debug Bench** — designed in `spec_file.md:292-334` as a layout mode, present in the v3
  nav, and absent from the app. The data it renders already exists: `ui/src/lib/api.ts:83`
  carries `diagnostics: ParsedDiagnostic[]` and `api.ts:513` carries
  `{ severity, message, at }[]`, and the only place either is exercised is
  `surfaces.dom.test.tsx:483`. Parsed, typed, tested, and shown to nobody.
- **Tasks** — `Activity` listed `tasks` and rendered nothing until spec 15.1 deleted the
  type. Spec 16 builds the Deck and background tasks; this builds the surface they live on.
- **History** — the surface exists (`ui/src/components/HistorySurface.tsx:38`) but it is not
  what the v3 palette describes it as: *"History — every command, every host"*. Today it is
  every command on this machine.
- **First run** — no first-run experience exists at all. Spec 19 designs the content; the v3
  build changes its *form*, and the two disagree. Recorded and resolved in 21.7.
- **Bridge and Hosts** — Paper Chrome §5.8 places both as **Settings tabs**, not surfaces.
  That contradicts an assumption three specs are carrying. Settled in 21.8.

**Dependencies.** 21.4 and 21.5 need spec 16 (Tasks, the Deck) and spec 07 (triggers). 21.6
needs spec 04 (Blocks over SSH) for the host attribution to be real. 21.7 needs spec 19 for
everything it detects. None of the others depend on anything but 15 and 20.

## Slices

### 21.1 — Seven surfaces in the nav, and the numbers that move

`store.ts:39` grows to seven members and `App.tsx:65-72` to seven entries, in the v3 order:
Terminal, Plan, Agents, Review, Debug, Tasks, History. History stays last for the reason
already written at `App.tsx:70` — *"the one you go to deliberately rather than watch."*

The keyboard numbers move. `ui/src/lib/keymap.ts:99-103` binds `mod+1` … `mod+5` with
History on `mod+5`; the v3 palette shows Debug on `⌘5`, Tasks on `⌘6`, History on `⌘7`.

That reassignment is safe *because* the action ids are stable: persistence keys on
`surface.history`, not on `mod+5`, so a user who has already rebound the History surface
keeps their binding and only the default moves. Verify that rather than assume it — spec 02
is what makes keymap persistence real, and this is the first slice that depends on it being
real.

Each new entry carries a badge only when it has something to report, and an absent badge is
absent rather than zero.

*Exit:* seven nav entries, each opening a surface that renders. `mod+1` through `mod+7` reach
them in order. A persisted custom binding for `surface.history` survives the default change,
asserted by a test.

### 21.2 — Debug Bench, part one: the log tab

Paper Chrome defines Debug Bench as *Terminal with a log tab plus the diagnostics dock*. That
sentence is the whole architecture: it is a composition of zones 3 and 2, not a new screen,
and the "interactive shell" `spec_file.md:292-334` asks for is the terminal well that is
already there with a different tab selected. Do not build a second terminal.

The log tab is a tab in zone 3's tab row alongside the shell panes, and it renders **in the
well**, because a log is machine output: `--well` background, well content colours, mono 12
at 1.55, and the same single scroll container per pane.

Sources are the ones that already exist, and a source producing nothing is absent rather than
shown empty:

- the thread event stream, including `runtime.unclassified` (spec 11)
- adapter and capability diagnostics
- PTY lifecycle events — spawn, exit code, signal
- the daemon's log, once spec 09 lands

Filtering is the point of the surface, so it is a control and not a menu: severity, source and
a text match, each visible as a state word rather than an icon, each showing what it excluded
(`312 lines · 40 hidden by filter`). A filter that silently drops lines is the same failure as
the app doing something and not saying so.

**No autoscroll unless already pinned to the bottom**, per Paper Chrome, with the status
rail's new-lines affordance from spec 15.5 as the counterpart. A log surface is exactly where
someone is scrolled up reading, and it is exactly where being yanked to the bottom loses their
place.

*Exit:* the log tab shows real events from at least two sources, filters visibly, and does not
move the viewport while the user is scrolled up. A source with nothing to report is not
listed.

### 21.3 — Debug Bench, part two: the diagnostics dock

A dock below the well at **172px** — the same height as the Review dock, deliberately, so the
two layout modes do not each invent a dock geometry. If one of them ever needs a different
height, that is a finding about the dock, not a licence to have two.

Diagnostics are **grouped by severity**, each group a count and a state word (`3 errors`,
`11 warnings`), collapsed by default below the highest severity present. The severities are
whatever the parser already produces (`api.ts:513`) — this slice renders a model it does not
get to redesign.

`spec_file.md`'s Debug Bench also asks for linked stack traces, ports, paths and files. Those
are cross-navigation, and they are the reason the surface is worth having: a path opens in
Review or in the editor, a port names the process holding it, a stack frame opens the file at
the line. Paths are full and mono and truncate mid-path, never mid-command.

The selected diagnostic drives the inspector's subject when the inspector is open, which is
how the "selected Thread context" requirement is met without a sixth zone.

*Exit:* a failing build produces grouped diagnostics with real counts, and clicking a path in
a stack frame opens that file at that line. The dock measures 172px, matching Review.

### 21.4 — Tasks: saved plans, with step detail

A task is a saved plan: a named sequence of steps that has been run before and can be run
again. The surface is a list on the left and step detail on the right — two columns inside
zone 2, which is what the surface area is for.

The list row carries what you need to decide whether to run it: name, what it runs, the
outcome of the last run as a state word and a concrete duration, and the next trigger if it
has one. The detail pane carries the steps, each with its own state, and the step is where
approval state appears — spec 15.6's rule applies, so a migration approved from the well
changes plan step 4 here from the same store update.

**A task that has never run says so** rather than showing an empty outcome column. `spec_file.md`
and the roadmap's standing constraints both land on the same rule: never claim a capability or
a result that does not exist.

Depends on spec 16, which owns Threads, worktrees and the Deck. This slice owns the surface
and must not fork the model — if the Deck and the Tasks surface disagree about what a task is,
the Deck is right and this is wrong.

*Exit:* a plan saved from the Plan surface appears in Tasks, runs from it, and shows per-step
state that matches what the Deck and the inspector show for the same run.

### 21.5 — Tasks: triggers

Spec 07 builds the trigger mechanism; this is where a trigger becomes visible and editable. A
trigger reads as a sentence — *when the build fails · on every push to main · at 09:00* — and
the surface shows both the condition and the last time it fired.

**A trigger is an execution path from a non-user event, and that is the whole security
question.** A command that runs because a file changed goes through the same Rules
classification, the same permission tier and the same audit entry as one the user typed.
Nothing about being triggered makes it pre-approved. Spec 13's sweep re-checks this; state it
here so it is designed in rather than audited on.

A trigger that would run something the current mode cannot approve unattended is shown as
`waiting` and stays waiting. It does not escalate itself.

*Exit:* a trigger fires, the resulting run appears in the Deck with the trigger named as its
cause, and its permission decision is recorded identically to a typed command. A test asserts
a triggered high-risk action is not auto-approved.

### 21.6 — History: every command, every host

`HistorySurface.tsx` exists and works. What it is missing is the second half of the v3
description: *every host*.

Today history is this machine's. Spec 04 makes Blocks work over SSH and in subshells, which is
what makes a host attribution real rather than a label — so this slice is the rendering of
something spec 04 produces, and it must not invent an attribution spec 04 cannot supply. A
command whose host is unknown says unknown; it does not default to `local`.

Work: a host column, a host filter, and the ability to distinguish the same command run in two
places. `HistorySurface.tsx:107,204,237,341` all carry `.label` section labels that spec 20.6
converts to 10.5px mono, so this slice and that one touch the same lines — land 20.6 first or
expect a conflict.

*Exit:* a command run over SSH appears in History attributed to that host, filterable by it. A
command with no known host is labelled unknown rather than local.

### 21.7 — First run: one dialog, four rows, no progress bar

**The conflict, stated plainly.** Spec 19 designs onboarding as a three-step flow, taken from
`docs/reference/tervin-workspace-v2.dc.html`: *"Where do you work today?"*, *"Here is what
Tervin found"*, *"How much do you want to see?"*. The v3 build replaces the wizard with a
single **620px dialog carrying four rows and an `n of 4 done` count** — no progress bar, no
illustration.

**v3 wins on form; spec 19 keeps the content.** A wizard implies an order that these steps do
not have — you can install shell integration before or after choosing a project, and neither
blocks the other — and it forces someone who only wants one of the four through all of them.
Four independent rows in one dialog is the same information without the false sequence. Spec
19's slices 19.2 (detection), 19.3 (consent) and 19.5 (the privacy line) are unaffected;
19.1's three-step framing is superseded by this slice and 19.4's disclosure level moves, per
below.

Why no progress bar: a bar implies both an ordering and a knowable duration, and these four
rows have neither. `n of 4 done` is derived by counting rows in the done state — one value,
computed, never stored. It is also the same rule as everywhere else in Paper Chrome: nothing
spins with no known end.

The four rows must cover spec 19's setup checklist. Read the exact labels off the v3 build
rather than inventing them; if the build names them differently, the build wins. What they
have to cover:

1. **Shell integration** — installed or not, with the one action that installs it.
2. **A project** — chosen and indexed, with its path in full mono.
3. **Agents on this machine** — found, each with its permission tier, per `profile.rs`
   discovery.
4. **Bring your setup across** — the import from the Claude desktop app, VS Code or a
   terminal, which is spec 19.2 and 19.3 entire.

Each row is a state word and one action, never an icon and a checkmark. A row that cannot
complete says why — a machine with no VS Code does not offer the VS Code path (spec 19.2), and
a count that says "6 folders" means six real folders.

The **disclosure level is not a row**, because it is a preference rather than a setup step and
it is the one thing in spec 19's step 3 that has a safe default. It ships as Standard, lives
in Settings, and the dialog's closing line points at it. Spec 19.4's constraint is unchanged:
Expert removes explanation, never a safety confirmation.

The closing line stays: **"Nothing leaves this machine until you run an agent."** Spec 19.5
requires a test behind it before it is displayed, and that requirement is not relaxed by the
change of form.

Behaviour: Escape closes and lands in a working terminal — it is never blocking. It re-runs
from Settings and from the palette, which in the v3 build already carries a
*"Run first-run setup"* row. As an overlay it sits above the palette (z40) and the approval
dialog (z50), which is only safe because it is mutually exclusive with them in practice.
Assert that rather than trusting it: **onboarding may not be open while a request is
pending**, because an approval hidden behind a setup dialog is the worst version of the app
doing something and not saying so.

*Exit:* first launch shows one 620px dialog with four rows and a derived `n of 4 done`.
Escape at any point leaves a usable app. Declining everything changes nothing on disk. A test
asserts onboarding cannot cover a pending approval.

### 21.8 — Bridge and Hosts are Settings tabs

Paper Chrome §5.8 places both in Settings. The v3 palette is the observable proof: its
*"Bridge — adapters and MCP tools"* and *"Hosts — connections"* rows are categorised under
Settings, alongside *"Rules and permissions"*, and not under Go.

**This contradicts an assumption three specs are carrying.** The previous version of spec 15
listed Bridge as a rail entry and warned *"do not let three specs each assume the other built
it"*; `03-completion.md:102-104,130` makes *"says so once in the Bridge panel"* a verbatim exit
criterion; `11-agent-ux.md:41,48` puts the unclassified-event count there. The rail is gone,
so the panel those three are waiting for does not exist and will not. **It is built here, as a
Settings tab**, and the three exit criteria are satisfied by the tab. Update their wording in
spec 12's docs pass so nothing is left pointing at a panel.

`ui/src/components/SettingsPanel.tsx:14-22` has five sections — Appearance, Shell integration,
Agents, Tervin Rules, About. It gains two:

- **Bridge** — adapters, MCP servers and tools. The framing from the mockup holds: *MCP
  supported, never required*. This is where a capability degradation is stated **once** rather
  than as a toast per keystroke (spec 03.2), and where `runtime.unclassified` events are
  counted per runtime with the raw payload reachable (spec 11). Both are counts of real
  things; a runtime with none shows none.
- **Hosts** — connections. `ConnectionsPanel.tsx` already exists and opens on `mod+shift+o`
  (`keymap.ts:104`). Do not build a second one: **the overlay is the switcher and the tab is
  the editor.** You pick a host from the overlay in the middle of work; you add, edit and
  remove one in Settings. State that division in both files, because two ways to reach the
  same list is how one of them ends up stale.

The fact list these tabs use is a named component with one pinned key-column width, per spec
20.9 — the v3 build renders it at 118px, 150px and 168px in three places, and this is one of
the three.

*Exit:* Settings has seven tabs. An unsupported shell produces exactly one Bridge notice, not
one per keystroke. The unclassified count is per runtime and its raw payload opens. A host
edited in Settings appears in the `mod+shift+o` overlay without a restart.

## Verification

```sh
pnpm exec vitest run && pnpm exec tsc --noEmit
cargo test --workspace
```

Manual: open all seven surfaces from the keyboard alone. Break a build and confirm the
diagnostics dock groups the failures and its paths open. Run onboarding on a fresh config
directory, press Escape at the second row, and confirm you land in a working terminal with
nothing written to disk.
