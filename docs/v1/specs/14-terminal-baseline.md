# Spec 14 — Terminal baseline completion

Everything `spec_file.md` "Required baseline" and "Sessions and connections" specify that
is not built. These are the items the original spec listed as table stakes.

## Context

`spec_file.md:338-358` enumerates what Tervin *must* support. Most of it is built and
several parts are better than the spec asked for. This spec is the remainder, checked
line by line against the code.

`spec_file.md:377-393` does the same for sessions and connections. Two items there are
refused rather than missing and stay refused: a latency number SSH cannot produce, and
becoming a credential vault. Both refusals are already written down; do not re-open them.

## Slices

### 14.1 — Shells: nushell and PowerShell
The baseline names "zsh, bash, fish, PowerShell, nushell, custom commands". Shell
integration ships hooks for zsh, bash, fish and pwsh
(`crates/shell-integration/assets/tervin.{zsh,bash,fish,ps1}`) — so pwsh has a hook but
needs exercising, and **nushell has none**.

nushell's config model differs enough that a hook may not be expressible. If it is not,
declare it `Partial` with a note saying Blocks fall back to heuristic prompt detection —
the mockup's own copy already anticipates this case: *"Without integration — Tervin still
works; Blocks fall back to heuristic prompt detection."*

*Exit:* nushell either emits OSC 133 or its capability says why not. pwsh is tested.

### 14.2 — Pane actions: detach and duplicate
The baseline names "resize, swap, zoom, duplicate, close, and detach pane actions".
`ui/src/lib/panes.ts` has split, close, swap, resize, zoom and even-sizes. **Duplicate**
(a new pane in the same directory, same profile) and **detach** (move a pane to its own
tab or window) are absent.

Detach-to-window depends on multi-window support, which `capabilities/default.json` does
not currently permit. Detach-to-tab does not, and is the useful half.

*Exit:* duplicate opens a pane in the same cwd. Detach moves a pane to a new tab and the
tree normalises correctly (`panes.ts:225`).

### 14.3 — Config reload
The baseline names "config reload". Ghostty's live-reloading flat `key = value` config is
called out in `COMPETITIVE-SPEC.md` §2 as something Tervin lacks.

Tervin's settings live in SQLite with a live-previewing UI, which is arguably better for
appearance. What it lacks is the file-based half: `agents.toml` and `mcp.json` are read at
startup and not watched. Edit either and nothing happens until relaunch.

Watch both, reload on change, report a parse failure rather than silently keeping the old
value — the existing pattern at `profile.rs:797`.

*Exit:* editing `agents.toml` while running updates the profile list, and a malformed
edit reports without discarding the working configuration.

### 14.4 — Connection profiles, host labels, reconnection states
`spec_file.md:379-391` names all three. `ConnectionsPanel.tsx` lists hosts parsed from
`~/.ssh/config` with key status and `ssh -O check` control-master state. What is missing
is Tervin's own layer on top: a named profile (host + user + directory + shell + terminal
profile), a human label distinct from the SSH alias, and an explicit reconnecting state.

Stays refused: a latency number. `README.md` and §5 both record why — SSH exposes no
round-trip time, so a number here would be a measurement of something else wearing a
latency label. The panel already says "N ms to connect", never "latency". Keep that.

*Exit:* a named connection profile launches a pane. Reconnection shows a state, not a
frozen pane.

### 14.5 — Selection expansion and smart-selection completeness
The baseline names "selection expansion" and smart selection for "paths, URLs, ports,
issue IDs, commits, emails, and stack traces". `ui/src/lib/links.ts:21-30` covers all
seven kinds, which is complete — but `expandSelection()` at `links.ts:274` is written,
unit-tested, and never imported, so selection expansion does not exist in the product.

Overlaps spec 01.8. Whichever lands first does it; the other verifies.

*Exit:* double-click expands by word, triple by line, and a modifier expands by
smart-selection kind.

### 14.6 — Multi-line paste safety, verified
The baseline names it and it is built: `pasteNeedsConfirmation` at `links.ts:303` fires
when the application has not enabled bracketed paste. Verify the awkward cases — a paste
containing a newline mid-command, a paste into a program that enabled bracketed paste
after the fact, and a paste while the input gate is still closed (`pty.rs:57-66`).

*Exit:* a PTY test per case.

## Verification

```sh
cargo test --workspace
pnpm exec vitest run
```

Manual: `docs/MANUAL-TEST.md` §4.1 and §4.6.
