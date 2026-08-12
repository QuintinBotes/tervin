# Spec 20 — Paper Chrome

**Second to last.** Everything else has landed; this makes it coherent, and it is now a
named system with fixed values rather than an abstract polish pass.

## Context

Two things arrive here at once and they are not the same kind of work.

**The first is a behaviour problem that a restyle does not fix.** The user's verdict on the
interface, recorded in `HANDOFF.md` as parked item #16, was that it *"does not behave well
at all"* and that it is *"unclear what to do where and when"*. `HANDOFF.md` names the
pattern behind it:

> **The app does something and does not say so.** A plan proposed with no indication. A
> button that appears dead because the agent already moved on. A message queued silently
> while the agent is busy. A divider that looked unresizable because the target was 5px.
> Each was fixed individually; the pattern was not.

Repainting those surfaces in new tokens leaves every one of those bugs exactly where it is.
Spec 11 fixes the known instances; 20.1 and 20.2 fix the pattern, and they are
design-independent — start them on day one regardless of where the token work has got to.

**The second is Paper Chrome**, which is settled and specific. The one idea: *the terminal
well is always the darkest surface on screen; chrome is furniture around it.* Two chrome
themes, Paper (light) and Graphite (dark), are the same layout and the same type with a
different token set. A component never branches on theme — only tokens do. The well's
content palette is theme-independent, so a screenshot of output is the same artefact under
either chrome; only the well's background moves, and it stays the darkest thing on screen
in both.

The sources are `Tervin Design Spec.md`, the written brief, and `Tervin Workspace v3.dc.html`
as the reference build. Where the brief and the build disagree, the brief wins and the
disagreement is recorded below — there are four, and each is a real decision rather than a
typo.

## Do not build: `bypass` mode

Paper Chrome's keyboard table cycles the agent mode `plan → ask → auto → bypass`, exactly as
`docs/reference/tervin-workspace-v2.dc.html` did before it.

**`bypassPermissions` is deliberately absent from the product.** `README.md:85-87`:

> `bypassPermissions` is deliberately absent from the offered modes. A one-click way to
> disable every check cannot be reconciled with telling you your actions are reviewable.

`COMPETITIVE-SPEC.md` §5 lists it under what Tervin should refuse to build. Spec 18 records
the same refusal against the v2 mockup; the design brief inherited the error and does not
overturn the decision. The cycle ships as `plan → ask → auto`.

This is a filter, not a hardcoded list. The shipped code reads the mode set from the
adapter (`ui/src/components/ThreadPanel.tsx:957-958` —
`thread.info?.metadata?.modes`), which is correct, because a runtime that offers three
modes should not be shown four. An adapter reporting a bypass-equivalent mode has that mode
removed from the offered set with a stated reason, not silently accepted because it arrived
over the wire.

*Exit for this rule:* a test feeds an adapter capability advertising `bypassPermissions` and
asserts the mode never appears in the cycle or the status rail.

## Slices

### 20.1 — Audit: every state transition, and whether it is visible

Design-independent. Start here regardless of what the token work is doing.

Enumerate every asynchronous transition in the product and record what the user sees: every
queue, every debounce, every optimistic update, every "the agent already moved on" case,
every control enabled while it cannot act, every operation over ~200ms with no indication.

`docs/DESIGN.md:203` lists "Silent disabled controls" as an instant reject, and `HANDOFF.md`
records that all 63 Tauri commands are now called *"but check new fields, not just
commands"* — the six-in-one-day bugs were fields plumbed and never set.

Two known instances to seed the table, both found while extracting the v3 build, both of
which the redesign would otherwise carry forward: the status-rail inspector toggle changes
its label on surfaces where the inspector does not render (spec 15.4), and the palette's
"Approve the pending migration" row opens an approval dialog without checking that anything
is pending, so it can present a stale decision for an already-resolved request.

*Exit:* a written table of every transition, its current indication, and whether that is
adequate. This table is the input to 20.2 and the record that the pattern was addressed
rather than another instance of it.

### 20.2 — Fix what the audit found

Loading states, empty states, error states, queued states, disabled-with-a-reason states.

Two rules govern every fix: a control that cannot act says why, and nothing happens without
an indication that it happened. Paper Chrome adds a third that constrains *how*: a pending
approval pulses nothing, and an indicator never spins with no known end. The indication is a
word and a colour, not motion.

*Exit:* every row in 20.1's table is adequate or has a stated reason it is not.

### 20.3 — Verify the token baseline still holds

**This slice moved.** The repair now runs as spec 22, second in the order, immediately
after hardening — because thirteen specs of interface work sit between there and here, and
every one of them would otherwise be built and visually judged against square-by-accident
overlays and a typeface that has never loaded. When the tokens were finally fixed, all of
those judgements would need making again.

What remains here is verification: the guard test from 22.2 still passes, no new undefined
token has appeared across thirteen specs of UI work, and the values 22.1 chose are still
the Paper Chrome ones after the vocabulary change in 20.4.

*Exit:* the undefined-token test is green, and the seven names below resolve to the Paper
Chrome radii and type roles.

The original finding, kept for the record — seven CSS custom properties used in production
and defined nowhere:

| Undefined | Call sites |
|---|---|
| `--radius-lg` | `App.tsx:468`, `SavedCommands.tsx:282`, `CommandPalette.tsx:266`, `ApprovalSheet.tsx:80`, `SettingsPanel.tsx:62`, `CommandHistory.tsx:103`, `DirectoryJump.tsx:138` |
| `--radius-md` | `SavedCommands.tsx:357`, `SettingsPanel.tsx:125` |
| `--radius-sm` | `ApprovalSheet.tsx:159`, `GitPanel.tsx:51`, `SettingsPanel.tsx:195,500,828` |
| `--text-title` | `CommandPalette.tsx:296`, `ApprovalSheet.tsx:94`, `SettingsPanel.tsx:906` |
| `--text-heading` | `SettingsPanel.tsx:860` |
| `--tervin-font-mono` | `TerminalPane.tsx:688` |

`base.css:54-57` defines `--radius-chip/control/panel/system` and `base.css:30-39` defines
`--text-page` through `--text-tag`. None of the seven above is among them, so **every
overlay in the app currently renders with radius 0 and every dialog title at inherited
size.** The sticky command header falls back to generic `monospace`.

Do this first. A restyle measured against a broken baseline cannot tell an improvement from
a repair, and someone will spend an afternoon deciding whether the palette's new corners
came from Paper Chrome or from a variable that finally resolved.

Resolve each to its Paper Chrome value: overlays 10, panels and well 8, inner cards 6,
controls 5; dialog title to the surface-title role, 16/600/-0.01em.

*Exit:* a lint or unit test walks every `var(--…)` reference in `ui/src` and asserts the name
is defined in `base.css` or written by `applyTheme`. It fails today; it passes after this
slice and keeps passing.

### 20.4 — The token layer: two chrome themes, and a well palette that is no longer fused to them

The largest slice, and the one with the real design conflict inside it.

**What ships today.** `ui/src/design/themes.ts:93-102` defines `Theme` as one object fusing
`SurfaceTokens` and a 16-colour `AnsiPalette`. `applyTheme` (`themes.ts:794-824`) writes 19
`--tervin-*` chrome variables; `toXtermTheme` (`themes.ts:831-857`) builds the terminal from
the *same* object — background, foreground, cursor and selection all come from `surface`,
and only the 16 hues come from `ansi`. Fifteen themes exist and 167 `var(--tervin-*)` call
sites read from them.

**What Paper Chrome specifies.** Eighteen tokens, two themes, every value pinned:

| Token | Paper | Graphite |
|---|---|---|
| `--bg` | `#F2EFE8` | `#141514` |
| `--panel` | `#F8F6F1` | `#1B1D1C` |
| `--panel2` | `#EDE9E0` | `#171918` |
| `--line` | `#DCD6C9` | `#232624` |
| `--line2` | `#E6E1D6` | `#232624` |
| `--ink` | `#1B1D1C` | `#E5E8E5` |
| `--muted` | `#5F6B66` | `#909894` |
| `--dim` | `#8A928E` | `#5d635f` |
| `--acc` | `#2F6F67` | `#68AEA5` |
| `--accInk` | `#F2EFE8` | `#141514` |
| `--amBg` | `#F7EEDC` | `#221F17` |
| `--amLine` | `#E4CD9E` | `#4A4030` |
| `--amInk` | `#6B4E14` | `#E0C48F` |
| `--amDot` | `#B98A3A` | `#D5AB68` |
| `--grn` | `#3F7A3A` | `#85BC7E` |
| `--red` | `#A4504C` | `#D77D79` |
| `--well` | `#141514` | `#0E0F0E` |
| `--wellLine` | `#141514` | `#1B1D1C` |

Three name mappings from what exists: `raised` → `panel2`, `hairline` → `line2`,
`accent` → `acc`. Three token families are new and nothing in the app expresses them today:
the four amber-block tokens (there is no rendering of a pending approval as a block at all),
`--accInk`, and `--well`/`--wellLine`.

`--wellLine` equalling `--well` under Paper is deliberate and not a copy error. Against
light chrome a dark slab needs no outline — the value gap already draws the edge, and a
lighter rim around a dark rectangle reads as a bevel. Under Graphite the well sits only one
step below `--panel`, so it gets a line. The v3 build ships `#242726` there and is wrong;
see the conflicts section.

**The split.** Divide the fused type:

- `ChromeTheme` — exactly two, owning all eighteen tokens above.
- `TerminalPalette` — N, user-selectable, owning the 16 ANSI hues plus foreground and
  cursor. **`terminalBg` is deleted from the palette entirely**, because the well background
  is chrome-owned by decree.

`SurfaceTokens.terminalBg/terminalFg/cursor/selection` (`themes.ts:63-70`) leave the chrome
type in the same change.

Selection becomes two fixed values rather than fifteen: `rgba(47,111,103,0.22)` under Paper,
`rgba(104,174,165,0.25)` under Graphite. Since the well is always dark, the well's selection
tracks the *chrome* theme's accent, not the palette's — which is the first consequence of
the split that an implementer will find surprising and it is correct.

**Define `--mono` and `--sans` as tokens.** The v3 build has neither: it interpolates
`'IBM Plex Mono',monospace` into roughly ninety inline style strings, which means the
user-selectable mono face is wired to nothing. Tervin already has `--font-mono`
(`base.css:71-72`) and must keep the indirection.

*Exit:* switching Paper ↔ Graphite changes only `document.documentElement` custom properties
— a test asserts no component's rendered class list or inline style differs between themes.
`grep -rn "#[0-9A-Fa-f]\{6\}" ui/src/components/` returns nothing except the theme swatch
(20.4a below). Prompt frameworks still render correctly under a palette change with the
chrome unmoved.

**20.4a, the one legitimate hardcode.** The Settings theme swatch is a 26px gradient chip
showing both halves of a theme at once —
`linear-gradient(135deg,#F2EFE8 0 55%,#141514 55% 100%)` for Paper. It cannot read tokens,
because it must show the theme you are *not* in. Annotate it as the single exception so the
grep above has one allowed hit and nobody later "fixes" it.

### 20.5 — The well is the darkest surface

The one idea, made true. Today it is false: `SurfaceTokens.terminalBg` is documented
*"Usually equal to `panel`"* (`themes.ts:63-64`) and in the default theme it is literally
`BRAND.graphite900` — the same value as the chrome panel. The well is not darker than the
chrome; it *is* the chrome. Four themes go further and ship a light well —
`tervin-light` `#FFFFFF`, `paper` `#FDFCF8`, `porcelain` `#FFFFFF`, `sandstone` `#FAF7F2` —
which Paper Chrome forbids outright.

Work:

- `TerminalPane.tsx:660-670` gains `border-radius: 8px` and `border: 1px solid var(--wellLine)`
  and reads its background from `--well`.
- Well *content* colours are pinned and do not move with the chrome: prompt `#68AEA5`, text
  `#E5E8E5`, output `#AEB5B1`, pass `#85BC7E`, warn `#D5AB68`, fail `#D77D79`. Worth knowing
  while implementing: these are not invented. `#68AEA5` is `BRAND.teal` (`themes.ts:119`) and
  `ansi.cyan`; `#E5E8E5` is `BRAND.ink`; `#AEB5B1` is `BRAND.ink2`; the last three are
  `ansi.green/yellow/red` of the default theme. Paper Chrome is pinning `tervin-dark`'s well,
  not authoring a new one.
- The sticky command header (`TerminalPane.tsx:672-706`) is inside the well, so it uses well
  content colours, not chrome tokens.

*Exit:* under both themes, the computed background of the terminal pane is strictly darker
than every chrome surface behind it — a test asserts the relative luminance ordering rather
than the hex, so it keeps holding if a token moves. No theme ships a light well.

### 20.6 — Type: IBM Plex, bundled for the first time

`base.css:68` declares `--font-ui: "Geist", "Inter", system-ui, …`. There is no `@font-face`
anywhere in the repository, `ui/src/design/fonts.css` does not exist despite
`docs/DESIGN.md:206-210` claiming it bundles the faces locally, and `index.html` loads
nothing. **The shipped app has never rendered in its own declared interface face.** It
renders in `system-ui` and always has.

So this is a first font implementation, not a swap, and it will move measurements
everywhere. Budget for that rather than being surprised by it.

- IBM Plex Sans 400/500/600 and IBM Plex Mono 400/500/600, as woff2 subsets under the
  existing `font-src 'self' data:` CSP. `font-display: block`, not `swap`: a desktop tool
  that reflows its chrome a beat after launch looks broken, and the faces are local so the
  block is measured in milliseconds.
- Roles, and nothing below 10.5px: surface title 16/600/-0.01em · project name 13.5/600 ·
  body 12.5/400/1.5 · secondary 12 · terminal command 12.5 mono · terminal output 12
  mono/1.55 · meta 11-11.5 mono tabular · section label 10.5 mono/0.08em/caps. Weight never
  exceeds 600.
- Delete `--text-page: 27px` (no Paper Chrome role is above 16) and `--text-tag: 10px`, which
  is below the floor and is used at `App.tsx:715,721,727` for the nav badges and
  `PaneTree.tsx:158` for the remote pane label.
- `.label` (`base.css:126-131`) is 11px sans today and becomes 10.5px mono. One rule; every
  call site inherits — `App.tsx:415,506`, `PlanSurface.tsx:371,391,404`,
  `HistorySurface.tsx:107,204,237,341`.
- Add `font-variant-numeric: tabular-nums` wherever meta numerals live. `.tabular` exists at
  `base.css:115-118` and is applied inconsistently; the v3 build applies it nowhere at all.

**The mono face is user-selectable**, so never rely on a specific advance width and never on
ligatures. The picker (`SettingsPanel.tsx:169-178`) drops from eight faces to the four Paper
Chrome names: IBM Plex Mono, JetBrains Mono, SF Mono, Berkeley Mono. Re-measure every layout
that assumes character widths: `ThreadPanel.tsx:1182-1230` (`width: 52`, `width: 108`,
`paddingLeft: 172`) and `ReviewPanel.tsx:220-224` (`width: 44` gutters).

**Keep a Nerd Font in the terminal's own chain.** `store.ts:181-185` defaults the terminal to
`"MesloLGS NF", "JetBrainsMono Nerd Font", …` for a stated reason — powerlevel10k draws
itself out of patched glyphs, and IBM Plex Mono is not patched. The UI mono becomes Plex; the
terminal font chain keeps a Nerd Font fallback. These are two settings and this slice must
not collapse them into one.

*Exit:* the app renders in IBM Plex Sans with the network disabled and on first frame. A test
asserts every `@font-face` `src` is a bundled path. Switching to each of the four mono faces
leaves no clipped or overlapping column on the Thread timeline or a unified diff.

### 20.7 — Geometry: spacing, radius, control heights, elevation

Mechanical, wide, and independently landable — nothing here changes behaviour.

- **Spacing.** `base.css:19-27` is 4,6,8,9,11,13,16,18,22. Paper Chrome is 2,4,6,8,10,12,14,
  16,20,26. `13` survives as panel padding; `9`, `11`, `18` and `22` do not, and they are
  load-bearing — `--sp-4: 9px` and `--sp-5: 11px` are the padding of every input
  (`base.css:189`) and every list row (`base.css:435`). That is roughly 200 `var(--sp-N)`
  call sites to re-decide. Fixed insets: panel padding 13, surface 14, gaps 12-14.
- **Radius.** 5 controls · 6 inner cards · 8 panels and well · 10 overlays. Only state dots
  are round. `--radius-chip: 4px` and `--radius-system: 12px` have no Paper Chrome
  equivalent; keep `12` for the macOS system notification and document it as deliberately
  outside the system, because at that point it is the OS speaking and not Tervin
  (`docs/DESIGN.md:136-138`).
- **Fixed dimensions.** Top bar 44 (from 42, `base.css:48`), status rail 26 (from 25,
  `base.css:51`), inspector 330, review tree 236, review dock 172, controls 24/26/28/30.
- **Elevation exists only on overlays**, and is one value: `0 24px 60px rgba(20,21,20,0.28)`.
  `base.css:66` currently has `0 22px 56px rgba(0,0,0,0.55)`, which under Paper's `#F2EFE8`
  reads as a smear rather than a lift. Nothing else in the product casts a shadow — output
  never becomes cards and blocks never get shadows.
- **One scrim, one value.** `.overlay-scrim` is `rgba(10,11,10,0.6)` (`base.css:468`) and four
  dialogs roll their own with `color-mix` against `--tervin-bg`: `App.tsx:453`,
  `SettingsPanel.tsx:47`, `ApprovalSheet.tsx:67`, `CommandPalette.tsx:251`. A `--bg`-derived
  scrim under Paper chrome is a *light* wash, which inverts the effect entirely. Every dialog
  uses `.overlay-scrim` at `rgba(20,21,20,0.42)`.

*Exit:* `grep -rn "borderRadius\|boxShadow\|rgba(" ui/src/components/` finds no literal outside
`base.css`. Screenshots at 1440px under both themes show a single shadow value on overlays and
none anywhere else.

### 20.8 — The five zones at their Paper Chrome dimensions

Spec 15 built the zones; this applies the system to them, zone by zone, and pins the numbers
against the reference build. Do not re-litigate the IA here — 15 settled it, there is no
sixth zone, and there is no activity rail.

Per zone the work is the token application plus the details 15 deliberately left to this
spec: the top bar sits on `--bg` with a `--line` bottom rule and *no* fill, while the status
rail is painted `--panel2` with a `--line` top rule. That asymmetry is intentional — the rail
is a floor and the bar is not a ceiling — and it is the kind of thing that gets "corrected"
by someone tidying up unless it is written down.

The 2px seam rule and the accent discipline are enforced here. Teal is the only non-semantic
colour and appears only as: a 2px nav underline, a 2px left border on a selected row or state
block, a 1px button outline, a prompt glyph, a fill on a button no taller than 30px, and the
seam. Green, amber and red are states only, never emphasis. No second accent, no gradients
except the seam, no glows, no tinted section backgrounds.

*Exit:* measured screenshots match the reference build's zone dimensions at 1440px. A grep for
`--acc` in a `background` position finds only controls at or below 30px tall.

### 20.9 — The component vocabulary, including the eight the brief does not name

Paper Chrome's §5 enumerates Block, Row, Pill, Card, State chip, Permission card, Dialog, Diff
and Palette. Build those, and pin the eight controls the v3 build ships that §5 never
enumerates — because they exist either way, and unnamed components are how a system acquires
five button heights.

| Component | Why it needs pinning |
|---|---|
| **Shell button** | §5 has no Button entry at all. The build ships five heights (23/24/26/28/30) across two radii (5 below 30, 6 at 30) and three fill recipes. Pin the set: 24 inline, 26 shell, 30 dialog footer; radius 5 below 30 and 6 at 30; fills accent / outlined / outlined-with-red-text. |
| **Waiting chip** | 26px, radius 5, sans 12 — a different control from the state chip's 20/4/11, named in the zone list and specified nowhere. |
| **Nav badge** | A bare 10px mono count with no chip. Becomes 10.5px under the type floor. |
| **Fact list** | A two-column definition row with a fixed key column. It recurs at three widths in the build — 118 in the approval dialog, 150 in Settings→Shell, 168 in Settings→Bridge — which makes it a real component with drift, not a one-off. Pick one width, state it, use it. |
| **Command echo slab** | A `--well`-coloured, borderless, radius-7 slab of well-content text living inside chrome, in the approval dialog. It is not the Block (no prompt glyph, no state border, no meta) and not the well (no border, wrong radius). Either fold it into the Block or declare it a named component at radius 8 with a `--wellLine` border, so the app has exactly one way to show a command inside chrome. |
| **Changed-file link** | Transparent, zero padding, teal mono text. The one sanctioned use of teal as *text* rather than as a line; write the exception down or it will be copied. |
| **Theme swatch** | The 26px gradient chip from 20.4a. |
| **Tab label** | Zone 3's tabs are plain mono labels with no chrome. §5 has no tab entry, so without this row someone builds a button. |

**The palette result row is a fourth row geometry** — 8px/14px padding, no `--line2` rule, no
2px left border, no selected state — and the brief sanctions it inside the Palette component.
An implementer building one shared Row primitive will not be able to reuse it. Say so in the
code, so the second row geometry is a decision rather than a discovery.

Two behavioural requirements that are part of the components, not decoration:

- **Copy is a word, not an icon.** State reads `waiting`, `running`, `done`, `failed`,
  `paused`, and colour reinforces the word rather than replacing it. Icons never replace state
  words in dense rows.
- **Secret values are never rendered.** The permission card shows
  `sqlx migrate run --database-url $DATABASE_URL` — the variable, never its contents. This is
  a component rule because the component is where it gets violated.

*Exit:* one file enumerates every component with its pinned geometry, and a visual test renders
each once per theme. `grep -rn "height: 2[0-9]" ui/src/components/` finds no control height
outside the pinned set.

### 20.10 — Restyle the surfaces that already exist

Everything above is infrastructure; this is where the app changes. Independently landable
per surface, in this order, because each is a smaller blast radius than the last:

1. **Overlays** — `CommandPalette`, `ApprovalSheet`, `SettingsPanel`, `SavedCommands`,
   `CommandHistory`, `DirectoryJump`, `SearchOverlay`. They share the scrim, the radius-10
   card and the one shadow, so they land together and they are where 20.3's undefined radii
   were doing the most visible damage.
2. **Terminal** — pane chrome, tab row, `PaneTree`, `BlocksPanel`, `PathComplete`,
   `PromptHistory`.
3. **Agents** — `ThreadPanel`, `AgentDeck`. The timeline's fixed widths are re-measured here
   against the new mono face.
4. **Review** — `ReviewPanel`, `GitPanel`, `FileExplorer`. Tree 236, dock 172.
5. **Plan and History** — `PlanSurface`, `HistorySurface`, `ProjectInstructions`.
6. **Settings** — `SettingsPanel`, including the two new tabs from spec 21.8.

*Exit:* every surface renders correctly under both themes with no component branching on
theme. `reachable.test.ts` still passes. `surfaces.dom.test.tsx` still passes or has been
updated with a stated reason per change.

### 20.11 — Motion

`base.css:60-63` defines 150/180/220ms; every transition in the stylesheet uses 150.
`docs/DESIGN.md:171` states "150–220ms". Paper Chrome is stricter and the strictness is the
point.

- 120ms ease for **colour, border and opacity only**. Nothing above 180ms anywhere. Delete
  `--motion-slow`.
- **Never animate layout.** The inspector appears and disappears; it does not slide. Pane drag
  follows the pointer with no easing. `.toggle::after` (`base.css:550-563`) transitions `left`
  and must become a colour or opacity change, or lose its transition.
- Overlays fade the scrim 0 → 0.42 over 120ms with at most a 2px upward settle on the card.
- Streaming output appends with no animation and no autoscroll unless already pinned to the
  bottom — with the rail's new-lines affordance from spec 15.5 as the required counterpart.
- A pending approval pulses nothing. It is amber and it stays amber. Nothing spins with no
  known end.
- Theme switch is instant. Do not transition tokens — a 120ms crossfade of eighteen custom
  properties is eighteen simultaneous repaints and it looks like a fault.
- `prefers-reduced-motion` drops every transition to 0ms. Every, not most: `base.css:158-166`
  has the block already and it must cover the new rules.

*Exit:* a test parses the compiled CSS and asserts no `transition` names a layout property and
no duration exceeds 180ms. With reduced motion on, `getComputedStyle` reports 0s on every
transitioned element.

### 20.12 — States and accessibility

`spec_file.md:356-358` requires accessibility and screen-reader semantics, high-contrast mode
and reduced-motion support in the *baseline*. None of the workspace chrome has it;
`screenReaderMode` is xterm's accessibility buffer, which is necessary and not sufficient.

Paper Chrome pins the interaction states, and one of them is a direct contradiction of what
ships:

- **Hover** moves borders to `--acc` and row backgrounds to `--panel2`. It never moves layout.
- **Focus is `outline: 2px solid var(--acc)` at `offset: 2px`, always visible, never
  suppressed.** `base.css:144-148` uses 1px at 1px offset, and `base.css:151-154` suppresses
  the ring entirely inside the terminal surface. That exemption has to go, or narrow to the
  xterm textarea only: the well now contains its own controls — inline approve and deny, pane
  close — and they need a ring. The original reasoning ("the terminal draws its own cursor")
  applies to the text area and not to buttons that happen to sit over it.
- **Disabled** is 45% opacity with pointer events off — *and* a stated reason, per 20.2.
- **Selection** is the two fixed values from 20.4.

Then the rest of the accessibility pass: roles, labels, focus order and live regions on every
surface; full keyboard reachability, which is `spec_file.md` acceptance criterion 5 and is
verified here rather than assumed; a real high-contrast theme measured rather than eyeballed;
and the close control in the inspector's thread card, which in the v3 build is a bare `×` glyph
with no `aria-label` and no `title`.

Shared with spec 18.6, which owns the macOS half — VoiceOver rotor behaviour and the system
reduced-motion setting. This slice owns the chrome.

*Exit:* VoiceOver reaches and announces every control. Every workflow in `docs/MANUAL-TEST.md`
completes without a mouse. Contrast is measured at every text size, including 10.5px section
labels against `--panel2` under both themes.

### 20.13 — First-run and empty-product experience

What Tervin looks like with nothing in it: no Blocks, no Threads, no history, no project.
Every list has an empty state that teaches rather than apologises.

Paper Chrome's rule: an empty state describes the next action in one sentence and offers the
shortcut that performs it. The model is the palette's own, from the v3 build: *"No matches.
⌘⏎ runs it in the focused pane."*

Pairs with spec 19 and spec 21.7: onboarding is the guided path, this is what the app looks
like for someone who dismissed it.

*Exit:* every list and surface has a written empty state naming one action and one shortcut.
Skipping onboarding still lands somewhere legible.

### 20.14 — Density and hierarchy at real sizes

Verify at the sizes people use: a 13" laptop, a 27" display, and the 720px minimum width the
window config permits.

`spec_file.md`: *"At smaller widths, collapse panes rather than shrinking controls into
unusable layouts."* Spec 15.8's thresholds are the mechanism; this is the check that they hold
after the type change, which moves every measurement in the app because the app has never
rendered in its declared face.

*Exit:* every surface is usable at 720px and does not look sparse at 2560px, under both themes.

### 20.15 — One design authority

`docs/DESIGN.md:3-5` calls itself *"The authoritative visual specification. Read this before
building any Tervin surface. It is dark-first…"*, and `CLAUDE.md:140` routes all interface work
through it. It then contradicts Paper Chrome on nearly every value: Geist and JetBrains Mono
(line 51), top bar 42 / tab strip 29 / panel header 38 / status rail 25 (line 77), motion
150–220ms (line 171), radius 4/5/6–8/12 (lines 79-80), shadow `0 22px 56px rgba(0,0,0,0.55)`
(line 86), spacing 4,6,8,9,11,13,16,18,22 (line 74), and a ban on *"Body text below 12px"*
(line 203).

It already contradicts itself: line 65 sets a 10.5px floor, line 103 specifies a 10px event
tag, and `base.css:39` implements the 10px.

Rewrite it as Paper Chrome in the same commit range as the token work and update the
`CLAUDE.md` pointer. Two authoritative specs disagreeing is worse than either being wrong,
because every future agent reads `CLAUDE.md` first and will build to whichever it finds.

Land `Tervin Workspace v3.dc.html` in `docs/reference/` and repoint `docs/DESIGN.md:7-10`,
which currently names `tervin-workspace-v2.dc.html` *"the layout blueprint… the visual source
of truth"*. The v2 build's root is `background:#141514; color:#E5E8E5; font-family:Geist,…`
with the well equal to the panel — the exact relationship Paper Chrome overturns. Leaving it
as the pointer means the doc chain still leads implementers to dark-first Geist chrome no
matter what the tokens say.

*Exit:* `docs/DESIGN.md` describes what shipped, every number in it is checked against
`base.css`, and `docs/reference/` contains the v3 build. Spec 12 (docs truth) re-verifies.

## Conflicts recorded

**The theme question, and its answer.** Fifteen fused themes are not compatible with two
chrome themes and a fixed well *as the code is shaped*. The answer is to split rather than to
replace, per 20.4, and then three follow-ups: drop or re-author the four light-well palettes
(a `#356A93` blue on `#141514` fails contrast, so they are not liftable intact); ship the
`tervin-dark` palette as the default; and treat Paper Chrome's six "fixed" well-content values
as **that default palette's values, not a global lock**. A hard lock would break the stated
reason the palettes exist at all — `themes.ts:10-14`: *"Prompt frameworks such as oh-my-zsh,
powerlevel10k, starship, and spaceship draw themselves out of the ANSI palette, so a theme
that only styled the chrome would leave every prompt looking wrong."* If the lock is genuinely
wanted, keep it as a palette named "Fixed" that is the default and non-removable, and label
the others as an explicit opt-out. Either way the well *background* stays chrome-owned and is
never a choice.

**The brief and the v3 build disagree on the Paper well.** The build ships `--well #131413`
and `--wellLine #242726` under Paper chrome; the brief says `#141514` and `#141514`. Take the
brief, for the reason given in 20.4: a lighter rim around a dark slab on light chrome reads as
a bevel, and Paper Chrome has no bevels. The Graphite column of the build matches the brief
exactly, which is what makes the Paper divergence look like drift rather than intent.

**The v3 build's root font-size is 13px**, which is not a value in the type scale. Every
element that fails to set a size inherits a size the system does not contain. Set the root to
a scale value and give every text element an explicit role.

**The v3 build has no transitions at all.** There is no `transition` property in the file, so
theme switching, hover and overlay entry are all instant. The 120ms colour rule is absent
rather than implemented, and an implementer reading the build as the source of truth will
conclude that hover has no transition. Implement the brief.

**Two columns maximum is superseded, not ignored.** Handled in spec 15, and 20 must not
reintroduce it — `docs/DESIGN.md:142` states it and 20.15 deletes it.

## What does not change

The copy rules survive any redesign, because they are the product's voice rather than its
appearance: precise, candid, calm, technical. *"Agent is waiting for approval"*, never *"AI
needs your attention!"*. *"23 passed, 1 failed"*, never *"Almost there!"*. Never magical,
seamless, supercharge, AI-powered. No emoji anywhere. Numbers concrete and mono. Paths full
and mono, truncated mid-path and never mid-command.

Three engineering invariants outrank every design consideration and any of them is grounds to
refuse a design detail:

- **Terminal bytes never become JSON, and per-frame state stays out of React.** A design
  requiring per-frame React work is not implementable at this product's throughput.
- **Unhandled keys reach the terminal.** No new interaction swallows a key `vim` or `emacs`
  needs.
- **Nothing fixed-position over terminal output.** The terminal is the centre; that is the
  product thesis, not a layout choice.

And the honesty rule outranks all of it. A layout that looks better by implying a certainty
Tervin does not have is not a better layout.

## Verification

```sh
pnpm exec vitest run && pnpm exec tsc --noEmit
cargo test --workspace
```

Manual: `docs/MANUAL-TEST.md` end to end at 720px, 1440px and 2560px, under Paper and under
Graphite, once with VoiceOver on and once with reduced motion on, entirely without a mouse.
