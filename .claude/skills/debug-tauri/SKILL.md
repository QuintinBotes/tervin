---
name: debug-tauri
description: Reproduce, diagnose, fix, and regression-test a local Tervin desktop issue through WebdriverIO.
argument-hint: "[bug report or reproduction steps]"
disable-model-invocation: true
allowed-tools: Read Grep Glob Bash Edit Write
---

# Debugging Tervin through the real window

`$ARGUMENTS` is a symptom, a bug report, or a flow someone wants exercised. Turn it
into a failing test, find the cause with evidence, make the smallest fix that holds,
and leave the test behind.

The rule this repository runs on applies here in full: **test against the real thing,
or say plainly that you did not.** A scenario that could not be reproduced is a
finding, not a failure to report.

## 1. Read the report and the ground already covered

Read `$ARGUMENTS`. Then look at what exists before writing anything:

```sh
ls tests/e2e/                      # every spec, and the closest one to this symptom
sed -n '1,60p' tests/e2e/wdio.conf.ts
grep -rn "data-testid" ui/src | sed 's/:.*data-testid=/ -> /'
```

Prefer extending the nearest existing spec to inventing a new one. If a spec already
covers the surface, the bug usually belongs in it.

## 2. Classify the failure layer

Say which one, and why, before touching code. Choosing wrong sends the whole
investigation into the wrong crate.

| Layer | What it looks like | Where to look |
|---|---|---|
| startup/build | `pnpm e2e:build` fails, or the window never opens | `cargo build -p tervin-app --features e2e` output |
| Rust backend | command returns an error, panics, or hangs | `crates/*/src`, backend log lines in the run output |
| WebView/frontend | wrong render, stale state, dead control | `ui/src`, browser console via the spec |
| IPC boundary | `invoke` rejects, argument or serde mismatch | `crates/tervin-app/src/commands.rs` and `ui/src/lib/api.ts` |
| permission/capability | Tauri denies a command that is not listed | `crates/tervin-app/capabilities/default.json` |
| packaging/debug-build | works in dev, not in the built binary | `crates/tervin-app/Cargo.toml`, `tauri.conf.json` |

Two traps worth knowing, both already paid for once:

- A blank window with `about:blank` and no `#root` is **not** a frontend bug. It means
  the binary was built without `tauri/custom-protocol`, so Tauri is waiting on the
  vite dev server. Build with `pnpm e2e:build`.
- `getCSSProperty()` returns values through WebdriverIO's CSS parser, which lowercases
  and keeps only the first font family. When a style assertion fails against a UI that
  looks correct in the screenshot, read `getComputedStyle` through `browser.execute`
  before believing the app is wrong.

## 3. Reproduce before diagnosing

Write the narrowest spec that fails, or add a failing case to the closest existing
one. Then run **only** that spec:

```sh
pnpm test:e2e:spec -- tests/e2e/<spec>.e2e.ts
```

Never start from the whole suite: it costs minutes per run and tells you less.

Selectors are `data-testid`, lowercase kebab case, named for intent
(`settings-close-button`, not `nav-btn-3`). Add one to `ui/src` when the control the
scenario needs has none. Do not select by CSS class, visible text, or position, and
do not click coordinates.

## 4. Gather evidence, and say which layer it indicts

A failed run leaves artefacts. Read them before forming a theory:

```sh
ls artifacts/e2e/screenshots/          # the window at the moment it failed
tail -80 artifacts/e2e/logs/wdio.log   # WebDriver commands, and backend lines
```

The screenshot is the fastest way to tell "the app is wrong" from "the assertion is
wrong" — those are different bugs with different fixes. Collect whatever applies:
failing selector and action, frontend console output, Rust log lines, and for an
`invoke` failure the command name, the argument shape, the Rust signature, and the
serde error. For a crash, rerun under a backtrace:

```sh
RUST_BACKTRACE=1 pnpm test:e2e:spec -- tests/e2e/<spec>.e2e.ts
```

Then state the root cause in one sentence, naming the file and line that causes it.
**Do not edit before this sentence exists.** If the evidence does not support a cause,
gather more; a guess wearing the costume of a diagnosis is worse than an open bug.

## 5. Fix, then prove it

Smallest safe change that addresses the cause rather than the symptom. One concern
per change. Then, in order:

```sh
pnpm test:e2e:spec -- tests/e2e/<spec>.e2e.ts   # the original scenario, now passing
pnpm test                                        # UI unit tests
pnpm typecheck
cargo test --workspace                           # if any Rust changed
cargo fmt && cargo clippy
```

Keep the scenario as a regression test. A test written to reproduce a bug and deleted
after the fix leaves nothing behind that would catch it coming back.

## 6. Report

State: the classification, the evidence that pinned it, the root cause, the files
changed, and the exact commands run with their outcomes. If verification was blocked,
say which step and why — never mark an issue fixed without a passing reproduction or a
stated reason it could not run.

## Never

- Run release packaging (`pnpm app:build`) unless explicitly asked.
- Touch credentials, secret files, or `agents.toml`; never log a token, key, cookie, or
  environment variable's contents.
- Delete user data. The suite runs against a throwaway `HOME` — clean up only what the
  test itself created, and never the developer's real profile.
- Widen `capabilities/default.json`, or add a blanket permission, to get past a denial.
  A denial is usually the correct answer and the bug is the caller.
- Register the WebDriver plugin outside the `e2e` Cargo feature, or ship it in a
  release build.
- Claim a fix without a passing test.
