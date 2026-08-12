# Spec 18 — macOS native integration

Designed in full in `docs/reference/tervin-workspace-v2.dc.html` and **built almost not at
all**. The mockup's Settings has a dedicated "macOS" tab with four groups and nineteen
rows; the app has `tauri-plugin-opener` and `tauri-plugin-dialog` and nothing else.

## Context

The mockup's own framing: *"Tervin behaves like a native Mac app: system notifications you
can act on, a Dock badge for work that needs you, and credentials in the Keychain."*

This is the largest designed-and-unbuilt surface in the project. It matters more than it
looks, because the product's central object — an agent waiting for a permission decision —
is exactly the thing a desktop notification should be able to answer without you switching
apps.

## Slices

### 18.1 — Notification Center, with actions
Four rows on by default in the mockup, one off:

- **Agent needs approval** — *"Banner with Approve / Deny / Open thread — actionable
  without focusing Tervin."* This is the important one.
- **Thread finished or failed** — grouped per thread, summary in the body.
- **Long command completed** — only when Tervin is in the background and the command ran
  over 30s. The 30s threshold is a constant and carries its reasoning.
- **Time Sensitive for permissions** — permission requests break through Focus; everything
  else stays quiet.
- **Focus filter** (off by default) — Tervin registers a Focus filter so Do Not Disturb
  holds non-urgent alerts.

Needs `tauri-plugin-notification`, which is not currently a dependency, plus native action
handling.

**The honesty constraint bites here.** An approval answered from a notification must be
the *same* decision path as the in-app sheet — same Rules classification, same audit
entry, same `enforceable` flag. A notification that approves something the gate could not
have stopped must say "observed", exactly as the sheet does.

Also: OSC 777 notifications from programs are already parsed and shown in-app only,
deliberately (`ui/src/App.tsx:150-157`) — *a process asking for one is not the same as the
person wanting one*. Keep that distinction. A program's notification is not promoted to
the OS just because Tervin now can.

*Exit:* approving from a banner denies or allows identically to the sheet, with the same
audit record. A program's OSC 777 stays in-app.

### 18.2 — Dock badge, Dock progress, Dock menu
- **Badge** — the number of agents waiting on you. `ThreadState::needs_user()` already
  computes this.
- **Progress** — a progress bar during builds, tests and long agent runs. Only where
  progress is actually reported; a runtime that reports none gets no bar.
- **Dock menu** — recent projects, new tab, new agent profile.

*Exit:* the badge matches the Deck's waiting count. No fake progress.

### 18.3 — Menu bar extra
*"Compact Deck — approve or stop a thread without switching apps."* A small always-
available surface showing what is waiting and letting you answer it.

Same constraint as 18.1: the decision path is the real one, not a shortcut around Rules.

*Exit:* a permission can be approved and a Thread stopped from the menu bar, with the same
audit trail.

### 18.4 — Keychain, Touch ID, Secure Input
The three security rows, and the reasoning for each is decided (see spec 00.4):

- **Keychain** — any secret the user hands Tervin goes to the macOS Keychain with only a
  reference on disk. **SSH passphrases stay refused**: `ssh-agent` with
  `--apple-use-keychain` already owns that and duplicating it makes Tervin a new target.
  The mockup's row overreaches; correct the row, do not build to it.
- **Touch ID for high-risk approvals** — require biometric confirmation for `sudo`,
  production targets and destructive SQL. `rules-engine` already classifies risk, so this
  is a gate on an existing signal. It genuinely strengthens the permission model.
  Off by default; a biometric prompt the user did not ask for is hostile.
- **Secure Input awareness** — *"Tervin pauses scrollback capture while a password field
  is active."* This is a real defect being closed, not a feature: today a password typed
  at a `sudo` prompt could land in persisted scrollback. macOS exposes secure input state;
  use it, and pause both capture and Block output.

*Exit:* a `sudo` password does not appear in the database. A test asserts it. Touch ID
gates a high-risk action when enabled and is absent when not.

### 18.5 — System conventions
- **Follow system appearance** — overlaps spec 05.3; whichever lands first does it.
- **Native tabs, Split View, Stage Manager** — standard AppKit window behaviour.
- **Services and Finder** — *"Open in Tervin"* in the Finder context menu and Services
  menu.
- **Shortcuts.app and a `tervin://` URL scheme** — run saved workflows from Shortcuts;
  `tervin://` links open a project or thread.
- **Spotlight and Quick Look** (off by default in the mockup) — exported Blocks and diffs
  indexed and Quick Look-able.

The URL scheme is the one with a security dimension: a `tervin://` link is untrusted input
from anywhere, including a web page. It may navigate; it must not run anything. State that
in `SECURITY.md`.

*Exit:* `tervin://` opens a project and cannot execute. "Open in Tervin" appears in Finder.

### 18.6 — VoiceOver and Reduce Motion
The mockup's last row: *"Full accessibility semantics; motion respects system settings."*
`spec_file.md:356-358` requires accessibility and screen-reader semantics and reduced-
motion support in the baseline.

`screenReaderMode` exists as an appearance setting today, which is xterm's accessibility
buffer — necessary and not sufficient. The workspace chrome around it needs roles, labels,
focus order and live regions.

Deliberately shared with spec 20, which owns the wider accessibility pass. This slice
covers the macOS-specific half: VoiceOver rotor behaviour and the system reduced-motion
setting.

*Exit:* VoiceOver can reach and announce every control. `prefers-reduced-motion` removes
every transition.

## Note on the mockup

The mockup's mode cycle is `['plan','ask','auto','bypass']`. **`bypass` is deliberately
absent from the product** — `README.md` and `CONTRIBUTING.md` both record that a one-click
way to disable every check cannot be reconciled with telling users their actions are
reviewable. The code is right and the mockup is stale. Do not build it.

## Verification

```sh
cargo test --workspace
pnpm exec vitest run
```

Manual: type a `sudo` password and confirm nothing lands in scrollback or a Block.
Background the app, trigger a permission request, and answer it from the banner.
