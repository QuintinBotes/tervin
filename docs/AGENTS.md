# Working with agents in Tervin

A practical guide to the part that is genuinely different from other terminals.

## Which runtime to pick

| You want | Use | Why |
| --- | --- | --- |
| The strongest permission gate | **an ACP agent** | The agent blocks waiting for Tervin's answer. Deny actually denies. |
| Claude Code's own features — hooks, plugins, subagents, skills | **Claude Code (direct)** | Its full feature set, plus a hook-based gate that can refuse actions. |
| Nothing leaving your machine | **a local model** | LM Studio, Ollama, vLLM, llama.cpp. Answers about your work; cannot act. |
| An agent Tervin has never heard of | **Settings › Agents › Add an ACP agent** | Any ACP-speaking command becomes a full structured integration. |
| A tool with no protocol at all | **a managed pane** | Full terminal fidelity, no structured events, and the Bridge panel says so. |

## Agents you start yourself

You do not have to launch an agent from Tervin for Tervin to know about it. Open a
pane, type `claude`, and a Thread appears — titled after your first prompt, showing
the replies, tool calls and file changes, and searchable in History › Prompts
afterwards.

This works because Claude Code announces its own lifecycle over an escape sequence
(`OSC 777;notify;warp://cli-agent`) and, when a turn ends, points at the session's
transcript on disk. Tervin reads the sequence agents already emit and the file they
already write. Nothing to configure, and no plugin to install.

**An observed session is read-only.** Tervin has no channel to a process it did not
spawn, so it cannot send a prompt, answer a permission request, or cancel a turn — and
the Thread shows an explanation instead of a composer rather than a text box that
silently does nothing. Type in the pane itself.

Two consequences worth knowing:

- **Tervin Rules do not gate an agent you started yourself.** The hook gate is
  installed by Tervin when *it* launches Claude Code (see below). A session you start
  by hand uses whatever settings you have configured, and Tervin only records what
  happened. If you want the gate, launch from the Agents surface.
- **Only the interactive TUI announces itself.** `claude -p 'something'` in a pane
  produces a Block like any other command, not a Thread.

Any agent that adopts the same envelope is picked up the same way; nothing in the
handling is specific to Claude Code beyond the names it reports.

## Multiple accounts

Tervin launches agents as direct child processes, so a shell alias cannot be used —
an alias exists only inside an interactive shell. Profiles do the same job explicitly.

A profile sets the binary, arguments, and environment. Crucially, **a profile fully
determines which account runs**: account-selecting variables it does not set are
*removed* from the child's environment, so an ambient `CLAUDE_CONFIG_DIR` cannot decide
for you.

That has a consequence worth knowing:

> A profile that sets no `CLAUDE_CONFIG_DIR` runs the **default** account
> (`~/.claude`) — not whichever account your shell aliases select.

If Tervin reports a 401 while `claude` works fine in your terminal, that is why.
Settings › Agents lists the profiles Tervin found from your aliases and config
directories; adopt the one you actually use and set it as the default. The error
message names the account it ran as, for exactly this reason.

Nothing is adopted automatically. Discovery reads your aliases by asking your shell to
list them, and that output is parsed, never executed — so a discovered profile is
*offered*.

## Reading the permission status

The Thread panel states who decides. There are four states and they are not
interchangeable:

**"Tervin Rules gate this Thread"** — a gate has fired. A refusal stops the action
before it runs.

**"…installed but not consulted yet"** — a gate is configured and unproven. Treat
approvals as the runtime's own until the first tool call arrives. Tervin will not claim
a gate on the strength of configuration, because a settings file that failed validation
is silently ignored.

**"Provider-native approvals"** — the runtime decides. Tervin shows what is proposed and
can stop the session, but does not gate individual actions.

**"Nothing to approve"** — a model endpoint. It cannot act.

A risk chip marked **"observed"** means Tervin classified the action but could not
prevent it. That distinction is the whole point; nothing in the interface blurs it.

### What the hook gate cannot do

Claude Code's hook gate **fails open**. Any exit code other than 2 is non-blocking, so
if Tervin is unreachable the action proceeds. The hook writes
`This tool call was NOT checked against Tervin Rules` to stderr rather than failing
quietly, and it appears in the agent's transcript.

Tervin also never answers `allow` through a hook — only `deny` or `defer`. Turning the
gate on can only add refusals; it can never turn something the runtime would have asked
about into something it does silently.

## Your own hooks

Tervin loads its gate with `--settings`, which *adds* settings. Your own configuration
is never read, rewritten, or overridden.

It also passes `--include-hook-events`, so your hooks become visible. A hook that
fails is named in the Thread panel with its exit code and stderr — hooks otherwise run
silently, and a broken one degrades every session with no message anywhere. A hook that
*blocks* something appears in the timeline as a denial attributed to the runtime, not
to Tervin.

## MCP

Tervin's config directory holds `mcp.json`, which uses the same `mcpServers` format every MCP client uses, so
an existing configuration can be pasted in unchanged.

These servers go to **ACP agents only**. Under ACP the client supplies MCP servers and
the agent has no config of its own — without this, an ACP agent would have none at all.
Claude Code reads its own configuration and Tervin deliberately does not add to it:
silently changing an agent's available tools is not something a terminal should do.

Set `"disabled": true` to turn a server off without losing its configuration.

## Handing work between agents

**Hand off** on a Thread turns its recorded work into a briefing another agent can
read: the task, the plan, files touched, commands and exit codes, tests, open problems,
and anything that was refused.

It loads into the composer rather than sending. Pick who receives it, read it, edit it
— nothing is shared until you submit.

What a bundle leaves out, and says it left out:

- **Reasoning traces**, because a receiving model reads a predecessor's thinking as
  established fact.
- **Full command output** — bounded excerpts only.
- **Everything not in the event stream.** No scrollback, no file contents, no
  environment.

A handoff taken from a Thread that is still working says so: *"It was still working
(editing) when this handoff was taken, so its changes may be incomplete."*

## What an agent can reach

Under ACP, Tervin hosts the agent's filesystem and command work, which is what makes
the gate cover them:

- `fs/read_text_file` and `fs/write_text_file` are confined to the session's project
  root, enforced **after** symlink resolution — a link inside the project cannot reach
  outside it.
- Credential-shaped files are refused even inside the root: `.env*`, `id_rsa`,
  `id_ed25519`, `*.pem`, `*.key`, `.netrc`, `.npmrc`, `credentials`, `secret*`, and
  others. If one is genuinely needed, attach it explicitly — the decision stays yours.
- `terminal/*` commands go through Tervin Rules first, are spawned directly rather than
  through a shell, and are killed when the session ends.

## Attachments

Nothing reaches a provider that you did not attach. Attach a Block, a diff, a
selection, or a file with `@path` in the composer. Every attachment appears in the
timeline as `context.attached`, so the transcript records what was shared.

This holds for local endpoints too. They feel safe, which is exactly what makes the
shortcut tempting.
