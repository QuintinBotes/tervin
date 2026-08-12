# Spec 05 — Terminal profiles & appearance parity

`COMPETITIVE-SPEC.md` §3.6 (profiles), plus the Warp appearance surface.

## Context

Tervin has *agent* profiles (`crates/agent-runtime/src/profile.rs`, TOML, multi-account,
with env scrubbing). It does not have *terminal* profiles — named terminal
configurations, which iTerm2 and Windows Terminal both centre their UX on and which Warp
calls launch configurations.

The appearance system is already strong: 15 themes tagged light/dark, live-applied as CSS
variables and pushed into running xterm instances, plus DEC 2031 colour-scheme change
notification and `CSI ? 996 n` query replies — rarer than most mature emulators. The gaps
are the ordinary ones around it.

`DESIGN.md` constrains this spec harder than most: fixed palette, one shadow in the
entire product, no gradients, no glassmorphism, no decoration. Opacity and blur are
Warp features that sit close to that line; implement them plainly, defaulted off.

## Slices

### 05.1 — Terminal profiles
A named set of: shell/program, arguments, starting directory, environment additions,
theme override, font override. Distinct from agent profiles and named so in the UI —
`spec_file.md` uses "profile" for both, which will confuse if left alone.

Store as TOML alongside `agents.toml`, for the same reason: a file a user can read, diff
and commit.

*Exit:* a profile can be created, launched into a new pane, and set as the default for
new panes. A parse failure is reported, not swallowed (mirror `profile.rs:797`).

### 05.2 — Launch configurations
Warp's term for a saved window/tab/pane arrangement with a profile per pane and an
optional command per pane. Tervin already serialises the pane tree
(`ui/src/lib/panes.ts:264,275`) for session restore, so this is that structure plus
intent.

Overlaps §3.10 (layout files as artefacts) deliberately — a launch configuration that can
be exported to `.tervin/layout.toml` is the same feature read two ways. Build the model
once; spec 06 adds the committed-file half.

*Exit:* a three-pane arrangement can be saved, named, and launched fresh.

### 05.3 — Sync with OS light/dark
Tervin reports colour-scheme changes to programs in a pane (DEC 2031) but does not
*follow* the OS itself. Add a "sync with system" option that picks a light theme and a
dark theme and switches between them.

*Exit:* toggling macOS appearance switches the theme and re-emits the DEC 2031
notification to running programs.

### 05.4 — Opacity, blur, pane dimming
Window opacity and background blur, both defaulted off. Inactive-pane dimming, which is
genuinely useful at four panes and is the one of the three that earns its place.

`DESIGN.md`: no glassmorphism. Blur here means a window background, not a frosted card.
If it cannot be done without violating that, dimming alone is enough — say so rather than
shipping something the design system rejects.

*Exit:* dimming works and respects `prefers-reduced-motion`. Opacity persists.

### 05.5 — Custom theme creation
Warp generates a theme from a background image. The useful, non-gimmick half is a theme
editor: take an existing theme, adjust the 16 ANSI colours and the UI tokens, save it,
export it as a file. `ui/src/design/themes.ts` is already the right shape.

Include a contrast check — `spec_file.md` requires a high-contrast mode and accessibility
semantics, and a user-authored theme is the most likely place legibility breaks.

*Exit:* a custom theme can be created, applied, exported and re-imported. A theme failing
contrast is flagged, not blocked.

### 05.6 — Global hotkey
A system-wide chord that summons the Tervin window. Standard in Warp, iTerm2 and Ghostty.
Off by default; a global hotkey that steals a chord silently is a bad first-run
experience.

*Exit:* the hotkey summons and hides the window. Unset by default.

## Verification

```sh
pnpm exec vitest run && pnpm exec tsc --noEmit
cargo test --workspace
```

Manual: `docs/MANUAL-TEST.md` §4.7 — all themes legible, theme switch while a TUI runs,
tab strip on all four sides — plus the new profile and launch-configuration paths.
