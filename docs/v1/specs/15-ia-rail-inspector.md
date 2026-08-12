# Spec 15 — The five-zone workspace and the context inspector

The filename is stable because `ORDER.json` keys on it. The subject is not: this spec was
"IA: rail, inspector, layout modes" and is now the five zones. The activity rail is out of
scope.

## The rail is out of scope, and why

Paper Chrome specifies five zones — top command bar, surface area, terminal canvas,
context inspector, status rail — and then says explicitly *"do not invent a sixth"*. An
activity rail is a sixth. Surface navigation lives in the top bar instead, as a row of
labels with a 2px accent underline on the active one.

This supersedes the decision the previous version of this spec recorded, which was to make
the rail an opt-in layout above a measured width threshold. That was a defensible hedge
between two documents that disagreed. It is no longer a hedge between two documents: the
design brief is the newer of the two and it decides.

It also happens to agree with the reason the code gave for going its own way.
`ui/src/App.tsx:6-11`:

> That is the design system's central layout rule — two columns maximum, a third pane
> means something collapses — and it is a real constraint rather than a stylistic one. A
> rail plus a terminal plus an inspector is three columns, which on a laptop leaves the
> terminal too narrow to be the thing you work in.

Paper Chrome resolves that arithmetic by deleting the rail rather than by rationing the
inspector: nav becomes 44px of vertical chrome that was already there, and the third
column is gone. The inspector is the only optional column, it is bound to `⌘B`, and the
brief calls it *"never permanent, never a chat window"* — which is the two-column rule
stated as a behaviour instead of a count.

**Consequence for the prose.** The two-columns-maximum sentences at `ui/src/App.tsx:6-11`,
`ui/src/lib/store.ts:36-37` and `docs/DESIGN.md:142` describe a rule that no longer
matches what ships, because a Review surface with a 236px tree and the inspector open is
three columns by any count. Delete them in 15.1 rather than leaving three files arguing
with the layout. What survives from them is *"collapse, never squeeze"*, which is a
behaviour and is kept in 15.8.

## The seam with spec 20

Spec 15 runs ninth; spec 20 (Paper Chrome) runs nineteenth. The division is:

- **15 owns structure** — which zones exist, what occupies each, what each derives from,
  and how the layout responds to width.
- **20 owns appearance** — tokens, type, radii, spacing and the component vocabulary
  applied to those zones.

Numbers appear in both, because a zone with no height is not a structure. Every number
here is Paper Chrome's; 20 verifies them against the v3 reference build and is the place
they change if the build and the brief disagree.

## Slices

### 15.1 — Delete the rail, and the types that describe it

`ui/src/lib/store.ts:42-51` declares `Activity` with nine members — `workspace`, `files`,
`git`, `threads`, `tasks`, `history`, `connections`, `bridge`, `settings` — and renders
none of them. `ui/src/lib/keymap.ts:91` binds `mod+shift+b` to `rail.toggle`, labelled
*"Toggle activity rail"*, and it toggles a rail that does not exist. This is the
built-and-unreachable pattern in its purest form: an entire information architecture typed
and dead, which is exactly what `reachable.test.ts` was written to stop and cannot catch,
because a type is not a component.

Delete `Activity`, delete `rail.toggle`, delete the two-columns-maximum prose named above.

`InspectorTab` (`store.ts:53-60`) goes with it, but by disposition rather than deletion —
each of its seven members has a home under the five zones, and the point of writing them
out is that none of them silently disappears:

| Tab | Where it lives now |
|---|---|
| `thread` | The inspector's thread card. The only one that stays in the inspector. |
| `review`, `files`, `git` | The Review surface. It already exists (`ReviewPanel.tsx`). |
| `diagnostics` | The Debug Bench diagnostics dock (spec 21.3). |
| `connections` | Settings → Hosts (spec 21.8). |
| `details` | The thread card's meta line — tier and thread id. |

The pinning requirement at `spec_file.md:277` — pin a Block, Thread, file, diff, test, task
or diagnostics group — survives as a change of *subject*, not a change of tab. The
inspector shows the focused thread by default; pinning freezes it on one subject until
unpinned. That is a one-field change, and it is what "never permanent" allows: a pinned
inspector is still closable.

*Exit:* `grep -rn "Activity\b" ui/src/lib/store.ts` returns nothing, `rail.toggle` is gone
from `keymap.ts`, and `pnpm exec tsc --noEmit` passes. No type in `store.ts` describes UI
that does not exist.

### 15.2 — Zone 1: the top command bar

44px, full width, `flex: 0 0 44px`, and no surface ever changes its height. Today
`base.css:48` says 42 and `App.tsx:664` consumes it.

Contents, left to right, from the v3 build: mark · identity button · nav · spacer ·
waiting chip · Search · Theme · Settings.

- **Identity** is one button covering the mark, project name and branch, and it opens the
  palette. Three separate targets that all open the same thing is three chances to miss.
  The dirty count sits inside it as `●3` in amber.
- **Nav** is the surface list. It is seven entries after spec 21, not five: Terminal, Plan,
  Agents, Review, Debug, Tasks, History. Active is the 2px accent underline and weight 600;
  inactive is `--muted` at 400. A badge is a bare mono count in `--dim`, not a chip —
  `App.tsx:700-727` currently renders 10px badges, which is below the 10.5px floor and is
  fixed in 20.6.
- **The waiting chip** renders only while something is actually waiting, and it is a
  different control from the state chip in the component vocabulary: 26px tall, radius 5,
  sans 12px, against the state chip's 20/4/11. Spec 20.9 pins both so the difference is
  deliberate rather than drift. Its label is a count and a noun — *"1 agent waiting on
  you"* — derived from `ThreadState::needs_user()`, not stored.
- **Settings** is not part of the nav underline set. It is a shell button that takes a 1px
  accent border while the Settings surface is open.

**A defect in the v3 build not to copy:** the Theme button's label shows the *current*
theme rather than the one it switches to. A button labelled "Paper" while Paper is active
reads as a state, not an action, and the identical geometry to the Search button next to it
makes it read as one more destination. Label it with the target, or label it "Theme" and
show the state elsewhere.

*Exit:* the bar measures 44px at every surface and every width. Every nav entry opens
something. The waiting chip is absent — not greyed, not zero — when nothing waits.

### 15.3 — Zones 2 and 3: the surface area and the terminal canvas

Zone 2 is the growing region between the bar and the rail: `flex: 1; min-height: 0`, with
14px padding and 14px gaps. `min-height: 0` is load-bearing — without it a long output
column pushes the status rail off the bottom of the window, and the app never scrolls as a
page.

Zone 3 is the terminal canvas inside it: a tab row, then the well. The tab row is plain
mono labels at 11.5px with no chrome — no border, no background, no radius — because a
tab that looks like a button competes with the well for the eye, and the well is the
subject. Selection is weight and colour only.

The well is one scroll container per pane, radius 8, 1px `--wellLine`, and it is always the
darkest surface on screen. Today `TerminalPane.tsx:660-670` gives the pane host
`background: var(--tervin-terminal-bg)`, 8px of padding, no radius and no border, and the
default theme sets `terminalBg` to the same graphite as the chrome — so there is no well,
only a region. Making it one is spec 20.5; making it a *zone* with a tab row above it is
here.

The tab row also carries the right-aligned keyboard hint (`⌘D split · ⌘B inspector`). It is
text, not a control, and it is the cheapest fix available for *"unclear what to do where
and when"*.

*Exit:* the surface area grows and the two fixed zones do not. At 400 lines of output the
status rail is still on screen. Selecting a tab changes only its own pane.

### 15.4 — Zone 4: the context inspector

330px, `flex: 0 0 330px`, appearing and disappearing on `⌘B`. It does not slide: layout
animation reflows text while someone is reading output, which is the one thing a terminal
must never do.

Two stacked cards, 12px apart, each scrolling independently — the container does not
scroll, so the thread card cannot be pushed off the top by a long activity log:

1. **The thread card.** State dot, thread name, `tier 1 · 3f2a`, close. Then the task in one
   sentence. Then either the permission card or the settled sentence — never both, and by
   swapping content rather than by hiding one, so nothing occupies space it cannot use.
   The dot derives from the thread's state; it is never written down.
2. **The activity card.** A section label, timestamped rows coloured by kind, and a footer
   pinned with `margin-top: auto` carrying the changed-file list. Each file cross-navigates
   to Review with that file selected — which is the inspector's whole job: it is where you
   find out where to go next, and then you leave.

The permission card inside the thread card runs the *same* `approve()`/`deny()` path as the
approval dialog and the inline block row. Three renderings, one decision path, one audit
entry. Spec 18.1 imposes the same rule on notifications.

**Never a chat window.** `spec_file.md`'s Terminal First mode says context opens as a
temporary panel and there is no permanently visible chat panel; Paper Chrome repeats it.
The inspector is a place you look, not a place you type.

**A defect in the v3 build not to copy:** the inspector column is conditional on the
Terminal surface only, so on Plan, Agents, Review, Debug, Tasks and Settings the status-rail
toggle and `⌘B` change a label and nothing else. A control that changes its label and does
nothing is precisely the failure spec 20.1 is auditing for. Either the inspector renders on
every surface that has a thread in context, or `⌘B` is disabled with a reason on the
surfaces where it cannot act.

*Exit:* `⌘B` opens and closes the inspector with no transition on any property except
colour and opacity. Approving from the permission card and approving from the dialog
produce identical state and identical audit records. On a surface where `⌘B` cannot act it
says so.

### 15.5 — Zone 5: the status rail

26px, `flex: 0 0 26px`, painted `--panel2` with a 1px top rule — the only zone with a fill
of its own, which is what makes it read as a floor rather than as more surface.
`base.css:51` says 25 today and `App.tsx:1043` consumes it.

Six slots. Five are text and are not interactive; only the trailing inspector toggle is a
button. `spec_file.md:280-290` requires: shell and cwd, host or SSH connection, git branch
and dirty state, agent mode and model where known, task progress, token and cost where
available, remote latency and reconnect state.

**"Where known" and "where available" are load-bearing.** A cost figure for a runtime that
reports none is absent, not zero. The latency slot follows §14.4's refusal — Tervin does
not print a number it did not measure.

Two derivations, not two stored strings: the agent slot is `2 running · 1 waiting` while
something waits and `2 running` when nothing does, computed from thread state; the mode
slot is the active mode verbatim, lowercase.

**Add the streaming affordance the v3 build omits.** Paper Chrome forbids autoscrolling away
from the user's scroll position. That is correct and it creates an obligation: if output
arrives while the user is scrolled up, something must say so. The rail is where —
`N new lines`, mono, clickable to jump to the bottom. Without it the rule turns a helpful
refusal into silent data loss, which is the same pattern as *"the app does something and
does not say so."*

*Exit:* the rail measures 26px. Every field present is real; a runtime reporting no cost
shows no cost. Scrolling up during output produces a new-lines count in the rail, and
clicking it returns to the bottom.

### 15.6 — One state, every surface

Paper Chrome: *"Approval state is ONE value rendered everywhere... Any surface showing a
thread's state DERIVES it, never caches it."*

Approving a migration must change, from a single state change: the inline block in the
well, the inspector's thread card, the corresponding plan step, the Deck row, and the
status rail. Five renderings, one value. The v3 build demonstrates the failure mode it is
guarding against — its approval dialog hardcodes an amber header dot in markup instead of
calling the same derivation the inspector uses, so the dot stays amber after the migration
resolves.

**The conflict, stated plainly.** "One value everywhere" is about *state*, not about *copy*,
and the difference matters because the product deliberately renders two different things
depending on whether a request was interceptable. `ThreadPanel.tsx:1210` appends
`· observed` to a risk chip when `risk.enforceable` is false, because Tervin distinguishes
an action it could have stopped from one it merely saw. A single derived state value that
flattened that distinction would make the interface claim a gate it does not have.

The resolution: the derived value carries the `enforceable` flag with it. Every surface
derives the same state *and* the same enforceability, and each renders both. Uniformity of
source, not uniformity of wording.

*Exit:* a test approves one request and asserts all five renderings changed from the same
store update, with no surface holding its own copy. An unenforceable request still reads
"observed" on every surface that shows it.

### 15.7 — Layout modes are compositions, not designs

`spec_file.md:292-334` names four modes. Paper Chrome defines each as an arrangement of the
same five zones, which means none of them is a new screen:

- **Terminal First** — Terminal with the inspector collapsed. The first-run default.
- **Mission Control** — the Agents surface. Depends on spec 16.
- **Review Desk** — the Review surface: 236px changed-file tree, diffs, 172px dock.
- **Debug Bench** — Terminal with a log tab, plus the diagnostics dock. Spec 21.2-21.3.

Written this way there is nothing to build here beyond making the four reachable by name
from the palette and remembering each one's pane geometry — the v3 build keeps `paneW`,
`drawerW`, `agentsW`, `reviewLeftW` and `reviewBottomH` separately for exactly that reason.

The value of writing it down is negative: it stops someone building four layouts. If a mode
needs a component the other three do not have, it is not a mode.

*Exit:* all four are reachable from the palette, each restores its own geometry, and no
component exists that only one mode uses.

### 15.8 — Width thresholds: collapse, never squeeze

`NARROW = 860` at `App.tsx:80` already hides the supporting column, with the reasoning
attached: *"a two-column compromise at 700px gives two unusable columns instead of one good
one."* Keep it, and add a second threshold above it at which the inspector force-closes.

Measure the second one rather than choosing it: the width at which 330px of inspector plus
28px of surface padding and gap leaves the terminal under 80 columns at the default mono
size. 80 columns is not a nostalgia figure — it is what `git log`, `cargo` output and
almost every `--help` page assume, and below it the well starts wrapping the thing the user
is reading.

Force-closed is not the same as toggled off: the user's preference is remembered and
restored when the window grows. A layout that forgets what you asked for is worse than one
that never offered it.

*Exit:* a written threshold with the measurement behind it. Resizing 1920 → 720 collapses
at both thresholds and never squeezes; growing back restores the inspector if it was open.
`docs/MANUAL-TEST.md` covers the round trip.

### 15.9 — Side-by-side diffs

`spec_file.md:786` requires unified or side-by-side, and Review Desk depends on it.
`git-service` already produces hunk-level diffs; `ui/src/components/ReviewPanel.tsx` has
only the unified view. Its gutters are hardcoded at `width: 44` (`ReviewPanel.tsx:220-224`),
which spec 20.7's mono change has to re-measure — noted here so the two slices do not each
assume the other checked.

*Exit:* a diff renders both ways and the choice persists across restart.

## Verification

```sh
pnpm exec vitest run && pnpm exec tsc --noEmit
```

Manual: resize from 1920 to 720 and confirm each transition collapses rather than squeezes.
Confirm `reachable.test.ts` still passes. Toggle `⌘B` on every surface and confirm it either
acts or explains why it cannot.
