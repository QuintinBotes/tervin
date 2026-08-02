# Contributing to Tervin

Thanks for looking. This document is short on ceremony and specific about the two
things that are genuinely unusual here: the honesty rule and the testing standard.

> **In development.** Nothing is released yet, so there is no compatibility to preserve —
> which makes this the best time to change something badly designed. A pull request that
> replaces a decision is more welcome now than it will ever be again.

## Getting set up

```sh
npm install
npm run app                # dev, hot reload
cargo test --workspace     # 456 tests
npx vitest run             # 80 UI tests
```

Requires Rust 1.82+, Node 20+, and a Unix-like OS. macOS is the tested platform.

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) before a substantial change. It
explains why the boundaries are where they are, which will save you from a
well-intentioned refactor that removes a constraint on purpose.

## The honesty rule

> Tervin never claims a capability it does not have.

This is not a style preference; it is the product. A pull request that makes Tervin
look more capable than it is will be declined even if the code is good.

Concretely:

- **Do not upgrade a capability on the strength of configuration.** A capability
  becomes `Supported` when something has been observed working, not when it has been
  set up. If you cannot observe it, it stays `Partial` with a note saying why.
- **`Unsupported` requires a reason, `Partial` requires a note.** The type enforces
  this. Do not write "not supported" — write what the user should do instead.
- **Never present an observation as a gate.** If Tervin can see an action but not
  stop it, `enforceable` is false and the UI says "observed".
- **Do not drop what you cannot classify.** Emit `runtime.unclassified` and keep the
  raw payload.
- **Do not add a code path that sends anything the user did not attach.** No
  scrollback, no file contents, no environment. This holds for local endpoints too —
  they feel safe, which is what makes the temptation real.

## The testing standard

**Test against the real thing, or say plainly that you did not.**

Mocks are allowed for things that are genuinely external and slow — but not for the
thing under test. If you are writing a terminal feature, drive a PTY. If you are
writing a protocol adapter, speak the protocol over a real pipe or socket.

Every bug this codebase has had in its riskiest areas was found by a test like that
and missed by review. Some of them:

- `permissions()` took the same non-reentrant lock twice in one struct literal.
  Temporaries live to the end of the statement, so it deadlocked. Found by a test
  calling it.
- `shutdown()` called `AsyncWrite::shutdown` on a child's stdin, which does not close
  the descriptor. The agent never saw EOF and outlived the session. Found by a test
  asserting the process was gone.
- A cancelled model turn hung forever if the server went quiet, because the cancel
  flag was only checked when a chunk arrived. Found by a test with a stalling server.

### Writing a good test here

- **Name it as a claim about behaviour.** `a_denied_permission_is_actually_denied`,
  not `test_permissions`. The name should tell a reader what breaks if it fails.
- **Assert the thing that matters, not the thing that is easy.** "The gate ran" is
  weak. "The command did not execute and the agent was told why" is the claim.
- **Say why in a comment when the reason is not obvious.** Especially for a constant,
  a bound, or a defensive branch. `// A killed process has no exit status of its own`
  is worth more than the line it sits above.
- **Prefer a failing assertion message that diagnoses.** Include the actual value.

### Opt-in tests

Anything needing network, credentials, or a paid API is gated behind an environment
variable and skipped by default:

```sh
TERVIN_LIVE_CLAUDE=1 cargo test -p agent-runtime -- the_real_cli_honours_a_refusal
```

Tests that need an absent binary return early rather than failing — a contributor
without `vim` installed should not see a red suite.

## Code style

`cargo fmt` and `cargo clippy` before pushing. Beyond that:

**Comments explain why, never what.** `// increment the counter` above `n += 1` is
noise. `// Removals must remove: an empty CLAUDE_CONFIG_DIR is an empty path, not an
absent one` is the reason the line exists and stops someone "simplifying" it back into
a bug.

**Match the surrounding code.** Comment density, naming, and idiom vary a little by
crate. Follow the file you are in.

**Constants carry their reasoning.** Every bound in this codebase has a comment
explaining the number. If you add one without a justification, it will be asked for.

**No emoji in the interface.** See [docs/DESIGN.md](docs/DESIGN.md); it is an
instant reject.

## Pull requests

- One concern per PR. A refactor and a fix in the same diff cannot be reviewed.
- Say what you verified and how. "Tested manually" is not a verification; "drove
  `vim` through the change and the alternate-screen assertions still pass" is.
- If you removed a constraint, say why it was safe. Several of them look arbitrary
  and are not.
- If something is unverified, write it in the PR. That is always acceptable;
  discovering it later is not.

## Reporting a bug

Include the platform, the shell, and whether shell integration was active. For
anything involving an agent, include the runtime and whether the permission gate was
reported as live — the two failure modes look identical from the outside and that
detail separates them.

Security issues go to [SECURITY.md](SECURITY.md), not the issue tracker.
