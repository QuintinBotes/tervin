# Manual test pass

For deciding whether a build is fit to hand to someone else. It covers the paths
automated tests cannot reach, and it is ordered so that a failure early on tells
you not to trust what comes after.

Tervin is in development. Expect rough edges; the point of this pass is to know
which ones you are shipping.

## Before you start

```sh
./scripts/testbed.sh          # creates ~/tervin-testbed
pnpm app                      # builds and launches
```

Point Tervin at `~/tervin-testbed`, not at Tervin's own source tree. The testbed
is small, disposable, and contains one genuine failing test, so asking an agent
to fix something is a real request rather than a rehearsal. An agent let loose in
Tervin's own repo can edit Tervin while you are testing it.

Two things that will waste your time if you skip them:

- **Start a fresh Thread after any rebuild.** A Thread launched by the previous
  binary holds a socket that no longer answers, and it will keep failing no
  matter what was fixed.
- **Check `git log` for commits you did not write.** An agent running inside
  Tervin can commit to whatever repo it is pointed at.

---

## 1. The terminal is a terminal

Nothing else matters if this is wrong.

- [ ] A pane opens with a working prompt, your theme, and your prompt framework's
      glyphs intact.
- [ ] `ls`, `cd ..`, `cd -` behave. The pane title and cwd track `cd`.
- [ ] `vim README.md` opens, redraws on resize, and exits cleanly.
- [ ] `python3 -m unittest -v` runs and reports one failure.
- [ ] Resize the window mid-command: output reflows, nothing is lost.
- [ ] The column divider can be grabbed and dragged. The line is thin; the target
      is deliberately wider than it looks.
- [ ] A pane is a *clean* shell: `echo $CLAUDE_CODE_CHILD_SESSION` prints nothing,
      even when Tervin itself was launched from inside an agent session.
- [ ] **The first character is not eaten.** Open a new pane and type immediately,
      before the prompt settles. You should see what you typed, not `s -la` for
      `ls -la`. This is the bug #35 fixes; it is easiest to catch by typing fast
      into a freshly opened pane.

## 2. Blocks

- [ ] Each command becomes its own Block with the right command text, exit code
      and duration.
- [ ] A failing command (`python3 -m unittest`) shows a non-zero exit.
- [ ] A command with quotes and semicolons survives intact:
      `echo 'a;b' "c d"`.
- [ ] A large burst stays whole: `for i in $(seq 1 5000); do echo line-$i; done`
      then check both ends are present.
- [ ] Bookmark, tag and note a Block. They persist across a restart.
- [ ] Block search finds a command by its text.

## 3. File explorer

- [ ] The tree shows the testbed, including `src/` and `docs/`.
- [ ] Opening a file shows its contents; a large file does not hang the UI.
- [ ] `.gitignore`d paths (`__pycache__`) are handled the way you intend.
- [ ] Files changed outside Tervin appear without a manual refresh, or refresh
      does what you expect.
- [ ] `scratch.txt` (untracked) and `src/inventory.py` (modified) are both
      visible and distinguishable.

## 4. Git

The testbed is committed once and then dirtied on purpose, so this has content.

- [ ] Branch and clean/dirty state are correct.
- [ ] `src/inventory.py` shows as modified with a real diff; `scratch.txt` as
      untracked.
- [ ] Stage, unstage, and stage a single hunk.
- [ ] A commit made from Tervin appears in `git log` with the right author.

## 5. Agent profiles

This is where a recent bug lived, so check it deliberately.

- [ ] Settings → Agents lists **every profile in your `agents.toml`**. If you
      have five configured, five appear. "No agent profile configured" while the
      file has profiles is the bug #33 fixes.
- [ ] Profiles appear immediately, before the "Discovered runtimes" list fills
      in. Discovery is slower and must never hold them up.
- [ ] "Discovered runtimes" says it is still looking rather than showing an empty
      list while it works.
- [ ] The paths shown for `agents.toml` and `mcp.json` are real and correct for
      macOS.
- [ ] Switching profile in the composer changes which account runs.

## 6. Threads

- [ ] Start a Thread with: `Run the tests and tell me what is failing.`
- [ ] It runs, streams output, and correctly identifies the tax bug.
- [ ] The timeline shows tool calls as they happen, not in one lump at the end.
- [ ] Interrupt mid-run. It stops, and says it stopped.
- [ ] Send a follow-up turn. Context is retained.
- [ ] Repeated identical events collapse to one row with a count, rather than
      filling the panel with duplicates.
- [ ] Attach a Block to a prompt and confirm the agent actually received it.
- [ ] `@path` in a prompt attaches that file, and nothing is sent implicitly.

## 7. Model and effort

New in #34. Both are launch flags, so they apply to a Thread that has not started.

- [ ] The composer offers a model picker and an effort picker.
- [ ] Model options are aliases — Opus, Sonnet, Fable, Haiku — plus a default.
- [ ] Effort offers low, medium, high, extra high, max, plus a default.
- [ ] Leaving both on default starts a Thread with whatever your profile or CLI
      already chooses. It must not pass an empty flag.
- [ ] Pick Opus, start a Thread. Once it reports back, the alias and the model it
      actually resolved to are shown together. They differ, and the difference is
      what it costs.
- [ ] Pick an effort and confirm the Thread starts. An unrecognised value only
      warns and silently falls back, so the point is that the list is right.
- [ ] Switch profile: both selections reset, because a different runtime may not
      accept them.
- [ ] Neither picker appears once a Thread is running.

## 7b. Starting a Thread, and where it runs

- [ ] **New Thread** works three ways: the button in the Deck, `mod+shift+i`, and
      the command palette. All three clear the selection so the composer starts a
      new one.
- [ ] It works *while another Thread is running*. That had no route at all before.
- [ ] Under the composer, the directory says where the next Thread will start, and
      it **follows the focused pane**: `cd` somewhere in the terminal and it moves.
- [ ] Click it and type a path. `Tab` takes the first completion, `Enter` accepts,
      `Escape` abandons the edit without changing anything.
- [ ] A pinned directory shows 📌 and an **unpin**. Emptying the box unpins rather
      than pinning to nothing.
- [ ] A running Thread shows the directory the runtime reports, under its title.

## 7c. Plan mode

The Plan surface can only fill if the Thread *started* in plan mode.

- [ ] New Thread → **Start mode: Plan** → ask for something.
- [ ] The agent proposes rather than acting. Confirm with `git status` that nothing
      was written.
- [ ] The **Plan tab shows a count**, and a notice says a plan is ready.
- [ ] The plan is readable. If the agent wrote prose rather than a list, the text
      is shown as written rather than an empty column.
- [ ] **Approve and continue** is enabled only while the agent is actually waiting.
      Once it has moved on, the line says so instead of offering a dead button.
- [ ] Approving makes the agent execute the plan it wrote.

## 7d. Subagents

- [ ] Ask for something that farms out to an `Explore` subagent.
- [ ] A live line appears: type, tool count, tokens, elapsed, and what it is doing
      right now.
- [ ] It disappears when the subagent finishes, and the finish is a timeline row.
- [ ] The Thread never looks stopped while a subagent is working. That was the bug.

## 8. The permission gate

The most important section. Tervin's central claim is that it can stop an agent,
and this failed open on every single call until recently.

- [ ] **Start a fresh Thread.** Not one from a previous build.
- [ ] Ask for something Tervin Rules should stop, e.g.
      `Delete every file in this project.`
- [ ] It is **blocked**, and the timeline says Tervin blocked it.
- [ ] The gate panel does **not** show `Tervin did not answer within 5s`. That
      line means the gate is failing open and nothing is being checked.
- [ ] Hook failures, if any appear, are attributed correctly. Tervin's own gate
      must not be listed as one of *your* hooks — that is the bug #33 fixes.
- [ ] Approve and deny a pending request; both take effect.
- [ ] With a deliberately broken hook in your own settings, Tervin reports it as
      yours and keeps working.

## 9. Plan handoff

- [ ] Ask for a plan: `Plan how you would fix the failing test. Do not edit yet.`
- [ ] Plan mode writes nothing to disk. Confirm with `git status`.
- [ ] The Plan is readable, and shows the Thread's own title rather than a
      sentence built awkwardly around it.
- [ ] Hand the Plan off to execution. The agent carries the plan's context rather
      than starting cold.
- [ ] It fixes `price_with_tax` and the suite passes:
      `python3 -m unittest -v` reports 3 passing.
- [ ] The edit appears in the Git panel as a normal diff you can review.

## 10. Persistence

- [ ] Quit and reopen. Panes, tabs and layout return.
- [ ] Scrollback is retained where you expect and discarded where you set it to
      be.
- [ ] Threads are listed and can be reopened.
- [ ] Settings survive: theme, font, retention.

## 11. Before distributing

- [ ] Install from a real artifact, not the dev build, on a machine that has
      never run Tervin.
- [ ] Confirm the unsigned-binary path behaves as documented. `curl` and `npm`
      set only `com.apple.provenance`; the quarantine flag comes from the
      downloading application.
- [ ] First launch with no config: Tervin bootstraps rather than erroring.
- [ ] Every install route in the README actually works, or is not in the README.
- [ ] Docs say Tervin is in development, and platform support matches reality.
