# Performance

Measured numbers, the budgets they are measured against, and the limits that are still
real. Everything here comes from `cargo bench` on an Apple M-series laptop; reproduce
with:

```sh
cargo bench -p terminal-core --bench osc_scan
cargo bench -p file-index   --bench fuzzy_match
cargo bench -p rules-engine --bench classify
```

Numbers from a different machine will differ. The *ratios* and the limits are the
durable part.

> **In development.** Tervin has not been released and its local data formats are not
> stable. This document describes the code as it stands, including the parts that are
> deliberately incomplete. See the [status notice](../README.md#status) in the README.

## The three paths that matter

Only three places in Tervin are in a loop tight enough for a constant factor to be
felt. Everything else is dominated by a subprocess, a disk, or a network.

| Path | Runs on | Budget | Why that budget |
| --- | --- | --- | --- |
| OSC scanning | every byte of terminal output | faster than output arrives | Fall behind and the terminal lags in a way no profile localises. |
| Fuzzy matching | every keystroke in the picker | one frame (~16 ms) | Slower and the list visibly trails your typing. |
| Risk classification | every gated agent action | imperceptible | Under ACP the agent is *blocked* waiting for the answer. |

## OSC scanning

| Output shape | Throughput |
| --- | --- |
| Plain build output | **1.24 GiB/s** |
| Colour-heavy (`cargo`, `jest`, `pytest`) | **1.00 GiB/s** |
| Dense with prompt marks and cwd reports | **434 MiB/s** |

A PTY on this hardware delivers output at a small fraction of that, so the scanner is
not the constraint on how fast a terminal can draw.

**Chunk size barely matters**, which is the more interesting result:

| Read size | Throughput |
| --- | --- |
| 512 B | 998 MiB/s |
| 4 KiB | 1.02 GiB/s |
| 64 KiB | 1.01 GiB/s |

Only ~1% is lost going from 64 KiB reads to 512 B ones. That is worth knowing because
a PTY hands over small, irregular reads, and the state carried between them is exactly
where the marker-splitting bug lived. The carry-over is cheap as well as correct.

Marker-dense output is ~3× slower per byte than plain text because each sequence is
parsed rather than skipped. It is also the rarest shape: a few hundred bytes per
prompt.

## Fuzzy matching

Ranking a whole corpus, worst case, with no result cap:

| Query | 2,000 files | 20,000 files |
| --- | --- | --- |
| `s` (1 char) | 277 µs | **2.87 ms** |
| `sm` | 247 µs | 2.52 ms |
| `uicomp` | 155 µs | 1.58 ms |
| `acpnorm` | 64 µs | **0.65 ms** |

### The prefilter, and what the benchmark found

The first measurement showed `acpnorm` at **11.4 ms** for 20,000 files: inside a frame,
but `FileIndex::MAX_ENTRIES` is 200,000, which extrapolates to ~114 ms and a visibly
laggy picker.

The cause was that every candidate paid the full O(query × candidate) dynamic program
plus five allocations, even when it could not possibly match. A greedy forward
subsequence scan now rejects those first, allocating nothing.

It is safe rather than merely fast: an alignment exists only if the query is a
subsequence, and a leftmost-first scan finds one whenever one exists. So the prefilter
can never reject a candidate the DP would have matched. Case comparison also stopped
allocating: `char::to_lowercase` returns an iterator because one character can
lowercase to several, and paying for that per DP cell dominated the inner loop while
nearly every path in a repository is ASCII.

The longer and more specific the query, the more it helps, which is the right shape,
because a longer query is what a user types when the list is not yet what they want.

### Reusing the scratch space

The prefilter left a second cost visible, **six allocations per candidate**: the
character vectors, the positional bonuses, and the three DP tables. For a short query
that fixed cost dominated the dynamic program it existed to serve, which is why a
one-character query barely improved.

`Matcher` now owns those buffers and `rank` keeps one for the whole pass, turning six
allocations per candidate into six for the entire ranking. `clear` then `resize` rather
than `resize` alone, so a shorter candidate cannot read stale cells left by a longer one.

Both changes together, at 20,000 files:

| Query | Original | Prefilter | Reused buffers | Total |
| --- | --- | --- | --- | --- |
| `acpnorm` | 11.4 ms | 0.66 ms | 0.65 ms | **17.5× faster** |
| `uicomp` | 10.6 ms | 2.05 ms | 1.58 ms | **6.7× faster** |
| `sm` | 6.39 ms | 4.06 ms | 2.52 ms | **2.5× faster** |
| `s` | 5.43 ms | 5.29 ms | 2.87 ms | **1.9× faster** |

The prefilter carried the specific queries; the buffers carried the short ones. Each
was invisible to the other, and neither would have been found without measuring.

### The limit that is still real

**A single-character query remains the slowest case**, at 2.87 ms per 20,000 files. The
prefilter cannot help it, one character matches nearly everything, so the DP runs for
almost every candidate, and the allocations are already gone.

Extrapolated to the 200,000-entry cap that is ~29 ms: still a perceptible hitch on the
first keystroke in a very large repository, down from ~53 ms. It is not a hang, it
affects only the least selective query possible, and it resolves as soon as a second
character is typed.

Stated here rather than left to be discovered. Going further would mean not scoring the
whole corpus, capping candidates, or indexing by first character, which trades exact
ranking for speed. That is a real trade and is not being made silently.

## Risk classification

| Command | Time |
| --- | --- |
| `rm -rf /` | **1.09 µs** |
| `cargo test --workspace` | 1.61 µs |
| `echo $(curl … \| sh)` | 2.50 µs |
| `cd build && make -j8 && sudo make install; echo done` | 4.33 µs |
| A long `docker run …` | 5.35 µs |

Microseconds against a gate an agent is waiting on. Classification is never the reason
an agent stalled.

Compound commands cost ~4× a simple one, and that is deliberate: `a && b; c | d` is
split into segments *before* anything is classified, so `echo hi && rm -rf /` is never
judged on `echo`. Correctness costs a split, and 4 µs is the whole price.

## Choices made for performance elsewhere

Not benchmarked, because the alternative is not slower so much as structurally wrong.

**Terminal bytes never become JSON.** Output crosses IPC as raw binary and arrives as
an `ArrayBuffer`. Encoding a build log as a JSON string array costs several times the
bytes plus a parse per frame.

**Scrollback is not in React state.** It lives inside each xterm instance, which owns
its own buffer and renderer. Putting bytes in a store would re-render on every frame of
output.

**Output is coalesced before it crosses the boundary.** The PTY pump batches on a 6 ms
interval or 32 KiB, whichever comes first, with a 120 ms ceiling while synchronized
output is held. A process printing a line at a time would otherwise generate an IPC
message per line.

**Block output spills to disk past 256 KiB**, capped at 64 MiB. Holding a full build
log in memory per Block does not survive a day's work.

**Blocking work stays off the async runtime.** Git and SQLite calls run on a blocking
pool. The exception is the terminal write path, a lock and a `write`, because routing
a keystroke through a task queue adds latency to typing.

**Discovery probes are short and parallel-safe.** Local model endpoints get 1.5 s; the
common case is that none of them are running, and startup must not pay for that.

## Release profile

`lto = "thin"`, `codegen-units = 1`, `panic = "abort"`, `strip = true`. Debug builds use
`opt-level = 1` with dependencies at 2, because a debug build with unoptimised
dependencies is too slow to drive a terminal and makes the PTY tests flaky.

## Benchmarks are not in CI

Deliberately. Shared runners vary enough between jobs that a threshold either fires
constantly or is set so loose it catches nothing, and a check nobody trusts is worse
than no check. Run them locally before and after a change to a hot path, and put the
numbers in the pull request.
