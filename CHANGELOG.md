# Changelog

Notable changes, newest first. Kept by hand rather than generated from commits: a commit
log says what changed, and this is for what it means to someone using it.

## Unreleased

### The permission gates became real

- **Claude Code now has a genuine pre-execution gate**, through a `PreToolUse` hook that
  answers over a Unix socket. A refusal stops the tool before it runs — verified against
  the real CLI, not against documentation.
- Tervin **never answers `allow`** through a hook, only `deny` or `defer`. `allow` would
  skip Claude Code's own permission checks, and a safety feature that disables another
  safety feature is not one.
- A capability is upgraded **by evidence, never by configuration**. `claude --help`
  states that settings files failing validation are silently ignored, so an installed
  gate that has never fired is indistinguishable from no gate — and is reported as
  unproven until it fires.
- The hook gate **fails open** by design: any exit code but 2 is non-blocking, so an
  unreachable Tervin does not stop your work. The session's permission text says so, and
  the hook writes `This tool call was NOT checked against Tervin Rules` rather than
  failing quietly.

### One adapter for every ACP agent

- Gemini CLI, GitHub Copilot CLI, Claude Code via the Zed bridge, and anything else
  speaking the Agent Client Protocol — added from Settings by command line, with no
  release needed. Under ACP the agent *blocks* waiting for Tervin's answer, which is a
  stronger gate than a hook.
- Tervin hosts the agent's filesystem and command work, confined to the project root
  after symlink resolution, refusing credential-shaped files, and routing every command
  through Tervin Rules first.

### Local models

- LM Studio, Ollama, vLLM, llama.cpp, and any OpenAI-compatible endpoint. A new
  `Conversational` tier says plainly that a model answers and cannot act, rather than
  dressing it up as an agent with most capabilities switched off.
- Token counts are reported; **no price is invented** for a model on your own machine.

### Moving work between agents

- **Hand off** turns a Thread into a briefing another agent can read — task, plan, files,
  commands and exit codes, tests, open problems, and what was refused. It drops reasoning
  traces, because a receiving model reads a predecessor's thinking as established fact,
  and it states what it left out.

### History

- A **History surface**: every command searchable months later by its output as well as
  its text, which a shell's history cannot do.
- **Agent prompts are searchable too**, with a 30-day retention window. Blocks are never
  pruned — a command and its output stay useful for years, while a transcript does not.

### Prompt editing

- `native`, `emacs`/readline, and `vim` modes for the composer. `⌘⏎` sends and plain `⏎`
  is a newline, because a prompt is usually several paragraphs.

### Layout

- The tab strip can live on any of the four sides, and `+` makes a tab rather than
  splitting the current one — which used to make a tab look like it had been renamed.
- A **file explorer**, lazily loaded. Clicking a file types its shell-quoted path into
  the focused pane rather than opening an editor.

### Fixed

- **A spurious macOS media-library prompt on every launch.** The file index rooted at the
  home directory walked `~/Music`, and macOS asks about your media library when anything
  reads it. Five hypotheses were eliminated first: it is not in the Info.plist, no media
  framework is linked or loaded, it happens with the canvas renderer as well as WebGL,
  and a hardened runtime with no media entitlements does not stop it.
- **Keystrokes reached the terminal while a dialog was open** — including `Return` in an
  approval sheet, which ran a command instead of answering a question about running one.
- **`shutdown()` leaked the agent process.** `AsyncWrite::shutdown` does not close a
  child's stdin, so the agent never saw EOF and outlived the session.
- **A profile's binary and arguments were ignored**, and empty environment values were
  *set* rather than removed — so `CLAUDE_CONFIG_DIR=""` selected the wrong account and
  produced a 401 that looked like an expired login.
- **A deadlock in `permissions()`**: two `lock()` calls in one struct literal, where the
  temporaries live to the end of the statement and the lock is not reentrant.
- A cancelled model turn hung forever if the server went quiet.
- Fuzzy matching is **17× faster** on a specific query and **1.9× faster** on the worst
  case — a subsequence prefilter plus reused scratch buffers. See docs/PERFORMANCE.md.

## 0.1.0

First public build.
