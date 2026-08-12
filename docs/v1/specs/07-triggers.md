# Spec 07 — Triggers

`COMPETITIVE-SPEC.md` §3.5, P2. *"This is the extension point Tervin lacks entirely."*

## Context

iTerm2 has had triggers for years: a regex over output that fires an action — highlight,
notify, run a command, capture. Tervin has no extension point at all. It has no plugin
system, no scripting API, and no Lua or Python surface, and §5 does not refuse one; it
simply has not been built.

Triggers are the cheapest real extension point available, because the matching substrate
already exists. `crates/terminal-core/src/osc.rs` already scans every byte at 1.0–1.24
GiB/s, and `crates/block-engine/src/parse.rs` already extracts paths, ports, diagnostics
and test summaries from output.

The Tervin-specific version is the interesting one, from §3.5: *"a trigger whose action is
'start a Thread with this output attached' turns a build failure into an agent task
without a human noticing it first."*

## Slices

### 07.1 — The matching engine
Regex over output, evaluated per line, on the Rust side where the bytes already are.
Compiled once and cached; `regex` is already a workspace dependency.

This runs on the hottest path in the product. `PERFORMANCE.md` sets the budget: OSC
scanning must stay faster than output arrives. Benchmark before and after — the existing
`crates/terminal-core/benches/osc_scan.rs` is the right harness — and put the numbers in
the PR, which is the repository's stated policy since benchmarks are deliberately not in
CI.

A trigger set that measurably slows the scanner is capped or refused, and the cap is
stated. Do not let an unbounded user regex become an unstated performance cliff.

*Exit:* a benchmark comparison across zero, ten and fifty triggers. Throughput stated.

### 07.2 — Actions: highlight, notify, capture
The three safe ones. Highlight decorates the matched region in the pane. Notify raises
the in-app notice (and the OS notification, once spec 01.1 adds one). Capture writes the
match to a Block annotation.

None of these executes anything, so none needs a gate.

*Exit:* a regex on `error:` highlights and notifies. A capture is retrievable later.

### 07.3 — Action: run a command
This one executes, so it is governed. Per §3.5: *"Gate it behind Tervin Rules like
anything else that runs, and never let a trigger execute a command without either a rule
or a confirmation."*

Route through `crates/rules-engine` exactly as an agent action is routed. A trigger is an
unattended actor; a compound command from a trigger gets the same splitting and the same
`Moderate`-and-unenforceable treatment for anything unparseable.

*Exit:* a trigger running `rm -rf` is stopped by Rules. A test asserts a trigger cannot
execute without a rule or a confirmation.

### 07.4 — Action: start a Thread with the output attached
The differentiator. A build failure becomes an agent task, with the matched output as
attached context.

Constraint from `SECURITY.md` and `CONTRIBUTING.md`: nothing leaves the machine that the
user did not attach. A trigger attaching output *is* the user attaching it, but only if
they wrote the trigger — so this action requires explicit per-trigger opt-in, and the
Thread shows what was attached.

*Exit:* a failing `cargo test` starts a Thread carrying the failure output, and the
Thread's context list names it.

### 07.5 — Management UI
A section in Settings: list, add, edit, enable/disable, test-against-a-sample. Triggers
are stored per-project and globally, and a project trigger set in `.tervin/` (spec 06)
requires consent before it can run anything.

*Exit:* a trigger can be authored and tested against pasted sample output without
attaching it to a live pane.

## Verification

```sh
cargo test --workspace
cargo bench -p terminal-core
pnpm exec vitest run
```

Manual: a trigger on `warning:` while building Tervin itself; a trigger that tries to run
something Rules forbids.
