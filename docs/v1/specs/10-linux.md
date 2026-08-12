# Spec 10 — Linux support

`COMPETITIVE-SPEC.md` §3.1, P1. **Windows is out of scope for v1.0** by decision.

## Context

The code is Unix-general already and the PTY layer has no macOS-specific assumptions. Per
§3.1: *"the honest blocker is that nothing has been run there."* `README.md` says it
plainly — *"'Should work on Linux' is not a claim, it is a guess."*

Branch `linux-ci` (PR #31) is open and parked. `HANDOFF.md` records that it **found three
real defects before being parked, two of them unsound tests that had never tested what
they claimed.** That is the strongest available argument for finishing it: the Linux run
is not only about Linux, it is a second opinion on the test suite.

Sequenced after the daemon (spec 09) so the daemon is debugged on one platform before
being debugged on two.

## Slices

### 10.1 — Revive `linux-ci` and triage what it found
Rebase PR #31 onto current `main` and re-run. Re-establish the three defects — in
particular the two unsound tests, since an unsound test is worse than a missing one and
those two may have relatives.

**Trap:** the repository ruleset requires up-to-date branches and allows squash merges
only. `gh pr update-branch 31`, then re-arm `--auto`.

*Exit:* a written list of every failure on Linux, each classified as a real defect, a
platform assumption, or an unsound test.

### 10.2 — Fix the platform assumptions
Expected areas, based on what the code touches:

- `crates/terminal-core/src/pty.rs` — `libc::tcgetattr` on the master fd for the input
  gate. `ICANON` semantics are POSIX, but the master-side behaviour differs between
  Darwin and Linux and this is load-bearing.
- `crates/tervin-core/src/paths.rs` — already branches to `~/.config/tervin`; verify
  `runtime_dir()` and its `0700` creation.
- `crates/shell-integration` — `ZDOTDIR`, `--init-file` and `vendor_conf.d` are all
  portable; `vendor_conf.d`'s location is not.
- `crates/session-manager` — serial device paths, `ssh-add` output format.
- Clipboard, notifications, and the global hotkey from spec 05.6 are the desktop-
  integration surface, and Wayland and X11 differ.

*Exit:* `cargo test --workspace` green on Linux.

### 10.3 — CI on Linux
Add a Linux job to `.github/workflows/ci.yml`. Mirror the macOS job's discipline: it
fails the build if `vim`/`less`/`zsh` are missing, specifically so silent test skips
cannot hide — the Linux job needs the same guard or the suite will quietly shrink.

*Exit:* CI green on macOS and Linux. A missing test binary fails rather than skips.

### 10.4 — Packaging
A Linux artifact. `packaging/` currently carries Homebrew and npm. Decide the format —
AppImage is the honest default for a Tauri app with no distro relationships — and produce
a checksummed release asset alongside the existing `SHA256SUMS.txt`, which the installer
already *requires* rather than warns about.

*Exit:* a Linux artifact builds in CI and verifies against the published checksum.

### 10.5 — The claim, changed last
§3.1's exit criterion is explicit and unusually strict: *"CI green on all three, and the
README's platform claim changes only after a human has actually used each for a day."*

Two of three here, since Windows is out of scope. Update `README.md`, `SECURITY.md`
("Linux is untested" under Known gaps) and `COMPETITIVE-SPEC.md` §1 **only after a human
has used Tervin on Linux for a day** — not when CI goes green. Windows stays listed as
not claimed, because it is not.

*Exit:* the docs say what is true. A green CI run alone does not change them.

## Verification

```sh
cargo test --workspace          # on Linux
cargo clippy --workspace --all-targets -- -D warnings
pnpm exec vitest run
```

Manual: a day of real use on Linux, following `docs/MANUAL-TEST.md` end to end at least
once. The daemon from spec 09 gets particular attention — Unix sockets and process
lifetime are where the platforms diverge most.
