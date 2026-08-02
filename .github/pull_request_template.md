## What this changes

<!-- One or two sentences. -->

## How you verified it

<!--
"Tested manually" is not a verification. What did you run, and what did it show?
If you changed anything in the terminal or an adapter, say whether the real thing was
driven — a PTY, a real subprocess, a real socket.
-->

## Checklist

- [ ] `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass
- [ ] `cargo test --workspace` and `npx vitest run` pass
- [ ] Anything I could not verify is stated here as unverified

## If this touches capabilities or permissions

- [ ] No capability is reported as `Supported` without evidence it works
- [ ] Every `Unsupported` carries a reason and every `Partial` carries a note
- [ ] Nothing presents an observation as a gate

## If this removed a constraint

<!--
Several look arbitrary and are not — the empty-value-means-remove rule in `apply_env`,
the single-writer rule for the block filter, the protected-folder skip in the file
index. If you removed one, say why it was safe.
-->
