# Spec 08 — Input & scrollback parity

`COMPETITIVE-SPEC.md` §3.6, P3, plus Warp's synchronised inputs.

## Context

A collection of small, individually-cheap items from iTerm2, kitty, zellij, Alacritty and
Warp. None changes what Tervin is; together they are most of what an experienced user
means by "it feels finished".

`ARCHITECTURE.md:225-228` governs the whole spec: unhandled keys reach the terminal.
Every item here adds a key path, and each must return `false` from `runAction` when it did
not act.

## Slices

### 08.1 — Vi-mode scrollback with regex hints
From Alacritty, which `COMPETITIVE-SPEC.md` §2 singles out as worth taking. A mode where
`hjkl`, `/`, `n`, `N`, `v` and `y` navigate and select scrollback without a mouse.

xterm.js has no vi mode, so this is Tervin's: a cursor overlay over the buffer, driven by
the keymap in a new `scrollback` context. `SearchAddon` already handles regex, case
sensitivity and whole-word, so `/` reuses it.

*Exit:* `⌘⇧Space` (or a chosen chord) enters the mode, `/error` jumps, `v`+`y` copies.
`Escape` leaves and the terminal takes the keyboard back.

### 08.2 — Broadcast input
From kitty and zellij. Type once, send to every pane in the tab — or a selected subset.
Warp calls it synchronised inputs.

This writes to multiple PTYs at once, which is exactly the kind of thing that deserves a
visible, unmistakable indicator while active. A user who forgets it is on will run a
command in a production SSH session they meant for a local shell.

*Exit:* broadcast to three panes writes to all three. The active state is visible in
every affected pane, not only in the one with focus.

### 08.3 — Multi-cursor in the composer
Warp parity item, §3.6. The composer is `ui/src/lib/editing.ts` (503 lines) with native,
emacs and vim modes already. Add multi-cursor to the native mode.

`editing.ts:25` has a section titled "What is deliberately not implemented" — if
multi-cursor conflicts with something already refused there, honour the earlier decision
and record why rather than overriding it.

*Exit:* `⌥`-click adds a cursor; typing edits at each.

### 08.4 — Paste history
From iTerm2. The last N clipboard entries, reachable from the palette, inserted rather
than run — matching `⌘J`, `⌘R` and `⌘⇧S`, where `docs/MANUAL-TEST.md` §4.5 requires
"Enter fills the pane and does not run."

Bounded and forgettable: paste history holds whatever was copied, which routinely
includes credentials. It lives in memory, not in SQLite, is capped, and is cleared on
quit. Say so in the setting.

*Exit:* the last paste is retrievable; nothing is written to the database; quitting
clears it.

### 08.5 — Annotations on scrollback
From iTerm2. Attach a note to a region of output. A Block already carries `notes`
(`crates/block-engine/src/model.rs:178-201`), so an annotation on a region inside a Block
has a home; one outside a Block needs a pane-scoped anchor.

*Exit:* an annotation survives a relaunch and is findable in History.

### 08.6 — Scroll behaviour controls
The audit found nothing owning this — no scroll-on-output, no scroll-lock,
no scroll-to-bottom-on-input. It is entirely xterm's defaults today, which is fine until
someone is reading scrollback while a build streams.

Add: scroll-to-bottom on input (on by default), lock-while-scrolled-up with a "jump to
bottom" affordance showing how far behind you are.

*Exit:* scrolling up during `yes | head -c 20000000` holds position; typing returns to
the bottom.

## Verification

```sh
pnpm exec vitest run && pnpm exec tsc --noEmit
cargo test --workspace
```

Manual: enter vi mode and confirm `vim` still receives `hjkl` when the mode is off;
broadcast to three panes; scroll up during heavy output.
