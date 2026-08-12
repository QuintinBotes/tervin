# Spec 22 — Repair the token baseline

**Runs second, immediately after hardening.** Extracted out of spec 20.3 and moved to the
front, for a reason given below.

## Context

Extracting the Paper Chrome reference build turned up four defects in what ships today.
Two of them are not design questions — they are bugs, and every UI slice between here and
spec 19 would be built and visually judged against them.

**Seven CSS custom properties are referenced in production and defined nowhere.**

| Undefined | Call sites |
|---|---|
| `--radius-lg` | `App.tsx:468`, `SavedCommands.tsx:282`, `CommandPalette.tsx:266`, `ApprovalSheet.tsx:80`, `SettingsPanel.tsx:62`, `CommandHistory.tsx:103`, `DirectoryJump.tsx:138` |
| `--radius-md` | `SavedCommands.tsx:357`, `SettingsPanel.tsx:125` |
| `--radius-sm` | `ApprovalSheet.tsx:159`, `GitPanel.tsx:51`, `SettingsPanel.tsx:195,500,828` |
| `--text-title` | `CommandPalette.tsx:296`, `ApprovalSheet.tsx:94`, `SettingsPanel.tsx:906` |
| `--text-heading` | `SettingsPanel.tsx:860` |
| `--tervin-font-mono` | `TerminalPane.tsx:688` |

`base.css:54-57` defines `--radius-chip/control/panel/system`; `base.css:30-39` defines
`--text-page` through `--text-tag`. None of the seven is among them. An undefined custom
property resolves to nothing, so **every overlay in the app currently renders with radius
0**, every dialog title at inherited size, and the sticky command header in generic
`monospace`.

**No font has ever loaded.** `docs/DESIGN.md:206-210` states the app "bundles them locally
instead" and points at `ui/src/design/fonts.css`. That file does not exist, and
`@font-face` appears nowhere in the repository outside `docs/reference`. `base.css:68`
declares `--font-ui: "Geist", …` and Geist has never been present, so every frame the app
has ever drawn used a system fallback.

## Why this runs second rather than at spec 20

Thirteen specs of interface work sit between hardening and the Paper Chrome pass. Each
will be looked at, judged, and adjusted. Doing that against a baseline where overlays are
square by accident and the declared typeface is absent means every one of those judgements
is made against the wrong picture — and when the tokens are finally repaired, all of it
needs re-judging.

It is also, separately, the cheapest possible way to make the app look considerably
better: the corners were designed, they simply never resolved.

The Paper Chrome values are used as the targets, so this is not throwaway work — it is the
first slice of spec 20, brought forward.

## Slices

### 22.1 — Define the seven, at their Paper Chrome values
Radius: overlays 10, panels and the well 8, inner cards 6, controls 5. Type:
`--text-title` to the surface-title role, 16px/600/-0.01em; `--text-heading` to the same
scale one step down. `--tervin-font-mono` to the mono stack the appearance setting already
resolves.

Do not rename the call sites yet — spec 20 owns the vocabulary change. This slice makes
the existing names resolve.

*Exit:* every overlay has visible corners. A screenshot before and after goes in the PR.

### 22.2 — A test that fails on the next undefined token
The reason seven accumulated is that nothing checks. Walk every `var(--…)` reference in
`ui/src` and assert the name is defined in `base.css` or written by `applyTheme`.

This is the same shape as the readership-matrix fixture the project already uses — a check
that makes drift a failing test rather than a discovery. It fails today, passes after 22.1,
and keeps passing.

*Exit:* the test exists, and deleting a definition turns it red.

### 22.3 — Ship the fonts, honestly
Bundle IBM Plex Sans and IBM Plex Mono as local `woff2` subsets with a real
`ui/src/design/fonts.css`, under the existing `font-src 'self'` CSP — the reason
`DESIGN.md` gives for bundling is sound even though the bundling never happened: a desktop
tool must not need a network round-trip to render its first frame.

Going straight to Plex rather than adding Geist first: spec 20 replaces the interface face
anyway, and installing a typeface twice to honour a document that was already wrong is
work for nobody.

**Budget for the shift.** The app has never rendered in its declared face, so this moves
measurements everywhere. That is not a regression to fix — it is the layout being seen for
the first time. Re-check dense rows and the status rail after.

*Exit:* the first frame renders in Plex with the network unavailable, verified rather than
asserted. A fallback chain is declared and honest.

### 22.4 — Correct the claim in DESIGN.md
`docs/DESIGN.md:205-210` currently describes bundling that does not exist. Fix the sentence
in the same change that makes it true.

Do not carry it forward with the typeface names swapped — that would reproduce the original
error with new nouns, and the whole point of this project's documentation standard is that
a claim is made after the thing works, not before.

*Exit:* the paragraph describes what the repository contains.

### 22.5 — Raise the 10px tag
`docs/DESIGN.md` sets a 10.5px floor at line 65 and specifies a 10px event tag at line 103.
`base.css:39` ships `--text-tag: 10px`, used at `App.tsx:714,721,727` and
`PaneTree.tsx:158`. The design system lost an argument with itself and the implementation
quietly took the losing side.

Paper Chrome settles it at 10.5. Raise the value, fix the four call sites, and remove the
contradiction from `DESIGN.md` line 103.

*Exit:* no text in the interface renders below 10.5px. `--text-tag` either carries 10.5 or
is gone.

## Explicitly not in this spec

The well inversion — `themes.ts:63-64` makes `terminalBg` equal to `panel`, a step
*lighter* than the app background, so today's well is the lightest surface rather than the
darkest, exactly inverting Paper Chrome's central idea. That is a theme-model change and it
belongs with the `ChromeTheme` / `TerminalPalette` split in spec 20.4, not here.

## Verification

```sh
pnpm exec vitest run && pnpm exec tsc --noEmit
cargo test --workspace
```

Manual: open the palette, the approval sheet, Settings and the directory jump. All four
have corners. Disconnect the network, relaunch, and confirm the first frame is Plex.
