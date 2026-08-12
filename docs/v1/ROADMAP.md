# Tervin v1.0 — the finishing roadmap

Working document. Not committed; lives in the session scratchpad by choice.

**Scope:** everything in `spec_file.md`, every gap closed, Warp terminal parity, the
detach/reattach daemon, Linux, and a UI/UX pass at the end. Windows deferred. Hardening
audited first and re-swept last. Specs execute under a swarm that auto-advances.

---

## What "v1.0" means here

`spec_file.md`'s ten acceptance criteria are the gate. Nine are met or nearly so. **The
one that is not is criterion 4** — *"Multiple heterogeneous agent tasks can run in
parallel while users understand their purpose, state, current action, permissions, and
output."* Tervin speaks ACP to one agent at a time.

Add to that the tmux-class expectation that a build survives a closed laptop, and those
two are what make this a 1.0 rather than a 0.5.

A v1.0 ships when:

1. Every classic terminal affordance a Warp user reaches for exists or is refused in
   writing. No silent absences.
2. Multiple agents run in parallel, isolated by worktree, with attribution.
3. A long build survives closing the app.
4. Linux is exercised, not guessed at.
5. Nothing in the interface is built-but-unreachable — the repository's single most
   recurring bug class, and currently true of an entire information architecture.
6. The security posture has been audited deliberately rather than inherited.
7. The interface answers "what do I do where and when", which the user's own verdict says
   it does not.
8. Every number and claim in the docs is true.

---

## The specs, in execution order

Filenames are stable; this table is the order.

| Order | Spec | Why it is here |
|---|---|---|
| 1 | `00-hardening-audit` | The opener capability grants `**` — every path on disk. And an API key is already stored in plaintext. |
| 2 | `01-terminal-hygiene` | Bell absent. Tabs can't close, rename or reorder. **Block navigation is faked** — the bound keys scroll 10 lines. |
| 3 | `14-terminal-baseline` | The rest of `spec_file.md`'s required baseline: nushell, duplicate/detach pane, config reload, connection profiles. |
| 4 | `02-keybindings` | Fully designed, then `App.tsx:212` calls `new Keymap()` with no argument. The header comment claims persistence that does not exist. |
| 5 | `03-completion` | The largest Warp gap. Branch `completion-driver` is pushed, has no PR, and is called from nowhere. |
| 6 | `04-blocks-remote` | The difference between Blocks being a feature and a property of the terminal. |
| 7 | `05-profiles-appearance` | Agent profiles exist; terminal profiles don't. Plus launch configs, OS appearance sync, dimming. |
| 8 | `08-input-parity` | Vi-mode scrollback, broadcast input, multi-cursor, paste history, annotations, scroll-lock. |
| 9 | `15-ia-rail-inspector` | An entire IA is typed in `store.ts` and renders nothing. **Paper Chrome settles it: five zones, no activity rail, surface nav in the top bar.** |
| 10 | `16-tasks-and-deck` | **Acceptance criterion 4.** Parallel Threads, worktree isolation, diff attribution, the Deck, background tasks. |
| 11 | `06-workflows` | The local half of Warp Drive, as `.tervin/` in git. Notebooks, workflows, markdown viewer. |
| 12 | `07-triggers` | The extension point Tervin lacks entirely. |
| 13 | `17-universal-search` | The palette is strong; the spec's other sources and all eight filters are missing. |
| 14 | `11-agent-ux` | The HANDOFF backlog: no busy state, `runtime.unclassified` discarded, Block over-capture, bash `PromptEnd`, one flaky test. |
| 15 | `09-daemon` | *"The single largest functional gap in this document."* A pane dies with the app. |
| 16 | `18-macos-integration` | Designed in full in the mockup, built almost not at all. Includes a real defect: passwords can reach persisted scrollback. |
| 17 | `19-onboarding-disclosure` | No first-run experience exists. Three steps and Guided/Standard/Expert are designed and unbuilt. |
| 18 | `10-linux` | `linux-ci` found three real defects before being parked — two were unsound tests. |
| 19 | `20-design-pass` | **Paper Chrome.** Implement the design system and the v3 reference build, plus the state-transition audit that a restyle does not fix. |
| 20 | `21-new-surfaces` | Debug Bench, Tasks and History as surfaces, and onboarding as a 620px dialog. Present in v3, absent from the app. |
| 21 | `12-docs-truth` | Three published test counts across three files; a README that both is and is not released. |
| 22 | `13-security-sweep` | Triggers, the daemon, `.tervin/`, notifications and a URL scheme are all new attack surface. |

**22 specs, ~130 slices.**

---

## The design stage: Paper Chrome

The design direction landed as a Claude Design project — `Tervin Design Spec.md`, a
written brief, and `Tervin Workspace v3.dc.html` as the reference build. It supersedes
`docs/reference/tervin-workspace-v2.dc.html`, which is now the old mockup.

**The one idea:** the terminal well is always the darkest surface on screen; chrome is
furniture around it. Two themes, **Paper** (light chrome) and **Graphite** (dark chrome),
are the same layout and type with a different token set — a component never branches on
theme, only tokens do. The well's *content* palette is theme-independent, so a screenshot
of output is the same artefact either way; only the well's background moves, and it stays
the darkest thing on screen in both.

What this changes from what ships today:

- **Type.** IBM Plex Sans for interface, IBM Plex Mono for anything a machine produced or
  will consume. Today: Geist and JetBrains Mono. The split is load-bearing rather than
  decorative — it is how a user tells their own text from the system's.
- **Themes.** Two chrome themes against fifteen shipped ones. Whether the fifteen survive
  as well/ANSI palettes under Paper Chrome's chrome is being established against
  `themes.ts` rather than assumed.
- **Zones.** Top bar 44px (from 42), status rail 26px (from 25), inspector 330px.
- **Accent as a line.** Teal appears only as a 2px underline or left border, a 1px
  outline, a prompt glyph, a fill no taller than 30px, or the seam rule. Nothing else.
- **Motion.** 120ms for colour and opacity only; layout is never animated. The inspector
  appears and disappears rather than sliding, because layout animation reflows text while
  someone is reading output.

**It settles the IA question.** Five zones, and explicitly *"do not invent a sixth"*.
There is no activity rail — surface nav lives in the top bar, and the four layout modes
are compositions of the same zones rather than separate designs. That supersedes the
earlier "rail as an option" decision, and it honours the two-column rule the code chose
for its own reasons.

### Two conflicts the design carries, and how they resolve

**`bypass` mode. Do not build it.** The brief's keyboard table cycles agent mode
`plan → ask → auto → bypass`, and the v2 mockup did the same. The product refuses it
permanently: `README.md` — *"bypassPermissions is deliberately absent from the offered
modes. A one-click way to disable every check cannot be reconciled with telling you your
actions are reviewable"* — and `COMPETITIVE-SPEC.md` §5 lists it under what Tervin should
refuse to build. The cycle ships as `plan → ask → auto`.

**Nocturne is not Tervin's design system.** The same project carries a
`_ds/nocturne-*` bundle: blue-grey ground, Inter, a blurple `#9184d9` accent, Phosphor
icons. It is a generic scaffold, and its accent directly violates Paper Chrome's own rule
against purple-blue AI styling. `Tervin Design Spec.md` and the brief are authoritative;
Nocturne is ignored, apart from its Phosphor icon choice, which Paper Chrome also names.

---

## Security posture — the decisions taken

Best practice, applied. These govern specs 00, 18 and 13.

**Secrets.** Corrected once against the code — see spec 00.4 for the full account.

- Tervin **already stores a secret in plaintext**, but not where this document first said.
  The path is `parse_alias_line` (`profile.rs:437-450`), which copies leading `VAR=value`
  pairs out of a discovered shell alias into `AgentProfile.env`, serialised with no mode
  set and rendered verbatim in Settings. The endpoint API key is *not* persisted at all.
- **The Keychain is the wrong fix here, and this was the roadmap's own mistake.** Keychain
  item ACLs bind to the code signature; Tervin ships unsigned by decision
  (`COMPETITIVE-SPEC.md` §5), and an ad-hoc signature changes every rebuild — so a stored
  item either re-prompts forever or becomes unreadable. That is worse than the plaintext
  file it replaces.
- **Fix:** store the variable's *name* and read its value from the environment at launch.
  Config files carrying user input get `0600` set explicitly, the reasoning `paths.rs:63`
  already applies to `runtime_dir()`.
- **SSH passphrases stay refused.** `ssh-agent` with `--apple-use-keychain` owns that
  natively; duplicating it makes Tervin a new target for no gain. The mockup's row
  overreaches — correct the row, don't build to it.
- **No vault surface.** Tervin does not become a credential manager. `SECURITY.md` §5
  holds unchanged.

**Capability surface.** The `**` opener glob goes. Every Tauri capability entry is
justified or removed. `connect-src` gets a test, not a reading — it is what mechanically
enforces the privacy promise.

**Secure Input.** A password typed at a `sudo` prompt can currently land in persisted
scrollback. macOS exposes secure-input state; pause capture and Block output while it is
active. This is a defect being closed, not a feature.

**Touch ID** gates high-risk approvals when enabled — `rules-engine` already classifies
risk, so it is a gate on an existing signal and it strengthens the permission model.
Off by default.

**The `tervin://` scheme** is untrusted input from anywhere including a web page. It may
navigate; it must not run anything.

**Unchanged and permanent:** no `bypassPermissions` mode, ever — the mockup's mode cycle
includes `bypass` and the mockup is stale on that point. Tervin never answers `allow`
through a hook. No team accounts, SSO, seats or hosted backend. No proprietary agent. No
latency number that is not a latency measurement.

---

## Warp parity, itemised

Terminal-perspective only. Warp's cloud agents, Drive sync and team features are refused
in `COMPETITIVE-SPEC.md` §5 and stay refused — a positioning decision, not a gap.

| Warp capability | Tervin today | Spec |
|---|---|---|
| Blocks with metadata, filtering, sharing | Present, and richer | — |
| Sticky command headers | Present | — |
| Block navigation | **Faked** — bound keys scroll 10 lines | 01 |
| Background blocks | Absent | 01 |
| Command search & history | Present, beyond a shell's | — |
| Tab completions, flag completions | **Absent** | 03 |
| Autosuggestions from history | Absent | 03 |
| Command inspector | Absent | 03 |
| Tabs: close, rename, reorder | **All absent** | 01 |
| Vertical tabs | Present, all four sides | — |
| Split panes, configurable layouts | Present, a real tree | — |
| Launch configurations | Absent | 05 |
| Session restore | Present, honest about dead processes | — |
| Synchronised inputs | Absent | 08 |
| YAML workflows | Absent | 06 |
| Notebooks, markdown viewer | Absent | 06 |
| Themes | Present, 15, light and dark | 05 |
| Custom theme creation | Absent | 05 |
| Sync with OS light/dark | Absent | 05, 18 |
| Opacity, blur, pane dimming | Absent | 05 |
| Desktop notifications, bell | **Both absent** | 01, 18 |
| Global hotkey | Absent | 05 |
| Vim keybindings in the editor | Present | — |
| Settings sync | Refused (§5) | — |
| Linux | Absent | 10 |

---

## Sequencing rationale

**00 first** because the capability surface and the plaintext secret are things later
specs would otherwise build on top of and multiply.

**01, 14, 02 early** because they are what a Warp user hits in the first ten minutes, and
because 03 has a half-finished branch that will rot.

**15 before 16** because Mission Control is a layout mode and the Deck fills it.

**09 late** because the daemon moves the PTY registry out of `tervin-app`, and every
earlier spec touching panes would need rebasing across it.

**10 after 09** because Linux CI on a codebase that just grew a daemon is one debugging
session; Linux CI before it is two.

**19 before 20** because onboarding is a surface the design pass then covers.

**20 second to last** because it can only make coherent what already exists, and because
it consumes a design direction being explored separately.

**13 last** by definition.

---

## Standing constraints on every slice

From `CONTRIBUTING.md` and `ARCHITECTURE.md`. Not negotiable per-spec.

- **Test against the real thing.** A terminal feature drives a PTY; a protocol adapter
  speaks the protocol over a real pipe. Never mock the thing under test.
- **A capability is upgraded by evidence, never by configuration.**
- **`Unsupported` requires a reason; `Partial` requires a note.**
- **Never claim a capability that does not exist.** If a slice cannot be finished, the
  interface says so rather than showing a control that does nothing.
- **Comments explain why.** Constants carry their reasoning.
- **No emoji in the interface.**
- **Unhandled keys reach the terminal.** Only a handled action calls `preventDefault`.
- **Terminal bytes never become JSON. Per-frame state stays out of React.**
- **Nothing leaves the machine that the user did not attach.**

## Traps the swarm must respect

From `HANDOFF.md`, each of which cost real time before:

- Do not edit Rust while `pnpm app` is running — the watcher restarts the app and kills
  live Threads. Branch work happens in a `git worktree`.
- Squash merges only. The PR body becomes the commit message.
- Never `git push --force --all`. It destroyed three merged PRs.
- Point test agents at `~/tervin-testbed` via `./scripts/testbed.sh`, never at Tervin's
  own source.
- `TERM=dumb` disables zsh's ZLE, and with no ZLE there is no completion system at all.
- A shell's line editor calls `tcsetattr` with `TCSAFLUSH` on startup, discarding queued
  input. `PtySession` gates writes until `ICANON` clears. Do not remove that.
- `grep --include=*.rs` needs the pattern quoted in zsh or the glob expands first.
