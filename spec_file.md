# Tervin — Build Specification Package

This document contains the complete Tervin product specification and the editable SVG logo source.

- **Product name:** Tervin
- **CLI binary:** `tervin`
- **Category:** Agent-native terminal workspace
- **Primary tagline:** *The agent-native terminal workspace.*

---

# Tervin Logo — Editable SVG

Save this section as `tervin-logo.svg` if you need the standalone asset.

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" role="img" aria-labelledby="title desc">
  <title id="title">Tervin logo</title>
  <desc id="desc">Two graphite tapered halves around a muted teal central seam</desc>
  <rect width="128" height="128" rx="24" fill="#141514"/>
  <path d="M39 22h20v84H39l11-42z" fill="#E5E8E5"/>
  <path d="M89 22H69v84h20L78 64z" fill="#AEB5B1"/>
  <path d="M61 22h6v84h-6z" fill="#68AEA5"/>
  <path d="M61 56h6v16h-6z" fill="#8CC9C1"/>
</svg>
```

---

# Tervin — Product, Brand & Build Specification

## Canonical identity

**Product name:** Tervin  
**CLI binary:** `tervin`  
**Category:** Agent-native terminal workspace  
**Primary tagline:** *The agent-native terminal workspace.*

Tervin is a fast, terminal-first development workspace for shells, coding agents, Git state, diffs, tests, logs, tasks, and reviewable permissions.

> **Tervin is not a chat application with a terminal widget.** It is a correct, fast terminal whose development workflow becomes more legible when agents participate.

Tervin is deliberately agent-agnostic. Claude Code is a first-class launch integration, but Tervin must support Codex-compatible runtimes, Gemini CLI, Aider, OpenCode-style tools, local-model agents, generic agent CLIs, MCP tools, and future runtimes.

## Naming rules

- Use **Tervin** in customer-facing text, documentation, UI headings, and wordmarks.
- Use lowercase `tervin` only for the command-line executable, configuration keys, file paths, URLs, and code identifiers.
- Do not use “Tervin AI”, “Tervin Terminal”, “Tervin IDE”, “Tervin Code”, “Tervin OS”, or “Tervin Labs” as public product names.
- Use clear product vocabulary consistently.

| Name | Meaning |
|---|---|
| **Tervin Blocks** | Structured command/output units |
| **Tervin Threads** | Provider-independent coding-agent tasks |
| **Tervin Deck** | Overview of active agents and background work |
| **Tervin Review** | Diff, test, diagnostic, and approval workspace |
| **Tervin Rules** | Cross-agent execution and permission policy |
| **Tervin Bridge** | Agent adapters, MCP tools, extensions, and runtime integrations |
| **Tervin Workspaces** | Persistent project, session, and pane arrangements |
| **Tervin Relay** | Future opt-in remote-session and collaboration capability |

## CLI language

```bash
tervin .
tervin agent
tervin review
tervin connect staging
tervin workspace open api
```

## Voice

Tervin should sound precise, candid, calm, and technical.

Use concrete language:

- “Review 3 changed files”
- “Agent is waiting for approval”
- “Tests passed”
- “Reconnect to staging”
- “Run this in a new pane”
- “This command modified 4 files”

Avoid inflated language:

- “Magical”
- “Revolutionary”
- “Supercharge”
- “All-in-one”
- “AI-powered everything”
- “Autonomous future”
- “Unlock the power of”

Tervin should never imply certainty where an agent is uncertain. Show the plan, command, files, diffs, test result, output, and evidence.

---

# Brand system

## Positioning

Tervin is the development cockpit for people who live in the terminal and increasingly work alongside coding agents.

It makes terminal work easier to understand, search, resume, review, and control without turning the terminal into a bloated IDE or generic chat dashboard.

## Logo concept

The Tervin mark is an abstract **central spine**.

It should quietly suggest:

- A terminal cursor
- A pane divider
- A stable guide through complex work
- A point of control where independent streams become inspectable together

The mark consists of two quiet tapered forms facing a narrow muted-teal seam. The seam represents Tervin’s role: shells, agents, logs, diffs, tests, and approvals remain distinct, but become understandable in one workspace.

The mark must not be:

- A literal prompt symbol
- A robot
- Generic AI sparkles
- Code brackets
- A ship or nautical icon
- A pylon
- A navigation pin
- A generic arrow
- A neon or cyberpunk graphic

## Logo rules

- Use the supplied SVG as the authoritative starting asset.
- The mark must remain identifiable at 16px and balanced at 128px or larger.
- Keep the mark vertical, compact, geometric, and quiet.
- Default application icon: centered mark on warm graphite `#141514`.
- Dark-surface variant: off-white and soft-graphite halves with teal seam.
- Light-surface variant: graphite halves with teal seam.
- Monochrome fallback: graphite or off-white only.
- Clear space around the mark is at least the width of the central teal seam.
- Never stretch, rotate, bevel, outline, add a drop shadow, place on noisy imagery, or add gradients.
- Do not put the mark inside a coloured circle.
- Do not use the mark as a decorative repeating pattern.

## Wordmark

Set “Tervin” in a restrained neutral sans-serif:

- Preferred fonts: Geist, Inter, or Satoshi
- Weight: 600 to 700
- Letter spacing: normal or slightly tight
- Capitalisation: title case, `Tervin`
- All caps only for tiny utility labels, never the primary wordmark

Do not use a sci-fi, cyberpunk, mono-display, or highly geometric display font for the brand wordmark.

## Brand palette

| Token | Hex | Use |
|---|---:|---|
| `--tervin-graphite-950` | `#141514` | Main app background and application icon |
| `--tervin-graphite-900` | `#1B1D1C` | Panels and terminal surfaces |
| `--tervin-graphite-800` | `#232624` | Raised surfaces |
| `--tervin-line` | `#323634` | Dividers and subtle boundaries |
| `--tervin-ink` | `#E5E8E5` | Primary text and light logo half |
| `--tervin-muted` | `#909894` | Secondary information |
| `--tervin-teal` | `#68AEA5` | Focus, primary action, logo seam |
| `--tervin-green` | `#85BC7E` | Passing command or test state |
| `--tervin-amber` | `#D5AB68` | Warning, plan mode, pending review |
| `--tervin-red` | `#D77D79` | Failure and destructive-action warnings |

### Color rules

- Graphite and off-white dominate the product.
- Teal indicates focus, intentional action, selection, and the brand’s central seam.
- Green, amber, and red are semantic state colours only.
- Do not use colour merely for visual decoration.
- No gradients.
- No glowing orbs.
- No purple-blue “AI” styling.
- No glassmorphism.
- No neon accents.

---

# Product principles

1. **Terminal correctness first.** Existing shells, SSH, tmux/zellij, Neovim, interactive CLIs, Unicode, mouse modes, bracketed paste, and ANSI/VT behaviour must work reliably.

2. **Terminal first, not terminal only.** The terminal remains visually central. Code review, Git state, agents, files, diagnostics, and tasks are available through progressive disclosure.

3. **Agent agnostic by design.** Claude Code is a first-class launch integration, but no core data model, UI surface, or permission policy assumes one model vendor or agent runtime.

4. **Every action is inspectable.** Users can see plans, files read, commands run, tool calls, diffs, tests, costs when available, and failures.

5. **Safe by default.** Agent actions are governed by Tervin Rules. Dangerous changes require contextual approval.

6. **Keyboard first.** Every essential action is available through shortcuts and a command palette. Mouse support is additive.

7. **Calm density.** Show enough information to move quickly without turning every output item into a large card or every session into a giant chat thread.

8. **Local-first privacy.** Tervin does not send scrollback, files, commands, environment data, or credentials to a provider unless a user explicitly runs an agent or integration that needs it.

---

# Primary users

- Developers who spend most of their day in a terminal.
- Developers using Claude Code, Codex, Gemini CLI, Aider, OpenCode, or local-model agents.
- Developers moving between repositories, branches, SSH hosts, services, tests, and long-running tasks.
- Teams that need reviewable, permission-aware agent-driven changes.
- Developers who want a superior TUI without surrendering terminal speed and control.

---

# Information architecture

## Main workspace

The default desktop application contains five zones.

### Top command bar

Includes:

- Project and workspace selector
- Git branch and dirty-state indicator
- Global command palette
- Universal search
- Connection state
- Active agent count
- Notifications
- Settings

### Activity rail

Includes:

- Workspace
- Files
- Git
- Threads
- Tasks
- History
- Connections
- Bridge
- Settings

The rail has a compact icon-only mode.

### Terminal canvas

Includes:

- Tabs
- Arbitrary horizontal and vertical splits
- Structured Tervin Blocks by default
- Continuous terminal mode per tab
- Shell sessions
- Managed agent sessions
- Log streams
- Test runners
- Remote terminals

### Context inspector

A collapsible right panel with:

- Thread
- Review
- Files
- Git
- Diagnostics
- Details

Users can pin a Block, Thread, file, diff, test, task, or diagnostics group to the inspector.

### Bottom status rail

Includes:

- Shell and current working directory
- Host or SSH connection
- Git branch and dirty state
- Active agent mode and model where known
- Task progress
- Token and cost data where available
- Remote latency and reconnect state

## Default layout modes

### Terminal First

This is the first-run default.

- Terminal occupies nearly the full screen.
- Inspector is hidden.
- Git, agent, diff, file, or diagnostic context opens as a temporary right drawer.
- No permanently visible chat panel.
- The terminal remains the visual centre.

### Mission Control

For multi-agent and long-running work.

- Two terminal panes
- Thread inspector
- Compact task timeline
- Tervin Deck summary
- Background activity visible without taking over the workspace

### Review Desk

For reviewing agent-generated changes.

- Changed-file tree
- Unified or side-by-side diffs
- Agent plan and activity timeline
- Test terminal
- Approval, revert, and open-in-editor actions

### Debug Bench

For investigating build/runtime failures.

- Live logs
- Interactive shell
- Diagnostics grouped by severity
- Selected Thread context
- Linked stack traces, ports, paths, and files

---

# Terminal core

## Required baseline

Tervin must support:

- Local shells: zsh, bash, fish, PowerShell, nushell, custom commands
- Tabs, windows, and arbitrary horizontal/vertical splits
- Resize, swap, zoom, duplicate, close, and detach pane actions
- Configurable persistent disk-backed scrollback
- Fast text and regular-expression search
- Copy/paste
- Multi-line paste safety
- Selection expansion
- Optional copy-on-select
- Hyperlinks and smart selection for paths, URLs, ports, issue IDs, commits, emails, and stack traces
- Unicode, CJK text, emoji, ligatures, true colour, font fallback, underline variants, cursor styles, and mouse reporting
- Light/dark themes
- High contrast mode
- User-selected fonts, font sizes, line height, cursor style, and keymaps
- Config reload
- Accessibility and screen-reader semantics
- Reduced-motion support

## Protocol and integration targets

Support or design for:

- OSC 7 current-working-directory reporting
- OSC 8 hyperlinks
- OSC 52 clipboard support with security policy
- Bracketed paste
- Synchronized rendering
- Kitty keyboard protocol where feasible
- Kitty graphics protocol where feasible
- iTerm2 image protocol
- Sixel where feasible
- Focus reporting
- Mouse reporting
- Terminal colour-scheme notifications

## Sessions and connections

Tervin provides:

- SSH manager using `~/.ssh/config`
- Connection profiles
- Host labels
- Latency and connection states
- Secure platform keychain integration
- Reconnection states
- tmux and zellij attachment
- Local and remote profiles
- WSL support on Windows
- Serial support for embedded workflows
- Session restore where safe

Never expose secret environment values. Indicate their presence only when relevant.

## Shell integration

Optional shell scripts for zsh, bash, fish, and PowerShell report:

- Prompt boundaries
- Submitted commands
- Exit status
- Duration
- Current working directory
- Host
- Git branch and repository
- Running state

Tervin remains functional without shell integration.

---

# Tervin Blocks

Every submitted command and its related output becomes a **Tervin Block**.

A Block contains:

- Command
- Timestamp
- Current working directory
- Host/session
- Start/end time
- Duration
- Exit code
- Standard output and error streams
- Git context
- Parsed paths, URLs, ports, diagnostics, warnings, and errors
- Tags
- Notes
- Bookmarks
- Exported artifacts

## Block actions

- Collapse and expand
- Copy command
- Copy output
- Copy both
- Re-run
- Edit and re-run
- Run in a new pane
- Bookmark
- Tag
- Annotate
- Pin to inspector
- Export Markdown, plain text, ANSI text, or image
- Filter by status, project, cwd, host, tag, command, date, or output
- Open discovered paths and URLs
- Send selected output to an agent as explicit context
- Save successful commands as parameterized workflows

## Block visual behaviour

- Blocks are quiet by default.
- Use type, spacing, and a small status marker rather than large cards.
- Preserve terminal selection and output fidelity.
- Failed, running, and bookmarked Blocks are easy to locate but do not flood the interface with colour.
- Long output displays a compact summary with expandable raw text.
- The raw terminal output is always available.

---

# Agent-native workspace

## Agent runtime model

Tervin hosts any agent through a stable `AgentRuntime` interface.

Initial integration targets:

- Claude Code
- Codex-compatible runtimes
- Gemini CLI
- Aider
- OpenCode-compatible tools
- Generic local or remote CLI agents
- Local-model workflows
- MCP-based integrations

Future tools may arrive through structured subprocesses, JSON Lines, JSON-RPC, plugins, APIs, or MCP-based integrations.

## AgentRuntime interface

```text
discover()
launch(config)
resume(session_id)
send_input(content, attachments)
interrupt()
capabilities()
event_stream()
permissions()
session_metadata()
diagnostics()
```

## Integration tiers

### Tier 1 — Structured

For agents with documented APIs, SDKs, JSON events, or structured output.

Capabilities:

- Rich timeline
- Plan mode
- Resume support
- Context attachment
- Permission bridge
- Tool-call visibility
- Model and cost metadata where available
- Links to Blocks, files, diffs, diagnostics, and tests

### Tier 2 — Enhanced CLI

For interactive terminal CLIs with limited machine-readable output.

Capabilities:

- Managed PTY
- Terminal output remains authoritative
- Extract paths, diffs, commands, URLs, warnings, and errors where reliable
- Attach Tervin context
- Surface observed permissions
- Keep the native agent interaction intact

### Tier 3 — Generic agent terminal

For arbitrary managed commands.

Capabilities:

- Run any agent command in a managed terminal pane
- Full terminal fidelity
- Manual task title and status
- Capture Blocks, Git delta, output, and artifacts
- No dedicated adapter required

## Tervin Thread

A Tervin Thread is provider-independent.

Each Thread stores:

- Agent/runtime identity
- Adapter tier
- Model where known
- Project
- Current working directory
- Git branch
- Worktree
- Host
- Task title
- Parent/subtask relationships where supported
- Linked terminal pane
- Timeline
- Permissions
- Blocks
- Files
- Diffs
- Diagnostics
- Tests
- Cost/token data where available
- Resume ID
- Audit history

### Thread states

```text
Idle
Starting
Awaiting input
Understanding
Planning
Reading
Editing
Executing
Testing
Waiting for permission
Waiting for external tool
Review required
Completed
Failed
Interrupted
Disconnected
Unknown
```

`Unknown` is valid for generic agent tools when Tervin cannot reliably infer the internal state.

## Unified event stream

Normalize runtime events into an append-only event stream:

```text
thread.started
user.prompted
context.attached
agent.message
plan.proposed
plan.approved
tool.requested
tool.completed
command.proposed
command.started
command.output
command.completed
file.read
file.changed
patch.proposed
patch.applied
git.changed
test.started
test.completed
diagnostic.detected
permission.requested
permission.granted
permission.denied
artifact.created
cost.updated
thread.completed
thread.failed
```

Each event includes:

- Event ID
- Thread ID
- Timestamp
- Agent identity
- Project and cwd
- Concise human-readable summary
- Safe raw-payload reference
- Links to relevant Blocks, files, diffs, diagnostics, tests, or artifacts

## Composer

The Thread composer supports:

- Multi-line input
- Prompt history
- Slash commands
- `@` path autocomplete
- Image paste and drag/drop where runtime supports it
- Selected Block and diff attachment
- Explicit project-context chips
- Agent/model/mode selection
- Context-budget indicator where available
- Keyboard-first send and plan actions

## Capability-aware UI

Do not fake feature parity between agents.

Tervin must show what each runtime can actually do:

- Plan mode
- Resume
- Tool events
- File edits
- Native permission bridge
- MCP support
- Hooks
- Subagents
- Image input
- Cost reporting
- Model selection
- Remote execution

Unsupported controls are absent or clearly disabled with an explanation.

## Project instructions

Discover and explain project-level agent instructions:

- `AGENTS.md`
- `CLAUDE.md`
- `GEMINI.md`
- `.cursorrules`
- `.github/copilot-instructions.md`
- Agent configuration files
- Build/test documentation

Show:

- Source
- Scope
- Precedence
- Consuming agents
- Conflicts

Never silently merge conflicting instructions.

## Tervin Bridge

Tervin Bridge is the neutral integration centre for:

- MCP servers
- Agent adapters
- Extensions
- Tools
- Auth providers
- Runtime hooks
- Project-level integrations

For each Bridge integration, show:

- Identity
- State
- Authentication requirement
- Scope
- Permission level
- Logs
- Failures
- Workspace/task enablement

MCP is supported, but never the only integration path.

---

# Tervin Rules

Tervin owns provider-neutral policy, approval, and auditability.

## Approval requests

Every request shows:

- Exact command, file operation, or tool action
- Working directory
- Host
- Reason
- Risk level
- Expected side effects
- Whether it affects Git, credentials, network, production, or destructive data

Approval options:

- Approve once
- Approve for this task
- Approve for this workspace
- Deny
- Edit before run
- Add policy rule

## Always require explicit confirmation

- Destructive deletion
- `sudo`
- Force push
- `git reset --hard`
- `git clean`
- Rebase operations with destructive effect
- Destructive database commands
- Production deployment
- Credential access or exfiltration
- Unknown network uploads
- Package publishing
- SSH key changes
- Out-of-scope process termination

## Policy transparency

- Clearly show when a generic agent action cannot be intercepted.
- Preserve an audit log of requested, allowed, denied, and executed actions.
- Distinguish Tervin-controlled approvals from provider-native approvals.
- Never claim that an action is sandboxed when it is not.

---

# Git, review, diagnostics, and tasks

## Tervin Review

Tervin Review supports:

- Working-tree diffs
- Per-Thread diffs
- Staged and unstaged diffs
- Worktree diffs
- Unified and side-by-side views
- Syntax highlighting
- Changed-file tree
- Hunk-level accept/revert where safe
- Open-in-editor
- Stale-diff warnings
- Links from timeline events to exact diff hunks
- Links from test output to changed files

Rules:

- Never auto-commit by default.
- Clearly display external or agent-created commits.
- Always show an understandable rollback path.

## Git panel

Includes:

- Status
- Branches
- Worktrees
- Staged/unstaged changes
- Commit composer and history
- Fetch, pull, push, sync
- Conflict handling
- Provider and pull-request link detection

## Diagnostics

Group and link:

- Compiler errors
- Test failures
- Linter warnings
- Stack traces
- Build output
- Ports and local URLs
- Background process state

Every diagnostic links back to its originating Block and source location where possible.

## Workflows and tasks

Tervin supports:

- Saved commands
- Parameterized workflows
- Pinned commands
- Repeat history
- Project templates
- Background task status
- Task output as Blocks
- Optional dependencies and preflight checks

---

# Search, palette, and keyboard

## Global command palette

Fuzzy-search:

- Actions
- Keybindings
- Settings
- Tabs
- Panes
- Workspaces
- SSH profiles
- Workflows
- Command history
- Files
- Git branches and commits
- Threads
- Tasks
- Help

Requirements:

- Immediate response
- Keyboard navigation
- Context-aware ranking
- Clear categories
- Useful empty states

## Universal search

Search:

- Current scrollback
- Persisted Blocks
- Commands
- Output
- Errors
- Files
- Agent prompts and summaries
- Tasks
- Commits
- Sessions

Filters:

- Project
- Branch
- Host
- Date
- Status
- Agent
- Tag
- Content type

## Essential keyboard actions

Provide configurable platform-native shortcuts for:

- New tab
- New split
- Focus pane
- Resize pane
- Zoom pane
- Swap pane
- Open palette
- Open search
- Toggle inspector
- Toggle activity rail
- Navigate Blocks
- Copy/re-run Block
- Open agent composer
- Change agent mode
- Approve/deny request
- Stop Thread
- Open keybinding reference

---

# Design system

## Visual direction

Tervin should feel like a precise development instrument:

- Dark-first
- Dense but breathable
- Warm graphite surfaces
- Crisp monospace output
- Restrained teal focus
- Very little decoration
- Strong hierarchy through spacing and typography

Avoid:

- Gradients
- Glassmorphism
- Purple/blue AI styling
- Glowing orbs
- Thick coloured card borders
- Huge rounded cards
- Oversized chat bubbles
- Icons inside decorative coloured circles
- Generic dashboard-card clutter

## Typography

- UI and wordmark: Geist, Inter, or Satoshi
- Terminal, code, and diffs: Berkeley Mono, JetBrains Mono, Iosevka, Maple Mono, or user-selected monospace
- Body text minimum: 14px in the desktop interface
- Metadata minimum: 12px
- Use tabular figures for time, costs, ports, durations, and status values

## Layout

- Use a 4px/8px spacing system.
- Keep top chrome compact.
- Keep the terminal visually central.
- Inspector collapses completely.
- Activity rail supports icon-only compact mode.
- AI-specific UI should not consume more than 30% of desktop width by default.
- At smaller widths, collapse panes rather than shrinking controls into unusable layouts.
- Avoid permanent fixed elements that block output on small screens.

## Motion

Use motion only for orientation and feedback:

- Pane opening/closing
- Inspector transitions
- Thread/task state changes
- Small list reordering

Target 150–220ms transitions. Respect reduced-motion settings. Do not use ornamental looping animations.

---

# Technical architecture

## Preferred implementation

- Rust terminal core
- Native windowing and GPU rendering
- Separate UI/application state
- Event bus for terminal, Git, task, file, permission, and agent activity
- Local workspace database
- Local-first storage
- Platform secure credential storage

## Core modules

```text
terminal-core       PTY, VT parser, screen buffers, renderer
session-manager     Local, SSH, tmux, zellij, serial sessions
workspace-manager   Projects, layouts, restore
block-engine        Grouping, metadata, indexing, replay
shell-integration   Command/cwd/status protocol
agent-runtime       Adapters, capabilities, event normalization
rules-engine        Policy, approvals, audit
git-service         Status, diffs, branches, worktrees
search-service      Global indexing and search
bridge              MCP, tools, extensions
ui                  Panes, overlays, keymaps, themes, accessibility
```

## Privacy model

- Local-first by default
- No telemetry by default
- No cloud sharing by default
- Opt-in collaboration only
- Secret redaction in exports
- Secure credential storage
- Never transmit code, scrollback, files, or environment values unless an explicit agent task or integration requires it

---

# Delivery plan

## MVP

Build in this order:

1. Local high-quality terminal: tabs, splits, search, shell integration, themes, and keymaps
2. Tervin Blocks: grouping, metadata, bookmarks, filters, re-run, export
3. Workspaces and restore
4. Claude Code as a Tier 1 or Tier 2 integration
5. Threads, composer, timeline, permission prompts, and stop control
6. Tervin Review linked to Git diffs and test Blocks
7. Git status and changed-file panel
8. Command palette
9. SSH profiles
10. Accessibility, settings, and light/dark themes

## Next releases

- Codex, Gemini, Aider, and generic agent adapters
- Tervin Deck
- Background task orchestration
- Worktrees
- Stronger remote persistence
- Graphics protocol support
- Parser SDK
- Saved workflows
- tmux/zellij depth
- Tervin Bridge SDK

## Explicitly defer

- Cloud sync as a default
- Marketplace
- Real-time multiplayer
- Embedded full code editor
- Custom LLM backend
- Autonomous production deployment without strong policy controls

---

# Acceptance criteria

1. A developer can open a project and begin real shell work within seconds.
2. Any command is easy to find, copy, re-run, filter, tag, export, and review.
3. A user can run a coding agent, inspect its plan, approve scoped actions, and review exact file changes and test results.
4. Multiple heterogeneous agent tasks can run in parallel while users understand their purpose, state, current action, permissions, and output.
5. Common workflows are fully keyboard accessible.
6. Terminal-heavy work is quicker and less cluttered than in a conventional IDE.
7. The app remains responsive under compilation, log streaming, tests, large scrollback, and streaming agent output.
8. Tervin remains calm and terminal-first rather than becoming a generic chat dashboard.
9. Essential workspace context restores after restart.
10. Protected actions never execute without clear, reviewable authorization.

---

# Final implementation instruction

Build Tervin as a dependable terminal with an exceptional control surface for modern development.

Prioritise:

- Terminal correctness
- Fast rendering
- Keyboard-first operation
- Information hierarchy
- Cross-agent compatibility
- Reviewable work
- Clear permission boundaries
- Local-first privacy

Do not prioritise feature count, decorative polish, generic AI branding, or autonomous behaviour over user understanding and control.