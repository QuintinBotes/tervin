# Spec 04 — Blocks over SSH and subshells

`COMPETITIVE-SPEC.md` §3.3, P1.

## Context

Today a Block needs Tervin's hook, and the hook lives on the local machine. Warp's
shell integration, blocks and AI survive an SSH hop, `nvm`, a venv, `docker exec` and
`kubectl exec`. Per the spec: *"This is the difference between Blocks being a feature and
being a property of the terminal."*

Tervin's existing mechanism is the right foundation and unusually well-behaved: it
injects integration per pane via `ZDOTDIR`, `--init-file` and `vendor_conf.d`, and
**never modifies a file the user owns** — asserted by test
(`crates/shell-integration/src/injection.rs`). Whatever this spec adds must preserve that
property, including on a remote host.

## Slices

### 04.1 — Detect the boundary
`OSC 7` already carries a hostname, and `crates/terminal-core/src/signals.rs:192,210`
already distinguishes local from remote. The trap is recorded in `HANDOFF.md`: **OSC 7
carries a hostname, not nothing** — zsh sets `$HOST`, so a bare "is it empty" test
classifies every local `cd` as remote. `Some(host)` must mean genuinely elsewhere.

Extend detection to subshell entry: a new shell inside the same pane (nvm, venv,
`docker exec`, `kubectl exec`) that no longer emits `133` marks.

*Exit:* entering and leaving an SSH session, and entering a `docker exec` shell, each
produce a distinct, tested state transition.

### 04.2 — Offer to install the hook remotely, with consent
Per §3.3: *"offer to install the hook on a remote host on first connect, with the diff
shown and consent required."*

Show exactly what would be written and where. Never write without an answer. Never touch
a file the user owns on the remote either — the same rule as locally. Remember the answer
per host; offer an uninstall.

kitty's SSH kitten copies terminfo to the remote host automatically and
`COMPETITIVE-SPEC.md` §2 calls that out as quietly fixing one of the most common remote
annoyances. Consider carrying terminfo in the same consented step.

*Exit:* connecting to a host offers installation, shows the diff, and does nothing on
refusal. Accepting produces Blocks on the remote.

### 04.3 — Re-emit integration into a subshell where possible
Where a subshell can be reached (`nvm use` in the same shell, a venv activate), re-emit
the integration rather than treating it as a new host. Where it cannot, fall to 04.4.

*Exit:* `nvm use 20` keeps Blocks forming in the same pane.

### 04.4 — Say when Blocks are unavailable, and why
Where neither remote install nor re-emission is possible — `docker exec` into a
distroless image, a shell Tervin has no hook for — the pane says Blocks are unavailable
and why. This is the existing pattern (`ui/src/components/ConnectionsPanel.tsx:114`
already says "Tervin's shell hook does not support this shell") and it is the honest
outcome, not a fallback.

*Exit:* a pane in a hook-less environment carries a stated reason, not silence.

### 04.5 — Remote Blocks carry their host
`Block` already has `host` in its model (`crates/block-engine/src/model.rs:178-201`).
Ensure remote Blocks record it, that History can filter by host, and that
`scrollback_load`'s existing refusal — output is only returned to a pane running the same
program, so a local shell's history can never reappear in an SSH session — still holds
with the hook installed remotely.

*Exit:* History filters by host. The scrollback isolation test still passes.

## Standing constraint

`SECURITY.md`: Tervin never modifies a file the user owns, and never stores credentials.
Remote installation must not cache a passphrase, and must not require one beyond what the
user's own `ssh` already does.

## Verification

```sh
cargo test --workspace
```

Manual: `ssh` to a real host, accept installation, run `ls` and confirm a Block forms
with the right host. Then `docker run -it alpine sh` and confirm the unavailable notice.
