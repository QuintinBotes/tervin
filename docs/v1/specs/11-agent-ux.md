# Spec 11 — Agent honesty & UX gaps

The `HANDOFF.md` backlog, plus the pattern behind it.

## Context

`HANDOFF.md` identifies two recurring bug classes, and says the second was fixed six
times individually while the pattern was never addressed:

> **The app does something and does not say so.** A plan proposed with no indication. A
> button that appears dead because the agent already moved on. A message queued silently
> while the agent is busy. A divider that looked unresizable because the target was 5px.
> Each was fixed individually; the pattern was not. This is what the user meant by
> "unclear what to do where and when".

Every slice here is an instance. The user's own verdict on the interface was that it
"does not behave well at all" — recorded as parked item #16, which they chose to defer
until the roadmap was done. This spec is the part of #16 that is well-understood enough
to build without a design pass.

## Slices

### 11.1 — Say when the agent is busy
`HANDOFF.md` next-step #2, and *"the highest-value item from the user's UX complaints."*

Sending a turn while the agent is mid-turn queues it with no acknowledgement — no busy
state, no "queued", nothing changes. The user reported this as *"the agent is useless in
responding."*

Same root as the Plan surface's Approve button appearing to do nothing. Show: the agent
is working, your message is queued, and where it sits.

*Exit:* sending during a turn shows the message as queued and shows it leaving the queue.
A test asserts the queued state is rendered.

### 11.2 — Surface unmodelled runtime messages
`HANDOFF.md` next-step #3. `runtime.unclassified` is filtered out of the timeline
entirely, so anything the normalizer does not model is invisible rather than plain.

That is exactly how subagent `task_progress` hid — *"the data was never missing, it was
in the discard bucket."* A count in the Bridge panel turns the next such discovery from a
bug report into a glance.

This is the honesty rule applied to Tervin's own blind spots: `CONTRIBUTING.md` says do
not drop what you cannot classify, emit `runtime.unclassified` and keep the raw payload.
The adapter does. The UI throws it away, which defeats the point.

*Exit:* the Bridge panel shows a count of unclassified events per runtime, with the raw
payload inspectable.

### 11.3 — Block over-capture past `133;D`
`HANDOFF.md` #10. Block output captures the next prompt's bytes past its `133;D`. Escapes
are stripped for display now, so it is **invisible rather than absent** — which is the
worse failure of the two.

`crates/block-engine/src/builder.rs` owns the boundary.

*Exit:* a PTY test asserts a Block's captured output ends at `133;D` with no prompt bytes.

### 11.4 — bash never emits `PromptEnd`
`HANDOFF.md` #8. In the injected login shell bash never emits `133;B`: the marker goes on
`PS1` and anything setting `PS1` afterwards drops it. Blocks still form; anything keyed on
`PromptEnd` gets nothing from bash.

Either find a placement that survives a later `PS1` assignment, or declare bash `Partial`
with a note — the capability model exists for exactly this, and a stated partial is better
than a silent one.

*Exit:* either bash emits `133;B` reliably, or its capability says why it does not.

### 11.5 — The flaky test
`HANDOFF.md` #17.
`session-manager`'s `a_closed_port_reads_as_refused_rather_than_as_a_timeout` failed once
under full-workspace load and passes in isolation. Per the handoff: *"An intermittently
red suite trains people to re-run rather than read."*

*Exit:* the test passes under `cargo test --workspace` load, repeatedly, or its timing
assumption is removed.

### 11.6 — Sweep for the pattern
The five above are known instances. Find the rest rather than waiting for them: audit
every asynchronous state transition in the UI for whether it is visible — every queue,
every debounce, every "the agent already moved on" case, every control that is enabled
when it cannot act.

`DESIGN.md` lists "Silent disabled controls" as an instant reject, and `HANDOFF.md`
records that all 63 Tauri commands are now called *"but check new fields, not just
commands"* — the six-in-one-day bugs were fields plumbed and never set.

*Exit:* a written list of every state transition and its indication, with gaps fixed or
recorded.

## Verification

```sh
cargo test --workspace
pnpm exec vitest run
./scripts/testbed.sh && pnpm app
```

Manual: `docs/MANUAL-TEST.md` §7 and §8 in full. Start a **fresh Thread after any
rebuild** — one launched by the previous binary holds a socket that no longer answers and
will keep failing regardless of what was fixed.
