# Tervin design system

The authoritative visual specification. Read this before building any Tervin
surface. It is dark-first, dense, quiet and keyboard-driven. **When in doubt,
remove something.**

The source mockups this is derived from are kept in
[`docs/reference/`](./reference/) — `tervin-workspace-v2.dc.html` is the layout
blueprint. They are the visual source of truth; this document is the extract that
implementation follows.

## What Tervin is

A correct, fast terminal whose development workflow becomes more legible when
coding agents participate. It is **not** a chat app with a terminal widget, and
**not** an IDE. The terminal is the centre; plans, diffs, tests, permissions and
agent state are progressive disclosure around it.

## Colour tokens

These hex values and no others.

| Token | Hex | Use |
|---|---|---|
| `--graphite-950` | `#141514` | App background |
| `--graphite-900` | `#1B1D1C` | Panels, terminal surfaces, hovered overlay rows |
| `--graphite-800` | `#232624` | Raised surfaces, selected segment, dividers-as-fill |
| `--line` | `#323634` | Borders on overlays and inputs |
| `--hairline` | `#1B1D1C` | Borders inside lists and panels |
| `--ink` | `#E5E8E5` | Primary text |
| `--ink-2` | `#AEB5B1` | Terminal output, secondary body text |
| `--muted` | `#909894` | Labels, metadata |
| `--dim` | `#5d635f` | Timestamps, keyboard hints, placeholder text |
| `--teal` | `#68AEA5` | Focus, selection, primary action, brand seam |
| `--teal-hi` | `#8CC9C1` | Hover on teal |
| `--green` | `#85BC7E` | Passing test or command |
| `--amber` | `#D5AB68` | Waiting, plan mode, pending review |
| `--red` | `#D77D79` | Failure, destructive action |

Overlay surface `#171918`. Overlay scrim `rgba(10,11,10,0.6)`. Block row hover
`#181A19`.

Diff add `rgba(133,188,126,0.10)` background on `#C7E0C2` text.
Diff remove `rgba(215,125,121,0.10)` background on `#E9BEBC` text.

**No** gradients, glassmorphism, purple/blue "AI" styling, neon, or glow. Colour
is never decoration — only state, focus, or brand.

## Type

UI: **Geist** (fallback Inter, system-ui). Machine text: **JetBrains Mono**.

| Size | Weight | Use |
|---|---|---|
| 27px | 600, −0.02em | Page title |
| 18px | 600 | Section |
| 14px | 600 | Subsection |
| 13px | 400 | Body |
| 12.5px | 400 | Control labels, list titles |
| 12px | 400 | Secondary body |
| 11.5px | mono | Metadata |
| 11px | uppercase, .08em | Section labels |
| 10.5px | — | Chips, keyboard hints |

Terminal text 12–12.5px. Never below 10.5px anywhere, and 10.5px only for chips,
event tags and keyboard hints.

Monospace for commands, output, paths, branches, ids, durations, costs, ports, and
event tags. Add `font-variant-numeric: tabular-nums` to any number that updates in
place.

## Geometry

4px base unit; padding and gaps are multiples — 4, 6, 8, 9, 11, 13, 16, 18, 22.

Controls are 22 / 24 / 26 / 28px tall. Chrome: top bar **42**, tab strip **29**,
panel header **~38**, status rail **25**.

Radius: 4 (chips) / 5 (buttons, inputs) / 6–8 (panels, overlays) / 12 (system
notifications only).

Borders are 1px hairlines — `#1B1D1C` inside lists, `#232624` between regions,
`#323634` on overlays and inputs. Cards, thick borders and big rounded containers
are forbidden: separate with a hairline and space.

**One shadow exists in the entire product:** `0 22px 56px rgba(0,0,0,0.55)` on
overlays. Nothing else casts one.

## Components

**Button** — h26/h28, radius 5, 12.5px. Primary is filled `#68AEA5` with
`#141514` text at 600 weight, **one per view**. Secondary is 1px `#323634`,
transparent, hover `#232624`. Danger is 1px `#D77D79` outlined, **never filled** —
filled red reads as a state, not a choice. Ghost has no border. Every button:
`white-space:nowrap; flex:0 0 auto`. Disabled controls explain themselves in the
label or a sibling line — never a silent grey button.

**Status chip** — h19–24, 1px border in the state colour, same colour text,
transparent fill, 10.5px.

**Dot** — 6–7px circle in the state colour. Success dots at 0.55 opacity.

**Event tag** — 10px uppercase monospace, letter-spacing .04em, coloured by event
class. Machine vocabulary, so it looks like it.

**Block** (command + output) — no card. Row padding `9px 18px 11px 15px`,
transparent 2px left border that becomes `#232624` on hover with background
`#181A19`. Status dot, then command in 12.5px mono, then right-aligned tabular
metadata: exit code · duration · time, in that order. Output indented 16px in
`--ink-2`. Optional guided-mode explanation as a 12px muted line behind a 2px
`#323634` left border. Raw output is always reachable.

**List row** — 11px 13px padding, hairline bottom border, 2px left border
transparent → `#68AEA5` when selected with `#1B1D1C` background. Max two lines,
both truncating.

**Tabs** — 29px tall, 1px teal bottom border when active, session dot, close ×
that stops propagation, draggable to reorder and to move between panes.

**Segmented control** — 26px pills, active is `#232624` fill + 600 weight.

**Toggle** — 30×17 track, 13px knob, teal when on, `#2A2E2C` when off.

**Input / composer** — 1px `#323634`, radius 6, background `#141514`, 9px 11px
padding, placeholder `#5d635f`.

**Diff** — 10% tint rows, never solid blocks. Sign column 20px, line number 38px
right-aligned in `#5d635f`. Side-by-side above ~900px of pane width, unified below.

**Resize handle** — 5px, `#232624`, `cursor: col-resize`/`row-resize`, `#68AEA5`
on hover and while dragging.

**Overlay** (palette, modal) — `#171918`, 1px `#323634`, radius 8, the one allowed
shadow. Escape always closes; the first row is preselected.

**System notification** — macOS geometry: radius 12, `#22262A` surface, `#3A4046`
borders, system greys, up to three inline actions. It is the OS speaking, so it
does not use Tervin's palette.

## Layout

- **Two columns maximum** at any time. A third pane means something collapses.
- Agent UI never exceeds ~30% of the window by default.
- **Collapse, never squeeze.** Below a threshold, hide a pane or fold controls into
  a `⋯` menu. Never shrink buttons until labels wrap or clip.
- Every flex child holding text: `min-width:0` plus
  `overflow:hidden; text-overflow:ellipsis; white-space:nowrap`.
- Bars that can overflow: `flex-wrap:wrap` or an overflow menu.
- Panes, drawers and bottom panels are user-resizable by dragging their divider,
  and those sizes persist.
- Nothing is fixed-position over terminal output.
- Under ~860px the supporting column hides entirely and returns via a button — no
  two-column compromise.

## Interaction

Keyboard first:

| Key | Action |
|---|---|
| `⌘K` | Command palette |
| `⌘B` | Thread drawer |
| `⌘⇧P` | Agent profile |
| `⌘D` | Split |
| `⌘⇧A` | Approve |
| `⇧⇥` | Cycle agent mode |
| `⌘.` | Stop a thread |

Every action is in the palette; mouse support is additive.

Motion is 150–220ms and only for orientation: pane open/close, drawer, state
change. No looping or ornamental animation. Respect `prefers-reduced-motion`.

Show real capability per agent runtime: absent or explicitly disabled controls,
never fake parity. **"Unknown" is a valid state** — show it in muted grey instead
of guessing.

## Copy

Precise, candid, calm, technical.

| Say | Not |
|---|---|
| Agent is waiting for approval | AI needs your attention! |
| Tervin cannot roll this back | Safely sandboxed |
| Review 3 changed files | Magical code review |
| State unknown for this runtime | Agent is thinking… |
| Writes schema to a non-local database | Performs some database work |
| 23 passed, 1 failed | Almost there! |

Never: magical, revolutionary, supercharge, all-in-one, unlock, seamless,
AI-powered. Never imply certainty an agent does not have — show plan, command,
files, diff, test result, evidence.

In Guided mode add exactly one plain-English sentence under a command — no jargon,
no second sentence.

## Instant rejects

Gradients · Glassmorphism · Purple-blue AI styling · Glowing orbs · Emoji · Big
rounded cards · Icons in coloured circles · Chat bubbles · Dashboard card clutter
· Looping animation · Colour as decoration · Fake feature parity · Silent disabled
controls · Body text below 12px

## Fonts in the desktop app

The mockups load Geist and JetBrains Mono from Google Fonts. The shipped app
**bundles them locally instead**: the Tauri CSP allows `font-src 'self' data:`
only, and a desktop tool must not depend on a network round-trip to render its
first frame. See [`ui/src/design/fonts.css`](../ui/src/design/fonts.css).
