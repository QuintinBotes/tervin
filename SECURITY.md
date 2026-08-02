# Security

> **In development, and that matters here.** Tervin has not been released, so this
> threat model has never been tested against real use: only against the tests. Two
> specifics worth stating plainly:
>
> - **Release builds are unsigned and unnotarised, deliberately and permanently.** A Developer
>   ID costs $99 a year and this is an open-source project. So **the checksum is the only thing
>   vouching for a binary**, which makes verifying it against `SHA256SUMS.txt` not a formality
>   but the actual security boundary. The installer script and the npm package both verify it
>   and refuse to install without it; a manual download is on you.
> - **No third party has reviewed any of this.** The permission model is documented
>   carefully and its limits are stated, but documented carefully is not the same as
>   audited.

## Reporting a vulnerability

Please report privately rather than in the issue tracker: open a
[GitHub security advisory](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
on this repository.

Include what you did, what happened, and what you expected. If it involves an agent,
say which runtime and whether the permission gate was reported as live: those two
states fail identically from the outside.

Expect an acknowledgement within a few days. Pre-1.0, there is no formal SLA, and
pretending otherwise would be the same kind of overclaiming this project exists to
avoid.

## What Tervin is and is not

**Tervin is a terminal.** It runs the commands you type, with your privileges, in your
shell, reading your rc files. It is not a sandbox and does not try to be one. Anything
you can do in your terminal, an agent in Tervin can attempt.

**Tervin never claims an action is sandboxed when it is not.** This is the load-bearing
security property. Where Tervin can see an action but not prevent it, the UI says
"observed" and `RiskAssessment.enforceable` is false.

## The permission gates, precisely

Two runtimes give Tervin a real pre-execution gate. They are not equally strong.

**ACP** (`session/request_permission`): the agent asks and *blocks* waiting for the
answer. If Tervin never answers, the agent never proceeds. This is the strongest gate
available.

**Claude Code hooks** (`PreToolUse`): Tervin registers a hook and answers over a Unix
socket. A refusal exits 2, which blocks the tool before it runs. **This gate fails
open**: any exit code other than 2 is treated as non-blocking, so if Tervin is
unreachable or slow the action proceeds. This is a property of the hook design, not a
choice. The session's permission text states it, and the hook writes
`This tool call was NOT checked against Tervin Rules` to stderr rather than failing
silently.

**Tervin only ever tightens.** Through a hook it returns `deny` or `defer`: never
`allow`, which would skip the runtime's own permission checks. Enabling Tervin's gate
cannot turn an action the runtime would have asked about into one it performs silently.

**A capability is upgraded by evidence.** `native_permission_bridge` stays `Partial`
until a gate has actually fired, because Claude Code silently ignores settings files
that fail validation: an installed-but-broken gate is indistinguishable from none.

**`bypassPermissions` is not offered.** A one-click way to disable every check cannot
be reconciled with telling a user their actions are reviewable.

## Data handling

**Nothing leaves your machine that you did not attach.** Prompts and explicit
`Attachment`s go to the runtime you selected. There is no code path that sends
scrollback, file contents, or environment variables: the promise is enforced by there
being no other way in. This holds for local endpoints too.

**Everything persists locally.** Blocks, events, and raw runtime payloads live in
SQLite under your data directory. Raw payloads are stored unredacted so an audit is
faithful to what the runtime actually said; redaction happens at export, and an export
states that it redacted rather than implying the original was clean.

**Secrets are named, never read.** Tervin identifies credential files to *refuse* them.
Under ACP, `fs/read_text_file` and `fs/write_text_file` are confined to the session's
project root, enforced after symlink resolution, so a link inside the project cannot
reach outside it, and files matching credential shapes (`.env*`, `id_rsa`,
`id_ed25519`, `*.pem`, `*.key`, `.netrc`, `.npmrc`, `credentials`, `secret*`, and
others) are refused even inside the root. If such a file is genuinely needed, the user
attaches it explicitly.

**An error message never contains a credential.** Only the *presence* of a credential
variable is reported, and only a config directory's path. There is a test for this.

## Protected folders

Tervin's file index **never descends into the folders macOS guards**, `~/Desktop`,
`~/Documents`, `~/Downloads`, `~/Music`, `~/Pictures`, `~/Movies`, `~/Library`, when
they are reached incidentally.

This is a trust property, not an optimisation. Reading `~/Music` makes macOS ask for
access to your media library, and a terminal that triggers a prompt about Apple Music
on launch has spent trust it had no reason to spend. Rooting the index at the home
directory used to do exactly that, and the prompt was impossible to connect to anything
the user had done.

A project that genuinely lives inside one of those folders still works: opening
`~/Documents/project` is an explicit request, and one expected prompt for a directory
you named is a different thing entirely. Tervin also no longer defaults its project
root to the home directory.

## Local attack surface

**The hook socket.** Tervin's `PreToolUse` gate listens on a Unix socket in its runtime
directory. Filesystem permissions are the authentication: the directory is `0700` and
the socket `0600`, set explicitly rather than left to the umask. Anything running as
your user can reach it, so the payload is bounded and input Tervin cannot parse is
*refused* rather than allowed.

**Agent-hosted commands.** Under ACP, `terminal/create` runs commands on the agent's
behalf. These are spawned directly rather than through a shell: the agent supplies a
command and arguments, and `sh -c` would reintroduce word splitting and globbing that
the classifier just reasoned about. Every one goes through Tervin Rules first, and a
session ending kills anything it started.

**No listening network port.** Tervin does not open one. The gate uses a Unix socket
specifically to avoid it.

**Shell integration.** Tervin injects its hook via `ZDOTDIR`, `--init-file`, and
`vendor_conf.d`: writing only inside its own directory. **It never modifies a file you
own**, which is asserted by test. Set `TERVIN_SHELL_INTEGRATION=0` to disable, or turn
injection off in Settings.

**Alias enumeration** runs `$SHELL -ic alias`, which sources your rc files. The output
is treated as untrusted input and only ever parsed, never executed, which is why
discovered profiles are *offered* rather than adopted.

## Known gaps

Stated rather than omitted:

- **Builds are not yet notarised.** macOS warns on first launch until they are.
- **Linux is untested.** The code is written for Unix generally, but untested is not
  supported.
- **A malicious agent is not contained.** Tervin gates what it is asked about. An agent
  that finds a path Tervin does not mediate, through a command that spawns another
  process, for instance, is limited by your OS, not by Tervin. Use the OS mechanisms
  you would use for any untrusted program.
