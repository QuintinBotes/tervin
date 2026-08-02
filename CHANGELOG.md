# Changelog

Notable changes, newest first. Kept by hand rather than generated from commits: a commit
log says what changed, and this is for what it means to someone using it.

## Unreleased

### Saved commands, with the parts that change named

- Half the commands anyone runs are a shape with one thing different: deploy to *this*
  environment, tail *that* service. Shell history gives you the last one you happened to
  type, which is the wrong instance of the shape more often than not — so people keep a
  scratch file and copy out of it. **⌘⇧S is that file**, with the holes made explicit:
  `kubectl logs -f {{service}} --namespace {{env:staging}}`.
- **It fills in and does not run.** A saved command is often the destructive kind, and
  seeing the filled-in line before sending it is the whole safeguard.
- **A hole left blank stays visible in the command.** `rm -rf {{path}}` with nothing filled
  in must not become `rm -rf` — a command that fails loudly beats one that runs with an
  argument silently missing.
- **The parser is deliberately narrow.** A hole is exactly `{{name}}` or
  `{{name:default}}`. `${HOME}`, `awk '{print $1}'`, a JSON body and `mv x.{txt,md}` are
  ordinary text and survive byte for byte — treating any brace as a hole would corrupt the
  command someone saved, and they would only find out when it ran.
- Parsing and rendering happen in Rust, not in the UI. A second implementation would
  eventually disagree about `${HOME}`, and the disagreement would be a broken command.
- A name repeated in a template is one parameter filled in both places, and saving over an
  existing name refines it without resetting how often you have used it.
### `cd` knows where you have been

- **⌘J jumps to any directory a pane has sat in**, ranked by how often you go there and
  how recently, then by what you typed. An empty box shows where you usually are; a typed
  one shows the thing you mean. This is what people install `z` or `autojump` for.
- It **fills in `cd` and leaves the newline to you**. Running a command in someone's shell
  because they pressed Enter in a picker is a more surprising thing than filling it in, and
  a wrong path is trivial to fix before sending.
- A directory that has since been deleted is **shown struck through with an offer to
  forget it**, not hidden. Quietly dropping it looks like a lost result, and "gone" is
  exactly what someone needs to know before wondering why `cd` failed.
- Bound to **⌘J**, not Tab. zsh and fish completion is better than anything Tervin would
  write for arbitrary commands, and taking Tab would replace something good with something
  worse.

### Fixed: a pane's directory never updated after it opened

`pane://cwd` had been emitted since Blocks existed and nothing listened to it — and
`BlockEvent::CwdChanged` did not carry a pane id, so nothing *could*. A pane's directory
stayed whatever it was when it spawned, which made the status rail stale, saved the wrong
directory into a restored session, and made per-pane completion impossible.

With that fixed, `@path` completion in the composer is now scoped to the focused pane's
directory, so `@src/…` in a split means that pane's `src` rather than the project root's.
### Commands an agent ran are Blocks now

- A command an agent runs is the same kind of thing as one you ran, so it becomes a
  **Block**: searchable with the rest of your history, bookmarkable, with parsed
  diagnostics you can jump to. `Block::thread_id` existed for this and had never been set.
- **It does not claim an exit code unless a runtime reported one.** An ACP terminal
  reports a real status; Claude Code reports success or failure and nothing more, and the
  0/1/130 on its events is Tervin's inference. A Block from the latter carries no number
  and says *"no exit status reported"* — because an exit code is the one field people read
  as fact, and a fabricated one is worse than an admitted gap.
- **It says the log is partial, and for the right reason.** Adapters pass a bounded
  excerpt, which is a different thing from a shell Block hitting the capture limit —
  showing the wrong reason would send someone looking for a setting that is not involved.
- A Block an agent ran is **marked as such** in the list. In a mixed list that is the
  difference between "I did that" and "an agent did that", which changes how a failure
  reads.
- This covers agents Tervin launched *and* one you ran yourself in a pane: a `Bash` call
  in a session transcript now produces a real command with its stdout and stderr, paired
  to its own call by id rather than by position.

### Programs in a pane are told when the theme changes

- Tervin ships fifteen themes, and switching between them is a normal thing to do — but
  until now a program in a pane never learned the background had changed, so a
  light-theme editor stayed styled for a dark one.
- DEC mode **2031** is now tracked per pane, `CSI ? 996 n` ("is your background light or
  dark?") is answered, and a pane that subscribed is told when the theme changes.
- The report goes **only** to programs that asked. Sending an unsolicited `CSI ? 997` to
  a shell that never enabled the mode would put stray characters on its command line.
- The scanner records queries rather than answering them, for the same reason it does not
  answer an OSC 52 clipboard *read*: the reply travels back as input to the program, so it
  belongs where the rest of Tervin's writes are made, not inside a byte tap.
- Light or dark is read from the theme's own declaration rather than measured from its
  background colour, because the theme's author already decided.

### The workspace comes back when you reopen it

- **Tabs, splits, each pane's directory and its recent output are restored on launch.**
  Losing the arrangement you built — four panes, each in the right place, one on a remote
  host — is the cost that keeps people running tmux under a terminal that cannot do this.
- **The processes are not revived, and each pane says so.** They exited with the app. A
  restored pane starts a fresh shell below its old output, under a line reading *"restored
  from your last session; nothing above is running"* — because a restored screen is
  otherwise indistinguishable from a live one, and someone could believe a command is
  still going.
- Restored output is written straight to the terminal and never through the PTY, so
  replaying it cannot fabricate Blocks for commands that already ran.
- A pane's saved output is only returned if the pane is running **the same program**. A
  local shell's history cannot reappear inside an SSH session, which on a remote host
  would be misleading in a way that matters.
- Panes hosting an agent Tervin launched are deliberately left out: that session is gone,
  and reopening it as a bare shell would be a different thing wearing its title. Those
  Threads are on disk and reachable from History.
- Output is capped, kept in the same local database as Blocks, ages out on the same
  retention window, and is **deleted the moment the setting is switched off** — which the
  UI says at the time rather than leaving to be discovered.

### Agents you start yourself are now part of the workspace

- **Open a pane, type `claude`, and it becomes a Thread.** Titled after your first
  prompt, with the replies, tool calls and file changes on the timeline, and searchable
  in prompt history afterwards. Nothing to install and nothing to configure.
- It works by reading `OSC 777;notify;warp://cli-agent`, which Claude Code already
  emits, and then the transcript it already writes — the sequence and its fields were
  captured from a real PTY rather than taken from documentation. Verified that it is
  *not* gated on `TERM_PROGRAM`, so Tervin setting its own does not suppress it.
- **An observed session is read-only, and says so.** Tervin has no channel to a process
  it did not spawn, so the Thread shows an explanation instead of a composer rather than
  a text box that silently does nothing.
- Only the interactive TUI announces itself; `claude -p` in a pane is a Block like any
  other command. And Tervin Rules do not gate a session you started by hand — that gate
  is installed when Tervin launches the agent.
- Any agent adopting the same envelope is picked up the same way.
- A desktop notification requested over OSC 777 is surfaced in Tervin's notice rail
  rather than raised as a system notification: a process asking for one is not the same
  as the person wanting one, and the request can come from a remote host.

### Every component is reachable, and tested where it mounts

- A static test now fails the build if a component is imported nowhere. It caught
  `GitPanel` and `ConnectionsPanel` — both complete, both unreachable, both the same
  shape as the crash that shipped in History. `GitPanel` now sits beside the diff in
  Review; `ConnectionsPanel` opens on `⌘⇧O`.
- Surfaces are mounted in tests against the data that actually breaks rendering: null
  exit codes, a mid-rebase detached HEAD, an unusable SSH pattern, control characters,
  a command long enough to be a paste accident.

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
