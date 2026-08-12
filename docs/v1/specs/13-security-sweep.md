# Spec 13 — Final security & best-practices re-sweep

**Runs last.** Spec 00 hardened what existed; this audits everything the roadmap added,
including the code the swarm wrote.

## Context

Twelve specs of new code will have landed between 00 and here, and three of them opened
genuinely new attack surface:

- **Spec 07 (Triggers)** introduced user-authored regexes that can *run commands* and
  *start agent Threads with output attached*. That is a new unattended actor.
- **Spec 09 (Daemon)** introduced a long-lived process outliving the app, holding every
  PTY, reachable over a Unix socket carrying every keystroke and every byte of output.
- **Spec 06 (`.tervin/`)** introduced a directory read from a cloned repository.

Each of those is exactly the kind of feature that is safe as designed and unsafe as
extended. Re-auditing them together, after they exist, catches what auditing them
individually at design time cannot.

## Slices

### 13.1 — Re-audit the daemon boundary
The socket carries more than the hook socket ever did. Confirm:

- Mode bits `0600` in a `0700` directory, set explicitly, asserted by test.
- The daemon authenticates the client, or the filesystem permissions genuinely suffice
  and that reasoning is written down — as it is for the hook socket in `SECURITY.md`.
- A malformed frame is refused, not parsed optimistically.
- Version skew refuses rather than guessing.
- No listening TCP port was introduced. `SECURITY.md` states plainly that Tervin does not
  open one, and stage 3 (attach from another machine) is the obvious pressure on that
  claim. If stage 3 landed, either it is Unix-socket-and-SSH-forwarding only, or
  `SECURITY.md` changes and says exactly what opened.

*Exit:* a test per item. `SECURITY.md` describes the daemon.

### 13.2 — Re-audit triggers as an execution path
A trigger is an actor that runs without a human present. Confirm:

- Every executing trigger goes through `rules-engine`, with compound-command splitting.
- A trigger cannot execute without a rule or a confirmation, asserted by test.
- A `.tervin/` trigger set from a cloned repository cannot run on project open.
- Regex denial-of-service: a catastrophically backtracking pattern on the hottest path in
  the product. Rust's `regex` crate has linear-time guarantees, which is the right answer
  — confirm nothing bypasses it, and confirm the per-trigger cost cap from 07.1 holds.
- The Thread-attachment action attaches only what the trigger matched, and the Thread
  shows what was attached.

*Exit:* a test per item.

### 13.3 — Re-audit the `.tervin/` trust boundary
Content from a cloned repository is untrusted input. Confirm nothing in `.tervin/` can
execute, write outside the project, install a hook, add an MCP server, or change a policy
without consent. Confirm a malformed file reports rather than fails silently — the
existing pattern for `mcp.json` and `agents.toml`.

*Exit:* a hostile `.tervin/` fixture in the test suite that does nothing on open.

### 13.4 — Full dependency and supply-chain re-check
Re-run everything from 00.6 against the final dependency set. The roadmap added at least
one xterm addon, possibly a markdown renderer (06.4), and whatever the daemon needed.

A markdown renderer is the notable one: rendering untrusted markdown inside a webview is
an XSS surface, and the CSP is the last line rather than the first. Sanitise, and assert
it with a test carrying a script tag and a `javascript:` URL.

*Exit:* `cargo audit`, `cargo deny`, `pnpm audit` clean. Actions still SHA-pinned. A
markdown XSS test passes.

### 13.5 — Re-verify the honesty machinery end to end
The promise is mechanical, so verify it mechanically:

- Every `CapabilityLevel::Unsupported` carries a non-empty reason and every `Partial` a
  non-empty note — tests already assert this in three adapters; extend to anything new.
- No capability was upgraded on the strength of configuration. In particular
  `native_permission_bridge` still requires an observed firing, and spec 09.4 did not
  let session restore claim live processes it does not have.
- No new code path sends anything the user did not attach. Grep the diff for every
  outbound call and confirm each is a local endpoint the user configured.
- `bypassPermissions` is still absent from the offered modes.
- Tervin still never answers `allow` through a hook.

*Exit:* a written confirmation per item, each backed by a test or a diff review.

### 13.6 — Threat-model refresh
`SECURITY.md` §"Local attack surface" and §"Known gaps" describe a product that no longer
exists in full. Rewrite both for the v1.0 shape: the daemon, triggers, `.tervin/`, remote
hook installation (spec 04.2), and Linux.

Keep the register. The current document's strength is that it states what a checksum does
*not* give you, and that a malicious agent is not contained. Add the new gaps in the same
voice rather than describing the new features as mitigations.

*Exit:* `SECURITY.md` describes the shipped product, gaps included.

## Verification

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo audit && cargo deny check
pnpm exec vitest run && pnpm audit
TERVIN_LIVE_CLAUDE=1 cargo test -p agent-runtime -- the_real_cli_honours_a_refusal
```

That last one is the test `docs/TESTING.md` says to run if you run only one: it proves
the `PreToolUse` hook genuinely blocks rather than being politely ignored. It is the
central claim of the product and it belongs in the final sweep.

Manual: `docs/MANUAL-TEST.md` end to end, especially §8 — *"The gate panel does not show
`Tervin did not answer within 5s`. That line means the gate is failing open and nothing is
being checked."*
