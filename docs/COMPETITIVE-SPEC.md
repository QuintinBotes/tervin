# Where Tervin stands, and where it should go

> **In development.** Tervin v0.1.0 is the first release. This document is a review of every
> terminal people actually use, an honest account of what Tervin lacks, and a specification
> for the work that follows. See the [status notice](../README.md#status) in the README.

Two things this document is not. It is not a marketing comparison: where a competitor is
better, it says so plainly, and where Tervin is behind for a good reason it says that too.
And it is not a wish list: every item is either specified well enough to build or marked as
a decision that has not been made.

Researched August 2026. Sources are listed at the end.

---

## 1. The honest baseline

What Tervin v0.1.0 actually has, verified against the code rather than the roadmap:

**Terminal.** PTY sessions, pane tree with arbitrary splits, zoom, swap, drag and keyboard
resize. Search over scrollback. Sixel and iTerm2 inline images. Ligatures. Unicode 11 width
handling. OSC 7, 8, 52, 133, 777 and 7373. DEC private modes including synchronised output
and colour-scheme reporting. Paste safety. Copy on select. Smart selection. 15 themes. A
34-binding keymap with context scoping. Session restore. Automatic shell integration that
never writes to an rc file.

**Blocks.** Every command with its output, exit code, duration, parsed diagnostics, test
results and ports. Searchable months later over FTS5. Bookmarks and tags.

**Agents.** Claude Code over `stream-json` with a `PreToolUse` hook that genuinely blocks.
Any ACP agent. Codex over `codex exec --json`. OpenAI-compatible local models. Agents you
start yourself in a pane become Threads, read from the escape sequence they already emit and
the transcript they already write. Commands an agent runs become Blocks.

**Honesty machinery.** `CapabilityLevel::{Supported, Partial, Unsupported, Unknown}` with a
reason on every absence. `exit_code_reported` so a derived exit status is never shown as a
measured one. Tervin never answers `allow` on a runtime's behalf.

**Not present, and worth saying:** no Linux or Windows build, no signing (§5, a decision
rather than a gap), no kitty keyboard
or graphics protocol, no scripting API, no plugin system, no team or sync features, no
instant replay, no broadcast input, no vi-mode scrollback, no tmux control mode, no CLI flag
completion.

---

## 2. Terminal by terminal

### Warp: the only true competitor

Warp is the product Tervin is closest to, and in several dimensions ahead of it. Being
specific matters more than being reassuring.

**What Warp has that Tervin does not:**

| Gap | Detail | Verdict |
| --- | --- | --- |
| **Cloud agents (Oz)** | Event-triggered autonomous agents in containers, reacting to webhooks, CI, cron and Slack. 20 to 40 concurrent depending on tier. | **Close it, differently.** See §4. |
| **Warp Drive** | Team-synced storage of workflows, notebooks, environment profiles and MCP server lists. | **Close the local half, refuse the cloud half.** See §4.4. |
| **Cross-platform** | GA on macOS, Linux (X11 and Wayland) and Windows, including ARM64. | **Close it.** §3.1. |
| **CLI flag completion** | Subcommand and flag completion for hundreds of CLIs, no plugin needed. | **Close it.** §3.2. |
| **Blocks over SSH** | Shell integration, blocks and AI survive an SSH hop and subshells (`nvm`, venv, `docker exec`, `kubectl exec`). | **Close it.** §3.3. |
| **Notebook blocks** | A command, its output and prose, shared as a link with execution context. | Partly. §4.4. |
| **Team features** | SAML SSO, zero data retention, shared rules. | **Deliberately not.** §5. |
| **Multi-cursor input** | The prompt editor has multi-cursor. | Minor; §3.6. |

**What Tervin has that Warp does not:**

- **A permission gate that actually blocks.** Warp's agent asks for approval in its own UI;
  Tervin's `PreToolUse` hook stops the tool before it runs, and where it cannot it says so.
  Nothing in Warp models the difference between "I stopped this" and "I watched this".
- **Agents it did not launch.** Tervin reads a `claude` session you started yourself in a
  pane. Warp's agent is Warp's agent.
- **Runtime pluralism.** Any ACP agent, Claude Code, Codex, and local models, each with a
  declared capability level. Warp is one agent with model choice.
- **No account required.** Warp's AI is a credit system behind a login. Tervin runs against
  your own subscriptions, or entirely locally.
- **Exit-code honesty.** Warp reports what its agent tells it.

**The uncomfortable summary:** Warp is ahead on reach (platforms, cloud, teams) and on the
polish of completion. Tervin is ahead on trustworthiness and on working with the agent
ecosystem rather than replacing it. That is the axis to press.

### iTerm2: fifteen years of accumulated features

The deepest feature set of any macOS terminal, and the source of most "why can't Tervin do
X" questions from experienced users.

**Gaps worth closing:**

- **tmux control mode (`tmux -CC`).** Remote tmux windows render as *native* tabs and
  splits. Nobody else has this. Tervin can attach to tmux but only as a normal pane.
  **§3.4: high value, moderate work.**
- **Triggers.** Regex on output firing actions: highlight, notify, run a command, capture.
  **§3.5: this is the extension point Tervin lacks entirely.**
- **Instant replay.** Scrub terminal output backwards like video, with timestamps.
  **Reconsider:** Tervin's Blocks already answer "what did that command print", which is the
  usual reason people reach for replay. Full replay is a large feature for the remainder.
- **Python API.** Automate windows, panes, profiles, tmux.
- **Shell-integration extras.** Click to download a file over SCP, drag and drop to upload,
  per-host command history, recent directories by frecency. Tervin has the frecency part.
- **Profiles.** Named terminal configurations, distinct from Tervin's *agent* profiles.
- **Composer, paste history, copy mode with vi keys, annotations.**

**Where Tervin is ahead:** Blocks as first-class records, agent integration, and a design
system rather than fifteen years of preference panes. iTerm2's own LLM chat is a side panel;
Tervin's Threads are part of the workspace model.

### Ghostty: the performance and native-feel benchmark

Fastest on macOS in 2026 tests, 2 to 5× WezTerm's rendering. Native AppKit UI. Flat
`key = value` config with live reload. Kitty graphics and keyboard protocols. Session
save and restore. `⌘F` search. OSC 133 click-to-position. 25M-line scrollback in one line
of config.

**Gaps:** Tervin's renderer is xterm.js in a WebView, which will not match a native GPU
terminal on latency or memory. This is a **real, structural disadvantage** and should be
stated rather than benchmarked away. §3.7 covers what can be done without a rewrite.

**Also missing:** kitty keyboard protocol (xterm.js does not implement it), kitty graphics
protocol, live-reloading text config, and a scrollback ceiling anywhere near 25M lines.

### kitty: the protocol author

Originated the kitty graphics and keyboard protocols, now adopted by Ghostty, WezTerm and
others. Python "kittens" as an extension system. Built-in tiling layouts. Broadcast input to
all panes. An SSH kitten that copies terminfo to the remote host automatically.

**Gaps:** the two protocols, broadcast input, a scriptable extension system, and the
terminfo-copying SSH trick, which quietly fixes one of the most common remote annoyances.

### WezTerm: the multiplexer

The best built-in multiplexer of any terminal, plus **built-in SSH multiplexing that needs
nothing installed server-side**, all three image protocols, serial port support, and Lua
configuration that is a real programming language.

**Gaps:** its SSH multiplexing is the standout. Tervin has serial support already.

### Alacritty: the argument for doing less

Deliberately omits tabs, splits, ligatures and images. ~20MB resident, fastest renderer.
Vi mode for scrollback with regex hints.

**The lesson, not a gap:** Alacritty is a reminder that every feature in this document has a
cost. Tervin should not become the terminal that does everything badly. Vi-mode scrollback is
worth taking.

### Apple Terminal.app

The baseline. Present on every Mac, starts instantly, and is what a new user compares
against. It has one thing Tervin should not lose sight of: it is *there*, with nothing to
install. Tervin's answer is `npx tervin`, which is close.

### Windows Terminal, Hyper, Tabby, Rio, Contour

Relevant mainly for cross-platform expectations. Windows Terminal sets the bar on Windows
(panes, profiles, JSON config, ConPTY). Tabby is the one to study for **SSH and serial
connection management with saved profiles and a vault**: closer to a connection manager than
a terminal, and Tervin's Connections panel is heading the same way.

### tmux and zellij: what people run *inside* a terminal

This is the category Tervin most needs to answer, because it is why people do not care which
terminal they use.

**tmux:** mature session persistence across disconnection, deep scripting, a large plugin
ecosystem. The reason it survives everything.

**zellij:** floating panes as a native feature, KDL layout files that save an entire
workspace including running commands, sandboxed WASM plugins, and a keybinding hint bar.
Native session persistence is still on its roadmap as of 2026.

**Gaps for Tervin:**

- **Detach and reattach.** A Tervin pane dies with the app. Session restore replays layout
  and scrollback but not *processes*. tmux's whole value is that a long build survives a
  closed laptop. **§3.8: the single largest functional gap in this document.**
- **Floating panes.** §3.9.
- **Layout files as a shareable artefact.** Tervin saves a session automatically; zellij lets
  you commit `layout.kdl` to a repository. §3.10.
- **Broadcast input.**
- **A keybinding hint bar** for discoverability.

### Shells and the tools people bolt on

Much of what users call "my terminal" is actually the shell and four tools. Being honest
about the overlap:

| Tool | What it gives | Tervin's position |
| --- | --- | --- |
| **fish** | Autosuggestions and syntax highlighting with no plugins, friendlier config, better completions. | Tervin supports fish as a shell. Its *inline* autosuggestion is a shell feature and should stay there. |
| **zsh + compsys** | Every installed completion spec, already correct. | **The argument for §3.2 being "ask the shell" rather than "ship specs".** |
| **atuin** | SQLite history with full-text search, per-directory scoping, exit code, duration, hostname, session, encrypted cross-machine sync. | **Tervin already does the local half better** (it also has output, diagnostics and tests). atuin has **sync**, which Tervin does not. §4.5. |
| **zoxide** | Frecency `cd`. | **Done**: ⌘J, shipped in v0.1.0. |
| **fzf** | Fuzzy everything, `Ctrl-R`. | Done for commands, directories and paths. |
| **starship** | Fast informative prompt. | Prompt is the shell's business. Tervin should never fight it. |

**The strategic reading:** atuin and zoxide are the two tools Tervin genuinely subsumes, and
it should say so. fish and starship are not competitors and Tervin should keep out of their
way.

### The agentic newcomers

- **Zed 1.0** shipped April 2026 with **parallel agents** as its headline feature: Codex CLI
  alongside Claude Agent and Gemini CLI in one window, over ACP. Zed authored ACP; 25+ agents
  and JetBrains, Google and GitHub have adopted it. Copilot CLI added ACP in January 2026.
- **VS Code** standardised on MCP rather than ACP, and has custom agents with handoff buttons
  that carry context to a suggested next agent.
- **Cursor** has cloud agents alongside local ones.

**The gap that matters most:** Tervin speaks ACP to *one* agent at a time. Zed runs several
**in parallel, in one window, with a thread each**. Tervin has the Threads model and the Deck
to do this and does not yet do it. **§4.1.**

**AGENTS.md** is read by 30+ tools and adopted in 60,000+ repositories. Tervin does not read
it. **§4.2: small work, real interoperability.**

---

## 3. Parity specification

Ordered by value per unit of work, not by section number.

### 3.1 Linux and Windows builds
`P1.` The code is Unix-general already and the PTY layer has no macOS-specific assumptions;
the honest blocker is that nothing has been *run* there. Add both to CI first, fix what
breaks, and only then claim support. Windows needs ConPTY behind the `portable-pty`
abstraction and a decision about shell integration, which has no equivalent to `ZDOTDIR`.

*Exit criteria:* CI green on all three, and the README's platform claim changes only after a
human has actually used each for a day.

### 3.2 CLI flag and subcommand completion
`P1.` The largest remaining Warp gap. Three approaches, and this specification picks one:

1. **Ship spec data** (Warp, Fig). A corpus to vendor and keep current; useless for internal
   tools. Rejected.
2. **Execute `--help` and parse it.** Works for any binary including private ones, but
   **executes an arbitrary program from `PATH`** to learn its flags. fish deliberately parses
   man pages instead, precisely to avoid that. Rejected as a default.
3. **Ask the user's shell.** zsh's compsys already knows every installed spec and is already
   correct. Drive it in a subshell and render the results in Tervin's own menu.

**Chosen: 3, with 2 as an explicit opt-in.** Rationale: zero guessing, zero execution of
anything the user has not already installed a completion for, and it inherits every spec they
already have. The cost is being shell-specific and fiddly to drive; fish and bash need their
own path, and an unknown shell falls back to path and history completion, which already work.

*Exit criteria:* `git ` offers subcommands, `git commit -` offers flags, both sourced from the
shell. A shell Tervin cannot drive degrades silently to today's behaviour and says so once in
the Bridge panel.

### 3.3 Blocks and shell integration across SSH and subshells
`P1.` Today a Block needs Tervin's hook, which lives on the local machine. Warp survives an
SSH hop, `nvm`, a venv, `docker exec` and `kubectl exec`. This is the difference between
Blocks being a feature and being a *property of the terminal*.

*Approach:* Tervin already injects integration per pane without touching rc files. Extend that
to (a) offer to install the hook on a remote host on first connect, with the diff shown and
consent required, and (b) detect subshell entry from OSC 7 and re-emit integration where the
new shell allows it. Where neither is possible, the pane says Blocks are unavailable and why,
which is the existing pattern.

### 3.4 tmux control mode
`P2.` Render a remote tmux session's windows and panes as native Tervin tabs and splits, as
iTerm2 does with `tmux -CC`. Requires speaking tmux's control-mode protocol and mapping it
onto the existing pane tree, which is already a tree of the right shape. Flow control matters:
iTerm2 needed tmux 3.2 to avoid excessive buffering.

*Why it is worth it:* it makes Tervin the best client for the multiplexer people already run,
rather than asking them to abandon it.

### 3.5 Triggers
`P2.` Regex over output firing an action: highlight, notify, capture to a Block, run a
command, or **hand to an agent**. iTerm2 has had this for years and it is Tervin's missing
extension point.

*The Tervin-specific version:* a trigger whose action is "start a Thread with this output
attached" turns a build failure into an agent task without a human noticing it first. Gate it
behind Tervin Rules like anything else that runs, and never let a trigger execute a command
without either a rule or a confirmation.

### 3.6 Smaller parity items
`P3.` Vi-mode scrollback with regex hints (Alacritty). Broadcast input to all panes (kitty,
zellij). Terminal *profiles*, distinct from agent profiles (iTerm2, Windows Terminal).
Multi-cursor in the composer (Warp). A keybinding hint bar (zellij). Paste history (iTerm2).
Annotations on scrollback regions (iTerm2).

### 3.7 Renderer honesty
`P2.` xterm.js in a WebView will not match Ghostty on latency or memory. Do not pretend
otherwise. Two concrete actions: publish a measured comparison in `PERFORMANCE.md` including
the cases where Tervin loses, and cap the damage: the WebGL renderer, a scrollback ceiling
that is honest about its cost, and no per-frame work in React.

**Not planned:** a native renderer rewrite. It would consume every remaining unit of effort
for a benefit most users of an agent-native terminal will not notice. Revisit only if
measurement shows Tervin is unusable on large output, which it currently is not.

**Also not planned:** kitty keyboard and graphics protocols. xterm.js implements neither, so
claiming them is impossible without replacing the renderer. Say so in the docs rather than
leaving people to discover it.

### 3.8 Detach and reattach: processes that survive
`P1, and the largest gap in this document.` tmux's entire value is that a build survives a
closed laptop. Tervin's session restore replays layout and scrollback; the processes are gone,
and the UI says so honestly, but honesty is not the same as capability.

*Approach:* a small supervisor process that owns the PTYs and outlives the app, with the app
as a client that attaches over a Unix socket. This is what tmux and zellij do, and it is a
significant architectural change: the PTY registry moves out of `tervin-app` into a daemon,
and `terminal-core` grows a client mode.

*Staging:* (1) daemon owns PTYs for the current app session, with the app reattaching after a
crash or reload. (2) Daemon survives app exit; panes reattach on next launch with live
processes. (3) Attach from a second window or machine, which is where it starts competing with
tmux directly.

*Honest note:* until stage 2 lands, "session restore" should keep saying plainly that processes
are not revived. It does.

### 3.9 Floating panes
`P3.` A pane that overlays the tiled layout, persists across tab switches, and can be moved
and resized. zellij has this natively and it is genuinely useful for a scratch shell or a log
tail. The pane tree does not need to change: a floating pane is a sibling of the root, not a
node in it.

### 3.10 Layout files as artefacts
`P3.` Session restore is automatic and invisible. zellij's `layout.kdl` is a *file you commit*,
so a repository can describe the workspace for working on it: three panes, the right
directories, the dev server already running. Add an exportable layout, and read one from
`.tervin/layout.toml` if a project has one, with consent before anything runs.

---

## 4. Beyond parity: the agentic specification

This is the part Tervin should be judged on. Everything above makes it a good terminal;
this is what makes it worth choosing.

The organising principle: **Tervin is where a human and several agents work on one codebase
together, and it never lies about which of them did what.**

### 4.1 Parallel Threads, properly
`P1.` Zed 1.0's headline feature is several agents in one window. Tervin has the Threads
model, the Deck and the capability system to do this better, because it can also say what each
agent is *allowed* to do.

*Specification:*
- N concurrent Threads, each with its own runtime, model and permission state.
- The Deck becomes the primary surface: one row per Thread with state, current action, cost,
  and whether Tervin can gate it.
- **Per-Thread worktree isolation.** Two agents editing one working tree is the fastest way to
  produce a mess neither of them understands. Each Thread gets a git worktree by default;
  sharing one is opt-in and labelled.
- Review aggregates diffs across Threads and attributes every change to the Thread that made
  it, with the Block that produced it linked.
- A conflict between two Threads' edits is surfaced as a conflict, not silently resolved.

*Why Tervin can do this better than an editor:* the terminal is where the commands actually
run, so attribution is a fact rather than an inference.

### 4.2 Read what the ecosystem already writes
`P1.` Cheap interoperability that Tervin currently declines:

- **`AGENTS.md`**: read by 30+ tools, 60,000+ repositories. Tervin should read it, show it in
  the Bridge panel as the instructions in force, and pass it to runtimes that do not read it
  themselves.
- **`CLAUDE.md`, `.cursorrules`, `.github/copilot-instructions.md`**: the same, reported
  rather than merged, so a user can see which files an agent is actually obeying.
- **Existing MCP config** (`.mcp.json`, `.claude.json`, `.codex/`): Warp auto-discovers these.
  Tervin has its own `mcpServers` file and should adopt whatever is already there.

*Exit criteria:* the Bridge panel lists every instruction source and MCP server in force, per
runtime, with the file it came from. A file Tervin found but a runtime will not read is shown
as such, because that distinction is exactly the kind of thing that silently wastes an hour.

### 4.3 Handing work between local, cloud and web agents
`P1, and the centre of this specification.` The industry lifecycle is
*ticket → cloud sandbox → autonomous edit → PR → human review*, and the handoff is the
workflow. Tervin already has `ContextBundle`, which is the right primitive and is currently
only used locally.

**The Tervin Context Bundle becomes a portable artefact.**

*Specification:*

1. **A defined, versioned, inspectable format.** JSON with a schema, containing: the task, the
   Blocks that matter with their exit codes and diagnostics, the diff so far, the files
   touched, the instruction sources in force, and an explicit list of **what was left out**.
   The existing implementation already always states omissions; keep that rule.
2. **Redaction before it leaves the machine, shown before it goes.** A bundle is the moment
   context crosses a trust boundary. The user sees exactly what is included, with anything
   matching a secret pattern excluded by default and the exclusion listed.
3. **Export targets, each honest about fidelity:**
   - Another local runtime: full fidelity, already works.
   - A cloud agent (Codex Cloud, Cursor cloud, Copilot coding agent, Devin, Jules): via its
     own API or by opening a PR with the bundle as the task description. Fidelity is partial
     and the UI says which fields survived.
   - A web chat: a formatted prompt on the clipboard. Lowest fidelity, clearly labelled.
4. **Import, which nobody does well.** A cloud agent finishes and opens a PR. Tervin should
   pull that branch into a worktree, reconstruct a Thread from the PR's commits and checks, and
   let a local agent continue with the cloud agent's work as context. **This is the missing
   half of every handoff story in 2026** and the single most differentiating item here.
5. **Round-trip attribution.** A commit made by a cloud agent from a Tervin bundle carries a
   trailer identifying the bundle. Review shows who did what across the whole chain: human,
   local agent, cloud agent.

*Why this is the right bet:* every vendor is building handoff *into their own product*. Nobody
is building the neutral carrier, because nobody else's incentive points there. A terminal is
the natural place for it, since it is the one tool that is already talking to all of them.

### 4.4 A local Warp Drive, without the cloud
`P2.` Warp Drive's useful half is a shared, version-controlled store of workflows, environment
profiles, MCP servers and runbooks. Its other half is a proprietary cloud.

*Specification:* `.tervin/` in the repository, committed like any other config, holding saved
commands (already built), environment profiles, MCP servers, Tervin Rules, and layouts. A team
shares it through git, which they already trust with everything else. Nothing is uploaded and
no account exists.

**Notebooks**, a command, its output and prose, export to Markdown with the Blocks embedded,
so it lands in a PR or a wiki rather than in a vendor's cloud.

### 4.5 History that follows you, without a server
`P2.` atuin's advantage over Tervin is encrypted cross-machine sync. Tervin's advantage is that
its history includes output, diagnostics and test results.

*Specification:* export and import the Blocks and prompt store as a signed, encrypted archive,
synchronised by whatever the user already uses: a private git repository, a synced folder,
`scp`. No Tervin server, because a Tervin server would need an account, and an account is the
thing that stops people trying it.

### 4.6 Local and open models as first-class, not as a fallback
`P1.` Currently local models are `Tier::Conversational`: they answer, they cannot act. That is
accurate for a bare endpoint, and it undersells what is possible in 2026.

*Specification:*
- **Tool calling for local models that support it.** An OpenAI-compatible endpoint advertising
  tool support gets the same tool loop as a hosted model, promoting it out of Conversational.
  Where an endpoint claims tool support and then misuses it, report that rather than retrying
  silently.
- **Tervin Rules as the gate.** This is where Tervin is strongest: a local model with no
  vendor safety layer is exactly the case where a real pre-execution gate matters most, and
  Tervin's is real.
- **Honest capability probing.** Ask the endpoint what it supports, record what it actually
  did, and upgrade the capability only on evidence: the same rule the Claude Code hook gate
  already follows.
- **Model routing per task.** A local model for "what does this error mean", a frontier model
  for a refactor, chosen by policy and shown in the Thread.

*Why it matters:* a terminal that works fully offline, against models you host, with a
permission gate you control, is a genuinely different product from every cloud-billed
competitor. Nobody in this comparison offers it.

### 4.7 The agent-facing terminal
`P2.` Tervin knows things no agent can see, which commands failed, what the diagnostics were,
which files changed, what the tests said.

*Specification:* expose Tervin's own store as an **MCP server**, so any agent, in Tervin, in
an editor, or in the cloud, can ask "what failed in the last hour", "what is this project's
test command", "show me the diff so far". Read-only, project-scoped, and off by default with a
visible indicator when on. This turns Tervin from a place agents run into a source of truth
they consult.

---

## 5. What Tervin should refuse to build

Naming these matters as much as the roadmap, because each is a plausible request.

- **Team accounts, SSO, seats, a hosted backend.** Every one requires a server holding user
  data and an organisation to run it. Tervin's proposition is that it works against your own
  subscriptions with no account. `.tervin/` in git covers real team needs.
- **A proprietary agent.** The ecosystem has enough. Tervin's value is being the best host for
  the ones that exist, including ones written after it.
- **Answering `allow` on a runtime's behalf.** Already refused, and permanently.
- **Storing credentials.** Already refused: the SSH work surfaces whether a key is loaded
  rather than holding a passphrase. The same applies to API keys: reference the keychain, do
  not become one.
- **A latency number that is not a latency measurement.** Already refused.
- **Kitty keyboard and graphics protocols**, unless the renderer is replaced. Not possible, so
  not claimed.
- **Apple Developer ID signing and notarisation.** $99 a year, forever, tied to one person's
  Apple ID, to remove a one-time dialog from the two least-recommended install routes, when
  five routes have no dialog at all because macOS applies quarantine in the downloading
  application and `curl` does not set it. The money buys polish on the path users are told
  not to take. A published `SHA256SUMS.txt` that the installer *requires* rather than warns
  about covers the realistic threat, a substituted download. It does not cover a verified
  third-party identity, which is a real thing to be missing and is said plainly in
  `SECURITY.md` rather than glossed.
- **Instant replay**, probably. Blocks answer the question people actually ask.

---

## 6. Ordered plan

**Now: credibility.** Linux and Windows in CI (§3.1). Read `AGENTS.md` and existing MCP
config (§4.2). CLI completion via the shell (§3.2).

**Next: the two structural gaps.** Detach and reattach (§3.8). Parallel Threads with worktree
isolation (§4.1). These are the largest pieces of work here and the two that change what
Tervin *is*.

**Then: the differentiator.** The portable Context Bundle, both directions, with import from
a cloud agent's PR being the part nobody else has (§4.3). Local models as real actors (§4.6).

**Alongside: parity polish.** Blocks over SSH (§3.3), triggers (§3.5), tmux control mode
(§3.4), `.tervin/` as a committed workspace (§4.4), vi-mode scrollback and broadcast input
(§3.6).

**Later, or never.** Floating panes, layout artefacts, history sync, the MCP server, instant
replay. Each is defensible; none changes the argument for using Tervin.

---

## Sources

Researched August 2026.

- [Warp Guide 2026: Agent Mode, MCP, Open Source & Deployments, DeployHQ](https://www.deployhq.com/guides/warp)
- [Warp AI Terminal 2026: Agentic CLI Workflows Guide, Digital Applied](https://www.digitalapplied.com/blog/warp-ai-terminal-agentic-cli-workflows-guide)
- [Choosing a terminal emulator in 2026: Ghostty, iTerm2, Kitty, Alacritty, WezTerm, Luminoid](https://blog.luminoid.dev/Terminal-Emulator-Comparison-2026/)
- [Best Terminal for Mac in 2026: Ghostty, Kitty, WezTerm, Alacritty, Warp, DEV](https://dev.to/vibehackers/best-terminal-for-mac-in-2026-ghostty-kitty-wezterm-alacritty-warp-more-4pe6)
- [Modern Terminal Emulators 2026: Ghostty, WezTerm, Alacritty, Calmops](https://calmops.com/tools/modern-terminal-emulators-2026-ghostty-wezterm-alacritty/)
- [iTerm2 tmux Integration documentation](https://iterm2.com/documentation-tmux-integration.html)
- [iTerm2 Shell Integration documentation](https://iterm2.com/documentation-shell-integration.html)
- [kitty: Comprehensive keyboard handling in terminals](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
- [kitty: Shell integration](https://sw.kovidgoyal.net/kitty/shell-integration/)
- [Zellij vs tmux: Honest Comparison, Petronella](https://petronellatech.com/blog/zellij-terminal-multiplexer-guide-2026)
- [Terminal Multiplexers: tmux vs Zellij, dasroot](https://dasroot.net/posts/2026/02/terminal-multiplexers-tmux-vs-zellij-comparison/)
- [Zed, Agent Client Protocol](https://zed.dev/acp)
- [Zed: External Agents](https://zed.dev/docs/ai/external-agents)
- [Codex CLI in Zed 1.0: Parallel Agents, ACP Integration](https://codex.danielvaughan.com/2026/05/05/codex-cli-in-zed-parallel-agents-acp-integration-ide-workflows/)
- [Agent Client Protocol (ACP) Explained: ACP vs MCP, Morph](https://www.morphllm.com/agent-client-protocol)
- [AGENTS.md Spec (2026), Morph](https://www.morphllm.com/agents-md-guide)
- [Standardize project context with AGENTS.md and Agent Skills, Red Hat Developer](https://developers.redhat.com/articles/2026/07/27/standardize-project-context-agentsmd-and-agent-skills)
- [Atuin: Magical Shell History with Sync, Search, and Statistics](https://kx.cloudingenium.com/en/atuin-shell-history-sync-search-statistics-guide/)
- [fish vs zsh vs nushell in 2026, SumGuy](https://sumguy.com/fish-vs-zsh-vs-nushell-2026/)
- [Devin vs Claude Code vs Codex 2026: 8 Agents Tested, TECHSY](https://techsy.io/en/blog/background-coding-agents-compared)
- [AI Coding Agents in 2026: A Practical Roadmap, CodePick](https://codepick.dev/en/guides/ai-coding-agents-2026-roadmap/)
- [Custom agents in VS Code](https://code.visualstudio.com/docs/agent-customization/custom-agents)
