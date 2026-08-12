# Spec 02 — Keybinding customisation

## Context

`ui/src/lib/keymap.ts` is a complete, well-designed, data-driven keymap: context
scoping (`global | terminal | overlay | composer`), conflict detection at
`keymap.ts:262`, invalid-chord reporting at `:214`, macOS chord formatting at `:283`.
Its header comment at lines 5-6 says bindings are "editable in settings, and persisted".

They are not. `ui/src/App.tsx:212` constructs `new Keymap()` with no argument, so
`DEFAULT_BINDINGS` always wins. There is no editor in `SettingsPanel.tsx` and nothing is
persisted. The design is done; the wiring is missing.

This is the same pattern `HANDOFF.md` records six instances of in a single day. Fixing
it is mostly plumbing, and the header comment is currently a claim Tervin does not have.

## Slices

### 02.1 — Persist and load a binding set
Store user bindings in the existing `kv` table via `settings_get`/`settings_set`
(`crates/tervin-app/src/commands.rs:2009,2015`) alongside `APPEARANCE_KEY`. Load them at
startup and pass them to `new Keymap(bindings)`.

Store only *overrides*, not the whole table — a full snapshot means a user who
customised one key stops receiving every new default binding thereafter.

*Exit:* a stored override survives a relaunch. A binding added to `DEFAULT_BINDINGS`
appears for a user who had customised something else.

### 02.2 — The editor UI
A section in `ui/src/components/SettingsPanel.tsx` listing every action with its current
chord, grouped by the existing context scopes. Recording a chord captures the next
keypress. Conflicts use `keymap.ts:262`, which already detects them — show the
conflicting action rather than silently refusing.

Per `DESIGN.md`: no silent disabled controls. A binding that cannot be taken (because
the terminal needs it) says which and why.

*Exit:* rebinding `⌘K` to something else moves the palette; the conflict path is
exercised by a test.

### 02.3 — Reset, per-action and wholesale
A reset on each row and one for the whole table. Resetting removes the override rather
than writing the default, so 02.1's forward-compatibility holds.

*Exit:* reset returns the row to `DEFAULT_BINDINGS` and removes the stored key.

### 02.4 — The keybinding hint bar
`COMPETITIVE-SPEC.md` §3.6 (P3), from zellij. A thin, dismissible bar showing the
bindings valid in the current context — which the keymap's context scoping already knows.
Off by default; `DESIGN.md` caps fixed chrome and forbids anything fixed over terminal
output, so this belongs in the status rail, not floating.

*Exit:* the bar's contents change when an overlay opens, matching the active scope.

### 02.5 — Reference sheet
`spec_file.md` lists "Open keybinding reference" among the essential keyboard actions.
An overlay listing every binding by context, searchable, reachable from the palette.

*Exit:* the palette can open it; every action in `DEFAULT_BINDINGS` appears.

## Constraint that governs this whole spec

`ARCHITECTURE.md:225-228`: **unhandled keys reach the terminal.** `runAction` returns a
bool and only a handled action calls `preventDefault`. This is what keeps `vim` and
`emacs` usable. A user must not be able to bind something that swallows a key the
terminal needs without being told what they are taking.

A test should assert that after applying a custom binding set, `Ctrl-A`, `Ctrl-E`,
`Ctrl-K`, `Ctrl-W`, `Escape` and `Tab` still reach a PTY unless explicitly rebound.

## Verification

```sh
pnpm exec vitest run
pnpm exec tsc --noEmit
```

Manual: rebind a key, relaunch, confirm it held; open `vim` and confirm `Ctrl-W` still
works.
