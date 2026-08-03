/**
 * Tervin Thread: timeline plus composer.
 *
 * Two things here are load-bearing rather than cosmetic.
 *
 * **Capability-aware controls.** Plan mode, model choice, and resume appear only
 * when the running adapter actually supports them, and a partially-supported
 * capability shows its caveat. Nothing is faked into place.
 *
 * **Honest permission status.** The composer states who decides about actions —
 * Tervin Rules or the provider's own system — because a user glancing at this
 * panel must not conclude Tervin is gating actions when it is only observing.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as api from "../lib/api";
import { describeError, useWorkspace, type ThreadView } from "../lib/store";
import { containsPane } from "../lib/panes";
import {
  applyEmacs,
  applyVimNormal,
  isSubmit,
  type EditResult,
  type VimMode,
} from "../lib/editing";
import { toneForState } from "../App";
import { PathComplete } from "./PathComplete";

export function ThreadPanel() {
  const s = useWorkspace();
  const thread = s.activeThreadId ? s.threads[s.activeThreadId] : null;
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [showReasoning, setShowReasoning] = useState(false);
  const editMode = s.appearance.composerMode;
  // A session someone started in a pane. Tervin observes it and cannot drive it, so
  // there is nothing for a composer to do.
  const observedInPane = thread?.paneId ?? null;
  // Kept current by the `pane://cwd` listener, so this follows the pane as it moves.
  const focusedCwd = useWorkspace((st) => {
    const tab = st.tabs.find((t) => t.id === st.activeTabId);
    const paneId = tab?.activePaneId;
    return paneId ? (st.panes[paneId]?.cwd ?? null) : null;
  });
  const [vimMode, setVimMode] = useState<VimMode>("insert");
  // Refs rather than state: neither should cause a re-render, and both must be current
  // inside the keydown handler without rebuilding it.
  const killRing = useRef("");
  const pendingOp = useRef("");

  // A mode change resets vim to insert. Landing in normal mode in a box you just
  // switched on would look like a broken text field.
  useEffect(() => {
    setVimMode("insert");
    pendingOp.current = "";
  }, [editMode]);
  const endRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);

  // `@path` completion state.
  //
  // The selected index lives here rather than in the picker so the textarea keeps
  // focus: moving focus into a dropdown mid-sentence loses the caret.
  const [pathQuery, setPathQuery] = useState<{ at: number; query: string } | null>(null);
  const [pathSelected, setPathSelected] = useState(0);
  const [pathCount, setPathCount] = useState(0);

  /** Re-evaluate whether the caret sits inside an `@path` reference. */
  const syncPathQuery = useCallback((value: string, cursor: number) => {
    const found = api.atPathQuery(value, cursor);
    setPathQuery(found);
    setPathSelected(0);
  }, []);

  /** Replace the `@…` span with the chosen path. */
  const acceptPath = useCallback(
    (path: string) => {
      if (!pathQuery) return;
      const input = inputRef.current;
      const cursor = input?.selectionStart ?? prompt.length;
      const next = `${prompt.slice(0, pathQuery.at)}@${path} ${prompt.slice(cursor)}`;
      setPrompt(next);
      setPathQuery(null);
      // Put the caret after the inserted path, not at the end of the whole prompt.
      const caret = pathQuery.at + path.length + 2;
      requestAnimationFrame(() => {
        input?.focus();
        input?.setSelectionRange(caret, caret);
      });
    },
    [pathQuery, prompt],
  );

  const profiles = s.agents?.profiles ?? [];
  const profile = profiles.find((p) => p.id === s.activeProfileId) ?? profiles[0];

  // What was asked for and what is actually running. They differ whenever an alias
  // was used, which is most of the time, and the difference is what it costs.
  // Guarded all the way down. A Thread can be observed before its metadata has
  // arrived, and reaching through a missing one takes the whole panel with it.
  const resolvedModel = thread?.info?.metadata?.model ?? null;
  const requestedModel = s.activeModel;
  const modelLine =
    resolvedModel && requestedModel && resolvedModel !== requestedModel
      ? `${requestedModel} → ${resolvedModel}`
      : (resolvedModel ?? (requestedModel || null));

  // Poll live session facts while a Thread is working. Metadata such as cost and
  // MCP state is push-free on the runtime side, so it is pulled at a low rate
  // rather than on every event.
  useEffect(() => {
    if (!thread || !["starting", "understanding", "planning", "reading", "editing", "executing", "testing"].includes(thread.state))
      return;
    const id = setInterval(() => void s.refreshThreadInfo(thread.id), 1500);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [thread?.id, thread?.state]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [thread?.events.length]);

  // A prepared handoff lands in the composer, not on the wire. The user chooses the
  // agent and can read — or edit — every word before it is shared.
  useEffect(() => {
    if (!s.pendingHandoff) return;
    setPrompt(s.pendingHandoff);
    s.setHandoff(null);
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      // Caret at the start: the briefing is long, and the useful thing to read
      // first is the task, not the omissions at the end.
      inputRef.current?.setSelectionRange(0, 0);
      inputRef.current?.scrollTo({ top: 0 });
    });
  }, [s.pendingHandoff, s]);

  /**
   * The subagent currently doing the work, if there is one.
   *
   * Folded into a single live line rather than one row per report: a subagent
   * emits progress on every tool call, so rendering each would bury the parent's
   * own timeline under a subagent's file reads. The finish stays a real row, since
   * that is a milestone rather than a heartbeat.
   */
  const activeSubagent = useMemo(() => {
    if (!thread) return null;
    let current: Record<string, unknown> | null = null;
    for (const e of thread.events) {
      if (e.payload.type === "subagent.progress") current = e.payload;
      if (e.payload.type === "subagent.finished") current = null;
    }
    return current;
  }, [thread]);

  const visible = useMemo(() => {
    if (!thread) return [];
    return thread.events.filter((e) => {
      if (e.payload.type === "runtime.unclassified") return false;
      // Shown live above the composer instead, so a heartbeat does not become a
      // timeline of its own.
      if (e.payload.type === "subagent.progress") return false;
      if (!showReasoning && e.payload.type === "agent.message" && e.payload.is_reasoning)
        return false;
      if (e.payload.type === "thread.state") return false;
      return true;
    });
  }, [thread, showReasoning]);

  /**
   * Collapse runs of identical consecutive events into one row with a count.
   *
   * A failing hook fires once per tool call, so a broken gate produced 106
   * byte-identical lines and a timeline nobody could read. The information in the
   * hundredth repeat is the number, not the text. Only *consecutive* identical
   * events collapse, so ordering is never rearranged and an interleaved event
   * always breaks the run.
   */
  const grouped = useMemo(() => {
    const out: { event: (typeof visible)[number]; count: number }[] = [];
    for (const event of visible) {
      const last = out[out.length - 1];
      if (last && sameEvent(last.event, event)) {
        last.count += 1;
      } else {
        out.push({ event, count: 1 });
      }
    }
    return out;
  }, [visible]);

  async function send() {
    const text = prompt.trim();
    if (!text || busy) return;
    setBusy(true);
    try {
      // Anything staged from the terminal, plus every `@path` written in the
      // prompt, travels as an explicit attachment — nothing implicit is sent.
      const referenced = [...text.matchAll(/(?:^|\s)@([^\s]+)/g)].map((m) => m[1]!);
      const attachments = [
        ...s.stagedAttachments,
        ...referenced.map((path) => ({ kind: "file", path })),
      ];

      if (thread && thread.info?.running) {
        await api.threadSend(thread.id, text, attachments);
        s.clearAttachments();
      } else {
        const started = await api.threadStart({
          profile_id: profile?.id ?? null,
          prompt: text,
          attachments,
          model: s.activeModel || null,
          effort: s.activeEffort || null,
          // Only meaningful here. An agent proposes a plan by calling
          // `ExitPlanMode`, which it does only when it started in plan mode, so a
          // mode chosen after the fact cannot produce one.
          permission_mode: s.activeMode || null,
          task_title: text.slice(0, 80),
        });
        s.clearAttachments();
        s.upsertThread({
          id: started.thread_id,
          profileId: started.profile_id,
          runtimeId: started.runtime_id,
          title: text.slice(0, 80),
          state: "starting",
          events: [],
          capabilities: started.capabilities,
          permissions: started.permissions,
          info: null,
        });
        s.setActiveThread(started.thread_id);
        void s.refreshThreadInfo(started.thread_id);
      }
      setPrompt("");
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  const caps = thread?.capabilities;
  const perms = thread?.permissions;

  /**
   * Where the work is, or will be.
   *
   * For a running Thread this is the runtime's own answer rather than the
   * directory Tervin asked for, because they can differ and the runtime's is the
   * one that decides what every path means. With no Thread it is the project root,
   * which is where the next one will start — deliberately not the focused pane's
   * directory, and worth saying so before someone assumes otherwise.
   */
  const threadCwd =
    thread?.info?.metadata?.cwd ?? (thread ? null : (s.environment?.project_root ?? null));

  return (
    // `width: 100%` and `minWidth: 0` are load-bearing: this renders as a flex item,
    // and a flex item with no width sizes to its content — which left the right-hand
    // half of the surface empty.
    <div className="col" style={{ height: "100%", minHeight: 0, width: "100%", minWidth: 0 }}>
      {/* Header: who is working, and in what state. */}
      <div
        className="row"
        style={{
          padding: "var(--sp-2) var(--sp-3)",
          borderBottom: "1px solid var(--tervin-line)",
          flex: "none",
          gap: "var(--sp-2)",
        }}
      >
        {thread ? (
          <>
            <span className={`dot dot-${toneForState(thread.state)}`} />
            <span className="col grow truncate" style={{ gap: 0, minWidth: 0 }}>
              <span className="truncate" title={thread.title}>
                {thread.title}
              </span>
              {/* Where the work is happening, under the title rather than buried in
                  a panel. Every path an agent reads or writes is relative to this,
                  and a Thread pointed at the wrong directory is indistinguishable
                  from one pointed at the right directory until it edits something. */}
              {threadCwd && (
                <span className="meta mono truncate" title={threadCwd}>
                  {abbreviatePath(threadCwd)}
                </span>
              )}
            </span>
            <span className={`meta tone-${toneForState(thread.state)}`}>
              {thread.state.replace(/_/g, " ")}
            </span>
            {thread.info?.running && (
              <button
                className="btn btn-danger"
                onClick={() => void api.threadInterrupt(thread.id).catch(() => {})}
                title="Stop this Thread"
              >
                Stop
              </button>
            )}
            <HandoffButton threadId={thread.id} />
          </>
        ) : (
          <span className="col grow" style={{ gap: 0, minWidth: 0 }}>
            <span className="meta">No Thread running</span>
            {/* Said before starting, not after. A Thread runs in the project root
                rather than the focused pane's directory, which is not what someone
                who has just navigated a pane elsewhere would assume. */}
            {threadCwd && (
              <span className="meta mono truncate" title={threadCwd}>
                next Thread runs in {abbreviatePath(threadCwd)}
              </span>
            )}
          </span>
        )}
      </div>

      {/* Timeline. */}
      <div className="grow" style={{ overflow: "auto", minHeight: 0, padding: "var(--sp-2)" }}>
        {!thread ? (
          <div className="empty">
            Start a Thread by describing a task below. Tervin shows the plan, the
            files read and changed, every command run, and the test results — and
            says plainly who is deciding about each action.
          </div>
        ) : (
          <>
            {grouped.map(({ event, count }) => (
              <TimelineRow key={event.id} event={event} repeated={count} />
            ))}
            <div ref={endRef} />
          </>
        )}
      </div>

      {/* Capability and permission disclosure, plus whatever is working right now.
          A running subagent counts on its own: capabilities and permissions arrive
          when the session reports them, and gating this strip on them would hide a
          working subagent during exactly the early, quiet stretch that reads as a
          dead Thread. */}
      {thread && (caps || perms || activeSubagent) && (
        <div
          style={{
            borderTop: "1px solid var(--tervin-line)",
            padding: "var(--sp-2) var(--sp-3)",
            flex: "none",
          }}
        >
          {perms && (
            <div className="meta row" style={{ gap: "var(--sp-2)", alignItems: "flex-start" }}>
              <span
                className={`dot ${perms.tervin_can_intercept ? "dot-teal" : "dot-amber"}`}
                style={{ marginTop: 5 }}
              />
              <span className="selectable">
                <strong>{perms.tervin_can_intercept ? "Tervin Rules gate this Thread" : "Provider-native approvals"}</strong>
                {" — "}
                {perms.explanation}
              </span>
            </div>
          )}
          {caps && <CapabilityStrip caps={caps} />}
          {activeSubagent && <SubagentLine progress={activeSubagent} />}
          <HookRuns runs={thread.info?.metadata?.hook_runs ?? []} />
        </div>
      )}

      {/* Composer — replaced by an explanation when the session is not ours to drive. */}
      {observedInPane ? (
        <ObservedNotice paneId={observedInPane} agent={thread?.runtimeId ?? "The agent"} />
      ) : (
      <div
        style={{
          borderTop: "1px solid var(--tervin-line)",
          padding: "var(--sp-2)",
          flex: "none",
          background: "var(--tervin-panel)",
        }}
      >
        <div className="row" style={{ marginBottom: "var(--sp-2)", gap: "var(--sp-2)", flexWrap: "wrap" }}>
          {/* Agent profile picker: this is how multiple accounts are switched. */}
          <select
            value={profile?.id ?? ""}
            onChange={(e) => s.setActiveProfile(e.target.value)}
            aria-label="Agent profile"
            title="Which agent and account to use"
          >
            {profiles.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>

          {profile?.sensitive && (
            <span className="chip tone-amber" title="This profile uses a work or shared account">
              {profile.badge ?? "shared"} account
            </span>
          )}

          {/* Model and effort, wherever the runtime declares them. Always shown, even
              while a Thread runs: they are launch flags, so they apply to the next
              Thread rather than this one, and hiding them meant the only moment you
              could not reach the model picker was while watching a Thread use the
              wrong model. */}
          <LaunchPickers profile={profile} appliesToNext={!!thread?.info?.running} />

          {/* Modes as the running session reported them. Never a hard-coded list:
              Claude Code offers four, an ACP agent offers whatever it defines, and
              a control offering a mode the agent would reject is worse than none. */}
          {thread && <ModePicker thread={thread} />}

          {/* Only shown in vim mode, and only because the mode changes what every
              keystroke means — an invisible modal state is the one thing worse than
              no modal editing. */}
          {editMode === "vim" && (
            <span
              className={`chip ${vimMode === "normal" ? "chip-amber" : ""}`}
              title="Escape for normal mode, i to insert"
            >
              {vimMode === "normal" ? "NORMAL" : "INSERT"}
            </span>
          )}

          <button
            className="btn btn-ghost"
            onClick={() => setShowReasoning((v) => !v)}
            title="Reasoning is kept collapsed by default"
          >
            {showReasoning ? "Hide reasoning" : "Show reasoning"}
          </button>
        </div>

        <div style={{ position: "relative" }}>
          {/* The picker sits above the composer so it never covers what is typed. */}
          {pathQuery && (
            <div
              style={{
                position: "absolute",
                bottom: "calc(100% + var(--sp-2))",
                left: 0,
                right: 0,
                zIndex: 40,
              }}
            >
              <PathComplete
                query={pathQuery.query}
                // Scoped to the focused pane's directory, so `@src/…` in a split means
                // that pane's `src` rather than the project root's. Falls back to the
                // whole index when no pane is focused.
                relativeTo={focusedCwd}
                selected={pathSelected}
                onCount={setPathCount}
                onAccept={acceptPath}
              />
            </div>
          )}

          <textarea
            ref={inputRef}
            value={prompt}
            onChange={(e) => {
              setPrompt(e.target.value);
              syncPathQuery(e.target.value, e.target.selectionStart ?? 0);
            }}
            onClick={(e) =>
              syncPathQuery(
                e.currentTarget.value,
                e.currentTarget.selectionStart ?? 0,
              )
            }
            onBlur={() => setPathQuery(null)}
            onKeyDown={(e) => {
              // While the picker is open it owns the arrows, Tab, and Enter — but
              // only Enter, so Shift-Enter still inserts a newline.
              if (pathQuery && pathCount > 0) {
                if (e.key === "ArrowDown") {
                  e.preventDefault();
                  setPathSelected((i) => Math.min(i + 1, pathCount - 1));
                  return;
                }
                if (e.key === "ArrowUp") {
                  e.preventDefault();
                  setPathSelected((i) => Math.max(i - 1, 0));
                  return;
                }
                if (e.key === "Escape") {
                  e.preventDefault();
                  setPathQuery(null);
                  return;
                }
                if (e.key === "Enter" && !e.shiftKey) {
                  // Accepting a completion, not sending the prompt.
                  e.preventDefault();
                  const list = document.querySelectorAll<HTMLElement>(
                    '[role="option"]',
                  );
                  list[pathSelected]?.dispatchEvent(
                    new MouseEvent("mousedown", { bubbles: true }),
                  );
                  return;
                }
              }

              const input = e.currentTarget;
              const stroke = {
                key: e.key,
                ctrl: e.ctrlKey,
                alt: e.altKey,
                meta: e.metaKey,
                shift: e.shiftKey,
              };

              // ⌘⏎ or ^⏎ sends. Plain Enter is a newline, because a prompt is usually
              // several lines and a box where Enter submits makes writing one an
              // exercise in not pressing it.
              if (isSubmit(stroke)) {
                e.preventDefault();
                void send();
                return;
              }

              if (editMode === "native") return;

              const state = {
                text: input.value,
                start: input.selectionStart ?? 0,
                end: input.selectionEnd ?? 0,
              };

              // Escape leaves vim's insert mode. It is claimed only in vim mode, so
              // Escape still closes an overlay everywhere else.
              if (editMode === "vim" && vimMode === "insert" && e.key === "Escape") {
                e.preventDefault();
                setVimMode("normal");
                return;
              }

              // Branched explicitly rather than with a nested ternary: vim's result
              // carries a pending operator that emacs's does not, and collapsing the
              // two into one union only obscures that.
              let result: EditResult = { handled: false, state };
              if (editMode === "emacs") {
                result = applyEmacs(state, stroke, killRing.current);
              } else if (editMode === "vim" && vimMode === "normal") {
                const vim = applyVimNormal(
                  state,
                  stroke,
                  pendingOp.current,
                  killRing.current,
                );
                pendingOp.current = vim.pending ?? "";
                result = vim;
              }

              if (result.vimMode) setVimMode(result.vimMode);
              if (result.yanked) killRing.current = result.yanked;

              // Unhandled keys go to the platform untouched. Swallowing them is how an
              // emulation breaks IME, dead keys, and accessibility.
              if (!result.handled) return;
              e.preventDefault();

              if (result.state.text !== state.text) {
                setPrompt(result.state.text);
              }
              // The caret is applied after React commits the value, or it would be
              // clobbered by the re-render.
              const { start, end } = result.state;
              requestAnimationFrame(() => {
                input.setSelectionRange(start, end);
              });
              syncPathQuery(result.state.text, start);
            }}
            placeholder={
              thread?.info?.running
                ? "Reply to the agent…  @ attaches a file"
                : "Describe a task. @ attaches a file, ⌘⏎ sends."
            }
            rows={3}
            style={{ width: "100%", resize: "vertical", fontFamily: "inherit" }}
            aria-label="Prompt"
          />
        </div>

        {s.stagedAttachments.length > 0 && (
          <div
            className="row"
            style={{ marginTop: "var(--sp-2)", gap: "var(--sp-1)", flexWrap: "wrap" }}
          >
            {/* Shown before sending, because this is what will leave the machine. */}
            {s.stagedAttachments.map((attachment, i) => (
              <span key={i} className="chip chip-teal">
                {describeAttachment(attachment)}
              </span>
            ))}
            <button className="btn btn-xs" onClick={s.clearAttachments}>
              Clear
            </button>
          </div>
        )}

        <div className="row" style={{ marginTop: "var(--sp-2)" }}>
          <span className="meta truncate grow">
            {profile ? `${profile.name} · ${profile.runtime_id}` : "No agent profile configured"}
            {/* An alias is not what runs. `opus` resolves to whichever model is
                current, and which one that is decides what the Thread costs, so
                once the session says what it actually got, that is shown too. */}
            {modelLine && ` · ${modelLine}`}
          </span>
          <button className="btn btn-primary" onClick={() => void send()} disabled={busy || !prompt.trim()}>
            {busy ? "Working…" : thread?.info?.running ? "Send" : "Start Thread"}
          </button>
        </div>
      </div>
      )}
    </div>
  );
}

/**
 * Model and reasoning effort, chosen before a Thread starts.
 *
 * Every option comes from the adapter, never from a list written here. Both are
 * launch flags rather than session controls, so they take effect when a Thread
 * starts. They are shown regardless, and say so when a Thread is already running.
 *
 * They were once hidden while a Thread ran, on the reasoning that a control which
 * cannot change the running session should not be offered. The effect was that the
 * one moment the model picker could not be reached was while watching a Thread run
 * on the wrong model, with no way to set the next one without abandoning the view.
 * A control that says what it applies to beats a control that vanishes.
 *
 * The empty value is a real choice, not a placeholder. It means "whatever the
 * profile and the CLI already select", which is different from passing an empty
 * flag, and it is what a user who has not thought about models should get. It is
 * also why a Thread can run on the account's own default: nothing was overridden.
 */
function LaunchPickers({
  profile,
  appliesToNext,
}: {
  profile: api.AgentProfile | undefined;
  appliesToNext: boolean;
}) {
  const s = useWorkspace();
  const options = profile ? s.agents?.launch_options[profile.runtime_id] : undefined;
  const models = options?.models ?? [];
  const efforts = options?.efforts ?? [];
  const modes = options?.modes ?? [];

  if (models.length === 0 && efforts.length === 0 && modes.length === 0) return null;

  const scope = appliesToNext ? " Applies to the next Thread, not the one running." : "";
  const describe = (choices: api.LaunchChoice[], value: string) => {
    const note = choices.find((c) => c.value === value)?.note;
    return `${note ?? ""}${note ? " " : ""}${scope}`.trim() || undefined;
  };

  return (
    <>
      {models.length > 0 && (
        <select
          value={models.some((m) => m.value === s.activeModel) ? s.activeModel : ""}
          onChange={(e) => s.setActiveModel(e.target.value)}
          aria-label="Model"
          title={
            describe(models, s.activeModel) ??
            `Which model a Thread starts with.${scope}`
          }
        >
          {models.map((m) => (
            <option key={m.value} value={m.value} title={m.note ?? undefined}>
              {m.label}
            </option>
          ))}
        </select>
      )}

      {efforts.length > 0 && (
        <select
          value={efforts.some((x) => x.value === s.activeEffort) ? s.activeEffort : ""}
          onChange={(e) => s.setActiveEffort(e.target.value)}
          aria-label="Effort"
          title={
            describe(efforts, s.activeEffort) ??
            `How much reasoning to spend.${scope}`
          }
        >
          {efforts.map((x) => (
            <option key={x.value} value={x.value} title={x.note ?? undefined}>
              {x.label}
            </option>
          ))}
        </select>
      )}

      {/* Plan mode is the reason this one has to be here rather than only on a
          running session. An agent proposes a plan by calling `ExitPlanMode`, and
          it only does that when it started in plan mode — so a Thread launched in
          `auto` can never produce one, and the Plan surface stays empty forever
          however patiently you wait for it. */}
      {modes.length > 0 && (
        <select
          value={modes.some((m) => m.value === s.activeMode) ? s.activeMode : ""}
          onChange={(e) => s.setActiveMode(e.target.value)}
          aria-label="Start mode"
          title={describe(modes, s.activeMode) ?? `Which mode a Thread starts in.${scope}`}
        >
          <option value="">Start mode: default</option>
          {modes.map((m) => (
            <option key={m.value} value={m.value} title={m.note ?? undefined}>
              {m.label}
            </option>
          ))}
        </select>
      )}

      {/* Said plainly, not only in a tooltip: the pickers are visible beside a
          running Thread they do not govern, and that has to be unambiguous. */}
      {appliesToNext && (
        <span className="chip" title="These apply when the next Thread starts">
          next Thread
        </span>
      )}
    </>
  );
}

/**
 * Shorten a path for a header without losing the part that identifies it.
 *
 * The home directory becomes `~`, and a long path keeps its last few segments
 * rather than its first: `/Users/x/Projects/tervin/crates/tervin-app` truncated
 * from the left reads as `/Users/x/Projects/…`, which is the half every path on
 * the machine shares. The tail is what tells you which directory this is.
 *
 * The element carries the full path as a tooltip, so nothing is actually hidden.
 */
export function abbreviatePath(path: string): string {
  const home = /^\/Users\/[^/]+/.exec(path) ?? /^\/home\/[^/]+/.exec(path);
  const short = home ? `~${path.slice(home[0].length)}` : path;
  if (short.length <= 44) return short;

  const parts = short.split("/").filter(Boolean);
  const tail = parts.slice(-3).join("/");
  return `…/${tail}`;
}

/**
 * The subagent currently working, and what it has spent.
 *
 * This line exists because its absence was read as a crash. A `Task` hands the
 * work to a subagent that can run for minutes; the parent makes one tool call and
 * then says nothing, so the Thread looks dead while it is busy. Everything here
 * comes from the runtime's own progress reports, which Tervin previously discarded.
 *
 * The counts are the point. "Working" alone is indistinguishable from stuck; ten
 * tools and 157k tokens is visibly progress.
 */
function SubagentLine({ progress }: { progress: Record<string, unknown> }) {
  const kind = String(progress.subagent_type ?? "subagent");
  const description = String(progress.description ?? "");
  const tools = Number(progress.tool_uses ?? 0);
  const tokens = Number(progress.total_tokens ?? 0);
  const elapsed = Number(progress.elapsed_ms ?? 0);

  const compactTokens =
    tokens >= 1000 ? `${Math.round(tokens / 1000)}k tokens` : `${tokens} tokens`;
  const seconds = Math.round(elapsed / 1000);

  return (
    <div className="meta row" style={{ gap: "var(--sp-2)", alignItems: "center" }}>
      <span className="dot dot-teal" />
      <span>
        <strong>{kind}</strong> subagent
      </span>
      <span className="tabular">
        {tools} {tools === 1 ? "tool" : "tools"} · {compactTokens} · {seconds}s
      </span>
      {description && (
        <span className="truncate grow" title={description}>
          {description}
        </span>
      )}
    </div>
  );
}

/** Capabilities as a compact strip, with reasons on hover for what is absent. */
/**
 * The mode control for a running Thread.
 *
 * Every option comes from the session itself. Nothing appears when a runtime
 * reports no modes, because the alternative — showing a plausible list — produces a
 * control that silently fails, and a mode is exactly the setting a user must be
 * able to trust.
 */
function ModePicker({ thread }: { thread: ThreadView }) {
  const s = useWorkspace();
  const [busy, setBusy] = useState(false);
  const modes = thread.info?.metadata?.modes ?? [];
  const current = thread.info?.metadata?.permission_mode ?? thread.permissions?.mode ?? "";

  if (modes.length === 0) return null;

  async function choose(id: string) {
    if (!id || id === current) return;
    setBusy(true);
    try {
      await api.threadSetPermissionMode(thread.id, id);
      await s.refreshThreadInfo(thread.id);
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  const active = modes.find((m) => m.id === current);

  return (
    <select
      value={modes.some((m) => m.id === current) ? current : ""}
      disabled={busy || !thread.info?.running}
      onChange={(e) => void choose(e.target.value)}
      aria-label="Mode"
      // The description says who decides, which is the only thing a mode name
      // needs to convey.
      title={active?.description ?? "How this agent handles permissions"}
    >
      {/* Shown only when the runtime reported a mode Tervin does not have an entry
          for, so the control never silently misrepresents the current state. */}
      {!modes.some((m) => m.id === current) && (
        <option value="">{current || "Mode"}</option>
      )}
      {modes.map((m) => (
        <option key={m.id} value={m.id}>
          {m.name}
        </option>
      ))}
    </select>
  );
}

/**
 * Hand this Thread's work to another agent.
 *
 * The payoff of a provider-neutral event stream: what Claude Code did can be handed to
 * an ACP agent or a local model without either knowing the other exists. The briefing
 * is loaded into the composer rather than sent, because the user picks who receives it
 * — and should see what is being shared before it goes anywhere.
 */
function HandoffButton({ threadId }: { threadId: string }) {
  const s = useWorkspace();
  const [busy, setBusy] = useState(false);

  return (
    <button
      className="btn"
      disabled={busy}
      title="Summarise this Thread's work as a prompt for another agent"
      onClick={() => {
        setBusy(true);
        void api
          .threadHandoff(threadId)
          .then((handoff) => {
            s.setHandoff(handoff.prompt);
            s.pushNotice(
              `Handoff ready (${handoff.summary}). Pick an agent and send — nothing has been shared yet.`,
            );
          })
          .catch((e) => s.pushNotice(describeError(e)))
          .finally(() => setBusy(false));
      }}
    >
      {busy ? "Preparing…" : "Hand off"}
    </button>
  );
}

/**
 * The user's own hooks, as they actually ran.
 *
 * Hooks are the most invisible part of a Claude Code setup — they run silently, and
 * a broken one degrades every session with no message anywhere. So a failure is
 * shown by default and named; the working ones collapse into a count, because "four
 * hooks ran fine" is reassurance, not information.
 *
 * Tervin's own gate is excluded: it reports itself in the timeline, and listing it
 * here would present Tervin's work as the user's configuration.
 */
/** Collapse identical hook failures, keeping first-seen order. */
function groupFailures(runs: api.HookRun[]): { run: api.HookRun; count: number }[] {
  const out: { run: api.HookRun; count: number }[] = [];
  for (const run of runs) {
    const existing = out.find(
      (g) =>
        g.run.name === run.name &&
        g.run.exit_code === run.exit_code &&
        g.run.message === run.message,
    );
    if (existing) existing.count += 1;
    else out.push({ run, count: 1 });
  }
  return out;
}

function HookRuns({ runs }: { runs: api.HookRun[] }) {
  const theirs = runs.filter((r) => !r.is_tervin);
  if (theirs.length === 0) return null;

  const failed = theirs.filter((r) => r.exit_code !== 0 && r.exit_code !== 2);
  const blocked = theirs.filter((r) => r.exit_code === 2);
  const fine = theirs.length - failed.length - blocked.length;

  return (
    <div className="meta col" style={{ gap: 2, marginTop: "var(--sp-1)" }}>
      {/* Grouped, not listed. One broken hook fires per tool call, so an hour of
          work produced 59 byte-identical lines and a panel nobody could read. The
          same reasoning the working hooks already got: the count is the
          information, the repetition is not. Grouped by name, exit code and
          message so two genuinely different failures never merge. */}
      {groupFailures(failed).map(({ run, count }) => (
        <div
          key={`${run.name}-${run.exit_code}-${run.message ?? ""}`}
          className="row"
          style={{ gap: "var(--sp-2)" }}
        >
          <span className="dot dot-amber" />
          <span className="mono">{run.name}</span>
          <span className="tone-amber truncate grow" title={run.message ?? undefined}>
            failed (exit {run.exit_code}){run.message ? `: ${run.message}` : ""}
          </span>
          {count > 1 && (
            <span className="chip tabular" style={{ flex: "none" }} title={`${count} times`}>
              ×{count}
            </span>
          )}
        </div>
      ))}
      {(fine > 0 || blocked.length > 0) && (
        <div className="tone-muted">
          {fine > 0 && `${fine} of your hooks ran`}
          {fine > 0 && blocked.length > 0 && " · "}
          {blocked.length > 0 && `${blocked.length} blocked an action`}
        </div>
      )}
    </div>
  );
}

function CapabilityStrip({ caps }: { caps: api.Capabilities }) {
  const entries: [string, api.CapabilityLevel][] = [
    ["Plan", caps.plan_mode],
    ["Resume", caps.resume],
    ["Tools", caps.tool_events],
    ["Edits", caps.file_edits],
    ["Gate", caps.native_permission_bridge],
    ["MCP", caps.mcp],
    ["Cost", caps.cost_reporting],
  ];
  return (
    <div className="row" style={{ gap: "var(--sp-1)", flexWrap: "wrap", marginTop: "var(--sp-1)" }}>
      {entries.map(([label, level]) => {
        const note = "note" in level ? level.note : "reason" in level ? level.reason : undefined;
        const usable = level.level === "supported" || level.level === "partial";
        return (
          <span
            key={label}
            className="chip"
            title={note ?? `${label}: ${level.level}`}
            style={{
              opacity: usable ? 1 : 0.5,
              borderColor: level.level === "supported" ? "var(--tervin-accent)" : undefined,
              color: level.level === "supported" ? "var(--tervin-accent)" : undefined,
              textDecoration: level.level === "unsupported" ? "line-through" : undefined,
            }}
          >
            {label}
            {level.level === "partial" && "*"}
          </span>
        );
      })}
    </div>
  );
}

/**
 * Two consecutive events are "the same" when a reader would learn nothing from the
 * second. Deliberately compares the rendered summary rather than the payload: an id
 * and a timestamp always differ, and it is the visible line that becomes noise.
 */
function sameEvent(a: api.TervinEvent, b: api.TervinEvent): boolean {
  return a.payload.type === b.payload.type && a.summary === b.summary;
}

function TimelineRow({
  event,
  repeated = 1,
}: {
  event: api.TervinEvent;
  repeated?: number;
}) {
  const [open, setOpen] = useState(false);
  const kind = event.payload.type;
  const risk = (event.payload as { risk?: api.RiskAssessment }).risk;

  // A failure reason is the one payload that must be readable without being asked
  // for: it is where the runtime says what to do next, and a summary line cannot
  // hold it. Anything else stays collapsed.
  const failure =
    kind === "thread.failed"
      ? ((event.payload as { reason?: string }).reason ?? "")
      : "";
  const detail = failure.includes("\n") ? failure.slice(failure.indexOf("\n")).trim() : "";

  return (
    <div
      style={{
        padding: "var(--sp-1) var(--sp-2)",
        borderLeft: `2px solid ${borderForKind(kind)}`,
        marginBottom: 2,
      }}
    >
      <div className="row" style={{ gap: "var(--sp-2)", alignItems: "flex-start" }}>
        <span className="meta tabular" style={{ flex: "none", width: 52 }}>
          {new Date(event.ts).toLocaleTimeString([], { hour12: false })}
        </span>
        <span className="meta" style={{ flex: "none", width: 108 }} title={kind}>
          {kind}
        </span>
        <span
          className="grow selectable"
          style={{ fontSize: "var(--text-meta)", wordBreak: "break-word" }}
        >
          {event.summary}
        </span>
        {repeated > 1 && (
          <span
            className="chip tabular"
            style={{ flex: "none" }}
            title={`This happened ${repeated} times in a row. The timestamp shown is the first.`}
          >
            ×{repeated}
          </span>
        )}
        {risk && risk.level !== "low" && (
          <button
            className={`chip tone-${risk.level === "critical" ? "red" : "amber"}`}
            onClick={() => setOpen((v) => !v)}
            title="Why this was flagged"
          >
            {risk.level}
            {!risk.enforceable && " · observed"}
          </button>
        )}
      </div>

      {detail && (
        <div
          className="meta selectable"
          style={{
            paddingLeft: 172,
            marginTop: "var(--sp-1)",
            whiteSpace: "pre-wrap",
            textWrap: "pretty",
          }}
        >
          {detail}
        </div>
      )}

      {open && risk && (
        <div className="meta selectable" style={{ paddingLeft: 172, marginTop: "var(--sp-1)" }}>
          {risk.reasons.map((r) => (
            <div key={r}>· {r}</div>
          ))}
          {risk.side_effects.map((r) => (
            <div key={r} className="tone-muted">→ {r}</div>
          ))}
          {!risk.enforceable && (
            <div className="tone-amber">
              Tervin observed this action but could not prevent it.
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** A one-line label for a staged attachment. */
function describeAttachment(attachment: Record<string, unknown>): string {
  const kind = String(attachment.kind ?? "context");
  if (kind === "file" || kind === "diff") return String(attachment.path ?? kind);
  if (kind === "selection") {
    const text = String(attachment.text ?? "");
    return `selection · ${text.length} chars`;
  }
  if (kind === "block") return `block · ${String(attachment.command ?? "")}`;
  return kind;
}

function borderForKind(kind: string): string {
  if (kind.startsWith("permission")) return "var(--tervin-amber)";
  if (kind === "thread.failed" || kind.includes("denied")) return "var(--tervin-red)";
  if (kind === "thread.completed" || kind === "test.completed") return "var(--tervin-green)";
  if (kind.startsWith("command") || kind.startsWith("patch") || kind.startsWith("plan"))
    return "var(--tervin-accent)";
  return "var(--tervin-line)";
}

/**
 * What a pane session offers instead of a composer.
 *
 * Tervin has no channel to a process it did not spawn: it cannot send a prompt,
 * answer a permission request, or cancel a turn. A disabled text box would be worse
 * than none — it reads as "not working yet" rather than "type it in the pane".
 *
 * So this says plainly what Tervin is doing, what it is not, and where to type.
 */
function ObservedNotice({ paneId, agent }: { paneId: string; agent: string }) {
  const s = useWorkspace();
  // Panes live in a split tree per tab, so finding the owner means walking it.
  const tab = s.tabs.find((t) => t.root && containsPane(t.root, paneId));

  return (
    <div
      style={{
        borderTop: "1px solid var(--tervin-line)",
        padding: "var(--sp-3)",
        flex: "none",
        background: "var(--tervin-panel)",
      }}
    >
      <div className="row" style={{ gap: "var(--sp-2)", alignItems: "baseline" }}>
        <span className="chip" title="Tervin did not start this session">
          in a pane
        </span>
        <span className="meta grow" style={{ textWrap: "pretty" }}>
          {agent} is running in a terminal pane. Tervin is recording what happens —
          prompts, replies and file changes are searchable — but it cannot send a
          prompt or answer a permission request for a session it did not start. Type
          in the pane itself.
        </span>
      </div>
      {tab && (
        <div className="row" style={{ marginTop: "var(--sp-2)", gap: "var(--sp-2)" }}>
          <button
            className="btn btn-xs"
            onClick={() => {
              s.setActiveTab(tab.id);
              s.setActivePane(paneId);
              s.setSurface("terminal");
            }}
          >
            Show the pane
          </button>
        </div>
      )}
    </div>
  );
}
