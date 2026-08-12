# Spec 01 — Terminal hygiene

The classic-terminal affordances that are conspicuously absent. None of these is hard;
all of them are the first thing an experienced user notices.

## Context

An audit of the terminal surface against iTerm2/Warp/Ghostty found nine real gaps. Three
of them are things a user reaches for within minutes — the bell, closing a tab, and
moving between blocks — and one of those three is worse than missing: `block.prev` and
`block.next` are bound (`ui/src/lib/keymap.ts:126-127`) and implemented as *scroll ten
lines* (`ui/src/App.tsx:1251-1256`), despite exact OSC 133 marker positions being
available at `ui/src/components/TerminalPane.tsx:333`. That is a control that lies.

## Slices

### 01.1 — The bell
Entirely absent: no `\x07` handling, no `onBell`, no visual flash, no audio, no tab
attention marker. Grep confirms `BEL` appears only as an OSC terminator in
`crates/terminal-core/src/osc.rs:25`.

Implement: xterm's `onBell` → a visual bell (a brief border pulse, respecting
`prefers-reduced-motion` per `DESIGN.md`), an optional audible bell off by default, and
an attention dot on the tab when the bell fires in an unfocused pane. Settings entries
alongside the existing cursor and copy-on-select controls in
`ui/src/components/SettingsPanel.tsx`.

*Exit:* `printf '\a'` in an unfocused pane marks its tab. A PTY test asserts the byte
reaches the frontend path.

### 01.2 — Tab close
There is no `closeTab` anywhere in `ui/src`. A tab disappears only when its last pane
closes, via `.filter((tab) => tab.root !== null)` at `ui/src/lib/store.ts:686-688`.

Add an explicit close: a `✕` in the tab strip (`ui/src/App.tsx:927-976`), `⌘W` bound in
`keymap.ts`, and confirmation when the tab holds a pane with a running foreground
process — reuse the same honesty the session-restore banner already applies.

*Exit:* `⌘W` closes the focused tab; closing the last tab leaves a usable window.

### 01.3 — Tab rename and auto-title
Titles are frozen at creation (`title: "Shell"`, `store.ts:716`). OSC 0/2 titles *are*
parsed into `ShellSignal::Title` (`crates/terminal-core/src/signals.rs:40-41`) and then
discarded by the only consumer at `crates/block-engine/src/builder.rs:354`.

Route `ShellSignal::Title` through to the pane, let it title its tab, and allow a manual
rename (double-click) that pins the title against further OSC updates.

*Exit:* `printf '\033]0;build\007'` retitles the tab. A manual rename survives the next
OSC 0.

### 01.4 — Tab reorder
No drag handlers exist in the tab strip. Add drag-to-reorder plus `⌘⌥←/→` bindings.
The tab strip renders on all four sides, so the drag axis follows `tabBarPosition`.

*Exit:* reorder works on a top and a left strip; order survives session restore.

### 01.5 — Real block navigation
Replace the scroll-ten-lines implementation with navigation over the OSC 133 markers
already registered as xterm markers (`TerminalPane.tsx:333-337`, kept as xterm markers
specifically so they survive reflow). `⌘↑`/`⌘↓` move to the previous/next command start
and scroll it into view; the sticky header (`ui/src/lib/sticky.ts`) should follow.

If a pane has no shell integration there are no markers — say so once rather than
falling back to a scroll that pretends to be navigation.

*Exit:* in a pane with shell integration, `⌘↑` lands on the previous prompt. In one
without, it reports why it cannot.

### 01.6 — Background blocks
Warp's term for a command that keeps producing output after you move on. Tervin has the
Block model to express this already — a Block whose `command.completed` has not arrived.
Surface long-running Blocks in the Deck and mark the tab.

*Exit:* `sleep 30 && echo done` in a background tab shows as running and marks the tab
on completion.

### 01.7 — OSC 52, with a decision path
Today OSC 52 writes are parsed (`signals.rs:291-307`), emitted as `clipboard://requested`
(`crates/tervin-app/src/commands.rs:406-412`), and shown as a notice saying Tervin "did
not allow it automatically" (`ui/src/App.tsx:169-173`) — with **no way to allow it at
all**. That is a dead-end control, which `DESIGN.md` lists as an instant reject.

Add an approve/deny affordance on the notice, a per-host memory of the answer, and a
setting for the default. Reads stay unanswered permanently (`signals.rs:560`) — that is
correct and should be stated rather than silently true.

*Exit:* a `printf` OSC 52 write can be approved and lands in the clipboard. Denying it
leaves the clipboard untouched. A read request is refused with a reason.

### 01.8 — Wire or delete the dead code
Four things are built and unreachable — the repository's most recurring bug class,
documented in `HANDOFF.md` as "Built in Rust, unreachable from the UI".

- `expandSelection()` — `ui/src/lib/links.ts:274`, written and unit-tested, never
  imported. `TerminalPane.tsx:15-17` claims smart selection that is actually xterm's.
- `@xterm/addon-web-links` — a dependency (`package.json:22`), never imported. The
  custom link provider supersedes it; remove the dependency.
- `PrivateMode::MouseReporting` — tracked at `osc.rs:87-94` with zero readers. Its
  stated purpose (routing a click to the program rather than starting a selection) is
  not implemented.
- `terminal.copy` / `terminal.paste` — bound in `keymap.ts:117-118` with no case in
  `runAction` (`App.tsx:1245-1260`), falling through to webview defaults.
- Ligatures — `fontLigatures` is passed to the constructor (`TerminalPane.tsx:255`) and
  exposed in Settings, but `@xterm/addon-ligatures` is not a dependency and
  `applyAppearance` does not update it. The setting is inert.

Each: wire it or delete it. A setting that does nothing is worse than an absent one.

*Exit:* no exported symbol in `ui/src/lib` is unreferenced; the ligatures toggle either
changes rendering or is gone.

## Verification

```sh
pnpm exec vitest run && pnpm exec tsc --noEmit
cargo test --workspace
```

Manual: `docs/MANUAL-TEST.md` §4.1 and §4.7, plus the bell, tab close/rename/reorder,
and `⌘↑`/`⌘↓` in a pane with and without shell integration.
