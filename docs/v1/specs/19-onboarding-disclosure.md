# Spec 19 — Onboarding & disclosure levels

Designed in full in the mockup. **Not built at all.**

## Context

`spec_file.md`'s first acceptance criterion is *"A developer can open a project and begin
real shell work within seconds."* There is currently no first-run experience whatsoever —
the app opens to a terminal with no shell integration explanation, no project chosen, and
no indication that anything else exists.

The mockup designs a three-step flow and a disclosure-level system that `DESIGN.md` also
references (*"In Guided mode add exactly one plain-English sentence under a command: no
jargon, no second sentence."*) and which the code does not have.

## The designed flow, verbatim from the mockup

**Step 1 — "Where do you work today?"**
*"Tervin brings your setup across. This takes about a minute."*
Sources: the Claude desktop app, VS Code, a terminal (iTerm2/Terminal.app).

**Step 2 — "Here is what Tervin found"**
*"Nothing has been changed yet. Uncheck anything you would rather keep separate."*

Per source, the mockup's actual copy:
- *desktop:* the Claude account already signed in ("No API key, no billing setup"); two
  Claude installs found and **kept apart**; six project folders added as workspaces;
  *"Nothing is uploaded — Tervin reads these folders on your Mac. Files only leave when
  you ask Claude to work on them."*
- *vscode:* theme, font, size and tab width copied; ⌘P becomes the palette, ⌘⇧P stays the
  profile switcher, ⌃\` opens a terminal; recent folders added with their git branches;
  *"Open in editor" hands files back to VS Code.*
- *terminal:* shell and prompt detected (zsh 5.9, Oh My Zsh, powerlevel10k, *"rendered
  exactly as it is today"*); SSH hosts from `~/.ssh/config`; tmux and zellij attachable;
  iTerm2 and Terminal.app keybindings mapped.

Plus a setup checklist: shell integration installed, project indexed, agents found on this
machine each with its permission tier.

**Step 3 — "How much do you want to see?"**
*"You can change this later in Settings — it only affects how much Tervin explains."*
Guided · Standard · Expert.

Closing line: **"Nothing leaves this machine until you run an agent."**

## Slices

### 19.1 — The three-step flow
Build it. Skippable at any point, re-runnable from Settings, and never blocking — a user
who hits Escape lands in a working terminal.

*Exit:* first launch shows step 1; Escape at any step leaves a usable app; Settings can
re-run it.

### 19.2 — Import: detection
Detect what is actually there rather than claiming it. Claude installs (`profile.rs`
already discovers these and keeps accounts apart), VS Code settings, iTerm2 and
Terminal.app preferences, shell and prompt, `~/.ssh/config` hosts, tmux and zellij
sessions, agents on the machine.

**Nothing may be claimed that was not observed.** "6 added" must be six real folders. A
source that is absent is not shown at all rather than shown as zero.

*Exit:* a machine with no VS Code does not offer the VS Code path. Every count is real.

### 19.3 — Import: consent and application
*"Nothing has been changed yet. Uncheck anything you would rather keep separate."* Nothing
is applied until Continue, everything is individually declinable, and a summary says what
was changed and where.

The two Claude installs case is the important one: **kept apart by default**, because
`profile.rs:40-50` already scrubs account-selecting variables precisely so an ambient
`CLAUDE_CONFIG_DIR` cannot silently run the work account under a profile labelled
"personal". Onboarding must not undo that.

*Exit:* declining everything changes nothing on disk. Accepting reports what it wrote.

### 19.4 — Disclosure levels
Guided, Standard, Expert. Per `DESIGN.md`: in Guided mode add *exactly one* plain-English
sentence under a command — no jargon, no second sentence. That constraint is the whole
design; a Guided mode that becomes chatty defeats it.

Expert removes explanatory text and confirmations that are not safety-bearing. **It does
not remove safety confirmations** — a level that quietly disables checks is
`bypassPermissions` under another name, which the product refuses.

The level shows in the status rail (`{{levelLabel}} mode`) and is changeable in Settings.

*Exit:* Guided adds one sentence per command and never two. Expert removes no safety
confirmation. A test asserts the second claim.

### 19.5 — The privacy line, and making it true
*"Nothing leaves this machine until you run an agent."*

That is the strongest claim in the entire product and it is mechanically enforced today —
there is no code path that ships scrollback, files or environment to a provider. Onboarding
displaying it is fine **only if a test proves it**, because a claim in an onboarding screen
is exactly where a future regression would go unnoticed.

*Exit:* a test enumerates every outbound network call and asserts each is a user-configured
endpoint reached only during an agent turn.

## Verification

```sh
cargo test --workspace
pnpm exec vitest run
```

Manual: onboard on a machine with a fresh config directory. Then again with VS Code and
two Claude installs present, declining everything, and confirm nothing on disk changed.
