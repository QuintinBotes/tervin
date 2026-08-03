/**
 * @vitest-environment jsdom
 *
 * Every surface, mounted with real data.
 *
 * This file exists because of a specific failure. `BlocksPanel` was written, read
 * correctly, and was imported nowhere — so none of its row-rendering code had ever run.
 * When it was finally mounted it took the WebKit content process down, and nothing caught
 * that because every test in this project was pure logic.
 *
 * So these assert that a surface renders without throwing, against data shaped like the
 * awkward real cases: a null exit code, a null duration, truncated output, failed tests,
 * an empty list, a Thread whose session has ended. They deliberately do not assert
 * appearance — that is what the design system and a screenshot are for, and a layout
 * assertion would break on every legitimate change while catching none of this.
 */

import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";
import * as api from "../lib/api";
import { DEFAULT_APPEARANCE, useWorkspace, type ThreadView } from "../lib/store";
import { BlocksPanel } from "./BlocksPanel";
import { ThreadPanel, abbreviatePath } from "./ThreadPanel";
import { GitPanel } from "./GitPanel";
import { ConnectionsPanel } from "./ConnectionsPanel";
import { SavedCommands } from "./SavedCommands";
import { CommandHistory } from "./CommandHistory";
import { ProjectInstructions } from "./ProjectInstructions";

/** A Block, with every field overridable so a test can make one awkward. */
function block(over: Partial<api.BlockSummary> = {}): api.BlockSummary {
  return {
    id: `blk_${Math.random().toString(36).slice(2)}`,
    pane_id: "pane_1",
    thread_id: null,
    command: "cargo test --workspace",
    cwd: "/Users/dev/proj",
    host: "local",
    project: "proj",
    started_at: new Date().toISOString(),
    duration_ms: 1234,
    exit_code: 0,
    status: "succeeded",
    bookmarked: false,
    tags: [],
    note: null,
    output_total: 4096,
    output_truncated: false,
    git_branch: "main",
    error_count: 0,
    warning_count: 0,
    tests: null,
    ports: [],
    preview: "test result: ok. 470 passed",
    ...over,
  };
}

// jsdom does not implement `scrollIntoView`, and the components call it to keep a
// timeline pinned to its newest row. Stubbed here rather than guarded in the component:
// it exists in every real browser and in the WebView Tervin ships, so a guard would be
// defensive code for a condition that cannot occur in production.
beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
  Element.prototype.scrollTo = () => {};
});

/** A pane as `makePane` builds one, for restore callbacks. */
function stubPane() {
  return {
    id: `pane_${Math.random().toString(36).slice(2)}`,
    title: "Shell",
    cwd: "/proj",
    threadId: null,
    exited: false,
    exitCode: null,
  };
}

beforeEach(() => {
  cleanup();
  vi.restoreAllMocks();
  // A known starting state, so one test cannot leave data behind that makes the next
  // one pass for the wrong reason.
  useWorkspace.setState({
    blocks: [],
    threads: {},
    activeThreadId: null,
    tabs: [],
    panes: {},
    activeTabId: null,
    appearance: DEFAULT_APPEARANCE,
    agents: null,
    notices: [],
    pendingApprovals: [],
    stagedAttachments: [],
    gitStatus: null,
    connections: null,
    agentsDiscovery: null,
    activeModel: "",
    activeEffort: "",
    activeMode: "",
    activeCwd: null,
  });
});

describe("the Thread's working directory", () => {
  it("is shown for a running Thread, from the runtime's own answer", () => {
    // Every path an agent reads or writes is relative to this, and a Thread aimed
    // at the wrong directory looks exactly like one aimed at the right directory
    // until it edits something.
    useWorkspace.setState({
      activeThreadId: "thr_cwd",
      threads: {
        thr_cwd: {
          id: "thr_cwd",
          profileId: "p1",
          runtimeId: "claude-code",
          title: "find the bug",
          state: "executing",
          events: [],
          capabilities: null,
          permissions: null,
          paneId: null,
          info: {
            running: true,
            metadata: { hook_runs: [], cwd: "/Users/dev/Projects/tervin/crates/tervin-app" },
          } as unknown as api.ThreadInfo,
        } as ThreadView,
      },
    });
    const { getByTitle } = render(<ThreadPanel />);
    expect(getByTitle("/Users/dev/Projects/tervin/crates/tervin-app")).toBeTruthy();
  });

  it("says where the next Thread will run, as something you can change", () => {
    // Not just a label. Before this the directory was inferred and unreachable:
    // no way to see it without starting a Thread, and no way to set it at all.
    useWorkspace.setState({
      activeThreadId: null,
      activeCwd: null,
      environment: { project_root: "/Users/dev/Projects/tervin" } as unknown as api.ShellEnvironment,
    });
    const { getByRole } = render(<ThreadPanel />);
    const button = getByRole("button", { name: /tervin/ });
    expect(button.title).toContain("/Users/dev/Projects/tervin");
    expect(button.title).toContain("Following the focused pane");
  });

  it("says when the directory is pinned rather than following the pane", () => {
    // A directory that silently follows something else is fine. One that follows
    // something else without saying so is how an agent works in the wrong repo.
    useWorkspace.setState({
      activeThreadId: null,
      activeCwd: "/Users/dev/Projects/other",
      environment: { project_root: "/Users/dev/Projects/tervin" } as unknown as api.ShellEnvironment,
    });
    const { getByRole, getByText } = render(<ThreadPanel />);
    expect(getByRole("button", { name: /other/ }).title).toContain("Pinned");
    // And a way back, or pinning would be a one-way door.
    expect(getByText("unpin")).toBeTruthy();
  });

  it("is changed by typing a path, not by an OS file dialog", async () => {
    // This is a terminal. The muscle memory is `cd`, the paths are already
    // indexed, and a modal chooser for a directory you could type in four
    // keystrokes is the wrong idiom in an app whose argument is that the keyboard
    // is faster.
    vi.spyOn(api, "pathComplete").mockResolvedValue([]);
    useWorkspace.setState({
      activeThreadId: null,
      activeCwd: null,
      environment: { project_root: "/Users/dev/Projects/tervin" } as unknown as api.ShellEnvironment,
    });
    const { getByRole, findByLabelText } = render(<ThreadPanel />);

    getByRole("button", { name: /tervin/ }).click();
    const input = (await findByLabelText("Directory for the next Thread")) as HTMLInputElement;

    // `fireEvent` rather than assigning `value`: React tracks the previous value
    // on the node and ignores a raw assignment, so the state never updates.
    fireEvent.change(input, { target: { value: "/Users/dev/Projects/other" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(useWorkspace.getState().activeCwd).toBe("/Users/dev/Projects/other");
  });

  it("treats an emptied box as going back to following the pane", async () => {
    // The obvious meaning of clearing it, and the same thing unpin does. Leaving a
    // Thread pinned to "" would point an agent at nothing.
    vi.spyOn(api, "pathComplete").mockResolvedValue([]);
    useWorkspace.setState({
      activeThreadId: null,
      activeCwd: "/Users/dev/Projects/other",
      environment: { project_root: "/Users/dev/Projects/tervin" } as unknown as api.ShellEnvironment,
    });
    const { getByRole, findByLabelText } = render(<ThreadPanel />);

    getByRole("button", { name: /other/ }).click();
    const input = (await findByLabelText("Directory for the next Thread")) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(useWorkspace.getState().activeCwd).toBeNull();
  });

  it("follows the focused pane rather than the project root", () => {
    // The terminal is the context the user is working in. It also has to match
    // `@path` completion, which already resolves against the pane's directory —
    // otherwise a completed path means one file in the composer and another in
    // the Thread.
    useWorkspace.setState({
      activeThreadId: null,
      environment: { project_root: "/Users/dev/Projects/tervin" } as unknown as api.ShellEnvironment,
      activeTabId: "tab1",
      tabs: [
        { id: "tab1", title: "t", root: null, activePaneId: "pane1", zoomedPaneId: null },
      ] as unknown as ReturnType<typeof useWorkspace.getState>["tabs"],
      panes: {
        pane1: {
          id: "pane1",
          title: "Shell",
          cwd: "/Users/dev/Projects/tervin/crates/block-engine",
          threadId: null,
          exited: false,
          exitCode: null,
        },
      },
    });
    const { getByRole } = render(<ThreadPanel />);
    // The pane's directory wins, and the project root is not what is offered.
    expect(getByRole("button", { name: /block-engine/ })).toBeTruthy();
  });
});

describe("abbreviating a path", () => {
  it("keeps the tail, because the head is what every path shares", () => {
    // Truncating from the left leaves `/Users/dev/Projects/…`, which identifies
    // nothing. The last segments are the part that says which directory this is.
    const short = abbreviatePath("/Users/dev/Projects/tervin/crates/agent-runtime/src/claude");
    expect(short).toContain("claude");
    expect(short.startsWith("…/")).toBe(true);
  });

  it("writes the home directory as a tilde", () => {
    expect(abbreviatePath("/Users/dev/proj")).toBe("~/proj");
  });

  it("leaves a short path alone", () => {
    expect(abbreviatePath("/tmp/x")).toBe("/tmp/x");
  });
});

describe("a running subagent", () => {
  function threadWith(events: unknown[]): ThreadView {
    return {
      id: "thr_sub",
      profileId: "p1",
      runtimeId: "claude-code",
      title: "find the bug",
      state: "understanding",
      events: events as ThreadView["events"],
      capabilities: null,
      permissions: null,
      paneId: null,
      info: { running: true, metadata: { hook_runs: [] } } as unknown as api.ThreadInfo,
    };
  }

  const progress = (over: Record<string, unknown> = {}) => ({
    id: "e1",
    thread_id: "thr_sub",
    ts: new Date().toISOString(),
    summary: "Explore · Reading ThreadPanel.tsx",
    payload: {
      type: "subagent.progress",
      tool_use_id: "toolu_1",
      subagent_type: "Explore",
      description: "Reading ThreadPanel.tsx",
      tool_uses: 10,
      total_tokens: 157251,
      elapsed_ms: 22979,
      ...over,
    },
  });

  it("says what it is and what it has spent, so quiet is not mistaken for dead", () => {
    // The report that prompted this: twenty file reads by an Explore subagent,
    // none of them attributed, and a Thread that looked stopped while working.
    useWorkspace.setState({
      activeThreadId: "thr_sub",
      threads: { thr_sub: threadWith([progress()]) },
    });
    const { getByText } = render(<ThreadPanel />);

    expect(getByText("Explore")).toBeTruthy();
    expect(getByText(/10 tools/)).toBeTruthy();
    // Rounded, because the exact token count is noise at a glance.
    expect(getByText(/157k tokens/)).toBeTruthy();
    expect(getByText("Reading ThreadPanel.tsx")).toBeTruthy();
  });

  it("stops showing one that has finished", () => {
    useWorkspace.setState({
      activeThreadId: "thr_sub",
      threads: {
        thr_sub: threadWith([
          progress(),
          {
            id: "e2",
            thread_id: "thr_sub",
            ts: new Date().toISOString(),
            summary: "Explore finished · 10 tools · 157251 tokens",
            payload: { type: "subagent.finished", tool_use_id: "toolu_1", subagent_type: "Explore" },
          },
        ]),
      },
    });
    const { queryByText } = render(<ThreadPanel />);
    expect(queryByText("Reading ThreadPanel.tsx")).toBeNull();
  });

  it("keeps the newest report rather than the first", () => {
    // Progress arrives per tool call. Showing the first would freeze the counts at
    // one, which is its own way of looking stuck.
    useWorkspace.setState({
      activeThreadId: "thr_sub",
      threads: {
        thr_sub: threadWith([
          progress({ tool_uses: 1, description: "Reading store.ts" }),
          { ...progress({ tool_uses: 9, description: "Reading api.ts" }), id: "e2" },
        ]),
      },
    });
    const { getByText, queryByText } = render(<ThreadPanel />);
    expect(getByText(/9 tools/)).toBeTruthy();
    expect(queryByText("Reading store.ts")).toBeNull();
  });
});

describe("the composer's launch pickers", () => {
  function withRuntime(
    models: api.LaunchChoice[],
    efforts: api.LaunchChoice[],
    modes: api.LaunchChoice[] = [],
  ) {
    useWorkspace.setState({
      agents: {
        profiles: [
          {
            id: "p1",
            name: "Claude",
            runtime_id: "claude-code",
            binary: "claude",
            args: [],
            env: {},
            model: null,
            permission_mode: null,
            badge: null,
            sensitive: false,
          },
        ],
        default_profile: "p1",
        launch_options: { "claude-code": { models, efforts, modes } },
        profiles_path: "~/.config/tervin/agents.toml",
        mcp_path: "~/.config/tervin/mcp.json",
      },
      activeProfileId: "p1",
    });
  }

  it("offers exactly what the adapter declared", () => {
    withRuntime(
      [
        { value: "", label: "Profile default" },
        { value: "opus", label: "Opus" },
        { value: "haiku", label: "Haiku" },
      ],
      [
        { value: "", label: "Default effort" },
        { value: "max", label: "Max" },
      ],
    );
    const { getByLabelText } = render(<ThreadPanel />);

    const model = getByLabelText("Model") as HTMLSelectElement;
    expect([...model.options].map((o) => o.value)).toEqual(["", "opus", "haiku"]);
    const effort = getByLabelText("Effort") as HTMLSelectElement;
    expect([...effort.options].map((o) => o.value)).toEqual(["", "max"]);
  });

  it("shows no control for a runtime that declared none", () => {
    // A picker offering a choice the runtime would reject is worse than no picker,
    // so an adapter that declares nothing gets nothing drawn.
    withRuntime([], []);
    const { queryByLabelText } = render(<ThreadPanel />);
    expect(queryByLabelText("Model")).toBeNull();
    expect(queryByLabelText("Effort")).toBeNull();
  });

  it("stays reachable while a Thread is running, marked as the next one's", () => {
    // The case that made this change. Hidden during a run, the only moment the
    // model picker could not be reached was while watching a Thread use the wrong
    // model — and the way out was to abandon the view. It now says what it governs
    // instead of disappearing.
    withRuntime(
      [
        { value: "", label: "Profile default" },
        { value: "sonnet", label: "Sonnet" },
      ],
      [],
    );
    useWorkspace.setState({
      activeThreadId: "thr_live",
      threads: {
        thr_live: {
          id: "thr_live",
          profileId: "p1",
          runtimeId: "claude-code",
          title: "a Thread already running",
          state: "executing",
          events: [],
          capabilities: null,
          permissions: null,
          paneId: null,
          info: { running: true } as unknown as api.ThreadInfo,
        } as ThreadView,
      },
    });

    const { getByLabelText, getByText } = render(<ThreadPanel />);
    expect(getByLabelText("Model")).toBeTruthy();
    expect(getByText("next Thread")).toBeTruthy();
  });

  it("offers a start mode, without which the Plan surface can never fill", () => {
    // The whole reason a mode belongs at launch. An agent proposes a plan by
    // calling `ExitPlanMode`, and it only does that when it started in plan mode.
    // Tervin sent no mode at all, so every Thread ran in `auto`, no plan event was
    // ever emitted, and the Plan tab sat empty no matter how long anyone waited.
    withRuntime([], [], [
      { value: "plan", label: "Plan", note: "Proposes a plan and writes nothing." },
      { value: "auto", label: "Auto" },
    ]);
    const { getByLabelText } = render(<ThreadPanel />);

    const mode = getByLabelText("Start mode") as HTMLSelectElement;
    expect([...mode.options].map((o) => o.value)).toEqual(["", "plan", "auto"]);
    // Defaults to unset, so nothing is imposed on a user who has not chosen.
    expect(mode.value).toBe("");
  });

  it("does not preselect a value the runtime never offered", () => {
    // Otherwise a selection carried over from another runtime renders as chosen
    // while the flag sent is something else entirely.
    withRuntime([{ value: "", label: "Profile default" }, { value: "opus", label: "Opus" }], []);
    useWorkspace.setState({ activeModel: "gpt-5" });
    const { getByLabelText } = render(<ThreadPanel />);
    expect((getByLabelText("Model") as HTMLSelectElement).value).toBe("");
  });
});

describe("BlocksPanel", () => {
  it("renders an empty list", () => {
    // The state it is in on a fresh install, and the state it was in when History
    // appeared to work.
    expect(() => render(<BlocksPanel />)).not.toThrow();
  });

  it("renders Blocks whose nullable fields are actually null", () => {
    // A running Block has no exit code and no duration. Reading either without a guard
    // is the likeliest way a row throws, and it is invisible until real data arrives.
    useWorkspace.setState({
      blocks: [
        block({ exit_code: null, duration_ms: null, status: "running" }),
        block({ git_branch: null, project: null, command: "" }),
        block({ status: "unknown", exit_code: null, duration_ms: null, preview: "" }),
      ],
    });
    expect(() => render(<BlocksPanel />)).not.toThrow();
  });

  it("renders a failure with diagnostics, ports, tags, and test counts", () => {
    useWorkspace.setState({
      blocks: [
        block({
          command: "pnpm test",
          status: "failed",
          exit_code: 1,
          error_count: 3,
          warning_count: 7,
          ports: [5173, 8080],
          tests: { runner: "vitest", passed: 110, failed: 2, skipped: 1 },
          output_truncated: true,
          output_total: 64 * 1024 * 1024,
          tags: ["ci", "flaky"],
          bookmarked: true,
        }),
      ],
    });
    const { container } = render(<BlocksPanel />);
    // The one content assertion worth making: a failure has to be findable.
    expect(container.textContent).toContain("pnpm test");
    expect(container.textContent).toContain("2 failing");
  });

  it("narrows to failures when asked", () => {
    useWorkspace.setState({
      blocks: [
        block({ command: "ok-command" }),
        block({ command: "bad-command", status: "failed", exit_code: 1 }),
      ],
    });
    const { container } = render(<BlocksPanel failuresOnly />);
    expect(container.textContent).toContain("bad-command");
    expect(container.textContent).not.toContain("ok-command");
  });

  it("survives a preview containing control characters", () => {
    // Output reaches a Block with escape sequences stripped, but a stray byte must not
    // be able to break rendering.
    const escape = String.fromCharCode(27);
    useWorkspace.setState({
      blocks: [block({ preview: `${escape}[31mred${escape}[0m\r\n\0` })],
    });
    expect(() => render(<BlocksPanel />)).not.toThrow();
  });

  it("survives a command long enough to be a paste accident", () => {
    useWorkspace.setState({
      blocks: [block({ command: "echo ".repeat(5000) })],
    });
    expect(() => render(<BlocksPanel />)).not.toThrow();
  });
});

describe("ThreadPanel", () => {
  it("renders with no Thread selected", () => {
    expect(() => render(<ThreadPanel />)).not.toThrow();
  });

  it("renders a Thread whose session has ended and reports no capabilities", () => {
    // Reached from prompt history: the events are on disk, but there is no live session,
    // so capabilities and permissions are null and every control has to cope.
    useWorkspace.setState({
      activeThreadId: "thr_1",
      threads: {
        thr_1: {
          id: "thr_1",
          profileId: "claude",
          runtimeId: "claude-code",
          title: "fix the flaky auth test",
          state: "completed",
          events: [],
          capabilities: null,
          permissions: null,
          info: null,
        },
      },
    });
    expect(() => render(<ThreadPanel />)).not.toThrow();
  });

  it("renders a timeline including an event type it does not model", () => {
    // `runtime.unclassified` exists precisely so an unmodelled event is kept. The panel
    // filters it out, and a payload with none of the fields it expects must not throw.
    const event = (type: string, payload: Record<string, unknown> = {}): api.TervinEvent => ({
      id: `evt_${type}_${Math.random()}`,
      thread_id: "thr_1",
      ts: new Date().toISOString(),
      agent: { runtime_id: "claude-code", display_name: "Claude Code", tier: "structured" },
      project: "proj",
      cwd: "/Users/dev/proj",
      // A timeline row shows the *summary*, not the payload — "one concise line,
      // written for a human scanning a timeline, never a dump of the payload". So the
      // fixture gives each event a realistic one.
      summary: payload.summaryText ? String(payload.summaryText) : `runtime: ${type}`,
      raw: null,
      links: [],
      payload: { type, ...payload },
    });

    useWorkspace.setState({
      activeThreadId: "thr_1",
      threads: {
        thr_1: {
          id: "thr_1",
          profileId: "claude",
          runtimeId: "claude-code",
          title: "a thread",
          state: "failed",
          events: [
            event("user.prompted", { text: "do the thing", summaryText: "do the thing" }),
            event("runtime.unclassified", { source_type: "something/new" }),
            event("thread.failed", { reason: "line one\n\nline two with advice" }),
            // A payload missing the field the row reads.
            event("permission.denied"),
          ],
          capabilities: null,
          permissions: null,
          info: null,
        },
      },
    });
    const { container } = render(<ThreadPanel />);
    expect(container.textContent).toContain("do the thing");
    // A multi-line failure reason is shown expanded, because that is where a runtime
    // says what to do next.
    expect(container.textContent).toContain("line two with advice");
  });
});

/**
 * `GitPanel` and `ConnectionsPanel` were both complete and imported nowhere, so neither had
 * ever rendered. Wiring them up is what makes these tests necessary: a component's first
 * mount is where it crashes, and both are now on a code path a user can reach.
 */
describe("ThreadPanel, for an agent running in a pane", () => {
  /** A Thread as `thread://observed` produces one: read-only, pinned to a pane. */
  function observed(over: Partial<ThreadView> = {}): ThreadView {
    return {
      id: "thr_pane",
      profileId: "",
      runtimeId: "claude-code",
      title: "fix the flaky auth test",
      state: "idle",
      events: [],
      capabilities: null,
      permissions: null,
      info: null,
      paneId: "pane_1",
      ...over,
    };
  }

  it("shows no composer, because Tervin cannot type into a session it did not start", () => {
    useWorkspace.setState({ activeThreadId: "thr_pane", threads: { thr_pane: observed() } });
    const { container } = render(<ThreadPanel />);

    // The whole point: a disabled box would read as "not working yet" rather than
    // "type it in the pane".
    expect(container.querySelector("textarea")).toBeNull();
    expect(container.textContent).toContain("in a pane");
    expect(container.textContent).toContain("cannot send a prompt");
  });

  it("still has a composer for a Thread Tervin launched", () => {
    // The guard is `paneId`, so this is the assertion that keeps it from hiding the
    // composer for every Thread.
    useWorkspace.setState({
      activeThreadId: "thr_pane",
      threads: { thr_pane: observed({ paneId: null }) },
    });
    expect(render(<ThreadPanel />).container.querySelector("textarea")).not.toBeNull();
  });

  it("renders a pane Thread's timeline, including a tool call and a file change", () => {
    const event = (
      payload: { type: string } & Record<string, unknown>,
      summary: string,
    ): api.TervinEvent => ({
      id: `evt_${Math.random()}`,
      thread_id: "thr_pane",
      ts: new Date().toISOString(),
      agent: { runtime_id: "claude-code", display_name: "Claude Code", tier: "enhanced_cli" },
      project: "proj",
      cwd: "/proj",
      summary,
      raw: null,
      links: [{ pane_id: "pane_1" }],
      payload,
    });

    useWorkspace.setState({
      activeThreadId: "thr_pane",
      threads: {
        thr_pane: observed({
          events: [
            event({ type: "thread.started", tier: "enhanced_cli" }, "Claude Code is running in this pane"),
            event({ type: "user.prompted", text: "fix the flaky auth test" }, "fix the flaky auth test"),
            event(
              { type: "tool.requested", tool_use_id: "t1", tool_name: "Bash", input_summary: "Bash cargo test" },
              "Bash cargo test",
            ),
            // A result the transcript could not pair with its request: the ids are
            // empty on purpose, and the row must still render.
            event(
              { type: "tool.completed", tool_use_id: "", tool_name: "", is_error: false, output_summary: "ok" },
              "Tool finished",
            ),
            event(
              { type: "file.changed", change: { path: "/proj/src/auth.rs", kind: "modified" } },
              "Changed /proj/src/auth.rs",
            ),
          ],
        }),
      },
    });

    const { container } = render(<ThreadPanel />);
    expect(container.textContent).toContain("fix the flaky auth test");
    expect(container.textContent).toContain("Bash cargo test");
    expect(container.textContent).toContain("auth.rs");
  });

  it("offers no way to reveal a pane that no longer exists", () => {
    // The pane can close while the Thread stays on disk. A button that jumps nowhere
    // is worse than no button.
    useWorkspace.setState({
      activeThreadId: "thr_pane",
      threads: { thr_pane: observed({ paneId: "pane_gone" }) },
      tabs: [],
    });
    const { container } = render(<ThreadPanel />);
    expect(container.textContent).toContain("in a pane");
    expect(container.textContent).not.toContain("Show the pane");
  });
});

describe("session restore", () => {
  it("keeps the restore setting on by default", () => {
    // Losing the arrangement you built is the cost that keeps people running tmux under
    // a terminal that cannot do this, so the useful behaviour is the default.
    expect(DEFAULT_APPEARANCE.restoreSession).toBe(true);
  });

  it("saves nothing and clears what was saved when switched off", async () => {
    const saved: string[] = [];
    const retained: string[][] = [];
    // The backend calls are the observable behaviour here; the point of the test is that
    // turning the setting off *deletes*, rather than merely stopping future saves.
    vi.spyOn(api, "workspaceSave").mockImplementation(async (_id, _name, json) => {
      saved.push(json);
    });
    vi.spyOn(api, "scrollbackRetain").mockImplementation(async (keys) => {
      retained.push(keys);
      return 0;
    });

    useWorkspace.setState({
      appearance: { ...DEFAULT_APPEARANCE, restoreSession: false },
    });
    await useWorkspace.getState().saveSession(() => "output");

    // An empty layout, not a real one: restore is off.
    expect(saved).toEqual([""]);
    // And an empty retain list, which deletes every saved pane's output.
    expect(retained).toEqual([[]]);
  });

  it("does not restore over a workspace that already has tabs", async () => {
    // The startup effect can run twice in development. Restoring on the second run
    // would duplicate every tab.
    vi.spyOn(api, "workspaceLoad").mockResolvedValue(null);
    useWorkspace.setState({
      appearance: DEFAULT_APPEARANCE,
      tabs: [{ id: "t1", title: "existing", root: null, activePaneId: null, zoomedPaneId: null }],
    });
    expect(await useWorkspace.getState().restoreSession(() => stubPane())).toBe(false);
  });

  it("reports false when there is nothing saved, so the caller opens a fresh pane", async () => {
    vi.spyOn(api, "workspaceLoad").mockResolvedValue(null);
    useWorkspace.setState({ appearance: DEFAULT_APPEARANCE, tabs: [] });
    expect(await useWorkspace.getState().restoreSession(() => stubPane())).toBe(false);
  });

  it("reports false when the saved session cannot be read, rather than throwing", async () => {
    // A failed read on launch must not stop the app opening.
    vi.spyOn(api, "workspaceLoad").mockRejectedValue(new Error("database is locked"));
    useWorkspace.setState({ appearance: DEFAULT_APPEARANCE, tabs: [] });
    await expect(useWorkspace.getState().restoreSession(() => stubPane())).resolves.toBe(false);
  });
});

describe("ConnectionsPanel reachability", () => {
  function withHost(over: Record<string, unknown> = {}) {
    useWorkspace.setState({
      connections: {
        shells: [],
        ssh_hosts: [
          {
            alias: "build-box",
            hostname: "10.0.0.4",
            user: "dev",
            port: 22,
            identity_file: null,
            proxy_jump: null,
            proxy_command: null,
            forward_agent: null,
            request_tty: null,
            source_file: "~/.ssh/config",
            is_pattern: false,
            ...over,
          },
        ],
        ssh_warnings: [],
        multiplexers: [],
        serial: [],
        wsl: [],
      },
    });
  }

  it("does not probe anything until asked", async () => {
    // A config can name a hundred machines. Probing on open would be a port scan of the
    // user's own infrastructure.
    const probe = vi.spyOn(api, "sshProbe").mockResolvedValue({ state: "timeout" });
    withHost();
    render(<ConnectionsPanel />);
    await new Promise((r) => setTimeout(r, 30));
    expect(probe).not.toHaveBeenCalled();
  });

  it("says a key is not loaded, which is the whole point of checking", async () => {
    // Knowing a connection is about to ask for a passphrase, before it asks.
    vi.spyOn(api, "sshKeyStatus").mockResolvedValue([
      ["build-box", { status: "not_loaded", path: "~/.ssh/id_ed25519" }],
    ]);
    withHost();
    const { container } = render(<ConnectionsPanel />);
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent).toContain("key not loaded");
  });

  it("says nothing when the key is loaded", async () => {
    // A row decorated with "fine" for every host is noise that hides the one that is not.
    vi.spyOn(api, "sshKeyStatus").mockResolvedValue([
      ["build-box", { status: "loaded", comment: "me@laptop" }],
    ]);
    withHost();
    const { container } = render(<ConnectionsPanel />);
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent).not.toContain("key not loaded");
  });

  it("does not call an uncheckable key a missing one", async () => {
    // The key may well be in the agent; saying otherwise sends someone looking for a
    // problem that is not there.
    vi.spyOn(api, "sshKeyStatus").mockResolvedValue([
      [
        "build-box",
        { status: "cannot_fingerprint", path: "~/.ssh/id", reason: "not there" },
      ],
    ]);
    withHost();
    const { container } = render(<ConnectionsPanel />);
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent).toContain("not checkable");
    expect(container.textContent).not.toContain("key not loaded");
  });

  it("labels a connect time as a connect time, never as latency", async () => {
    // The whole reason this feature is shaped the way it is. SSH reports no round-trip
    // time, so a number called "latency" would be a measurement of something else.
    vi.spyOn(api, "sshProbe").mockResolvedValue({ state: "open", connect_ms: 42 });
    withHost();
    const { container } = render(<ConnectionsPanel />);

    const check = [...container.querySelectorAll("button")].find(
      (b) => b.textContent === "check",
    )!;
    check.click();
    await new Promise((r) => setTimeout(r, 30));

    expect(container.textContent).toContain("42 ms to connect");
    expect(container.textContent?.toLowerCase()).not.toContain("latency");
    expect(container.textContent?.toLowerCase()).not.toContain("ping");
  });

  it("shows an already-open connection without a number", async () => {
    // Known rather than inferred, so attaching a millisecond figure would imply a
    // measurement that never happened.
    vi.spyOn(api, "sshProbe").mockResolvedValue({ state: "multiplexed" });
    withHost();
    const { container } = render(<ConnectionsPanel />);
    [...container.querySelectorAll("button")].find((b) => b.textContent === "check")!.click();
    await new Promise((r) => setTimeout(r, 30));

    expect(container.textContent).toContain("connected");
    expect(container.textContent).not.toContain("ms");
  });

  it("says a jumped host is not checkable rather than calling it unreachable", async () => {
    // The case a naive implementation gets confidently wrong: the address may only be
    // routable from the jump host.
    vi.spyOn(api, "sshProbe").mockResolvedValue({
      state: "skipped",
      reason: "reached through bastion, so it cannot be probed directly",
    });
    withHost({ proxy_jump: "bastion" });
    const { container } = render(<ConnectionsPanel />);
    [...container.querySelectorAll("button")].find((b) => b.textContent === "check")!.click();
    await new Promise((r) => setTimeout(r, 30));

    expect(container.textContent).toContain("not checkable");
    expect(container.textContent?.toLowerCase()).not.toContain("unreachable");
  });
});

describe("SavedCommands", () => {
  function view(over: Partial<api.SavedCommandView> = {}): api.SavedCommandView {
    return {
      id: "sc_1",
      name: "deploy",
      template: "deploy {{env:staging}} --service {{service}}",
      description: "Ship a service",
      uses: 3,
      parameters: [
        { name: "env", default: "staging" },
        { name: "service", default: null },
      ],
      ...over,
    };
  }

  it("explains what a saved command is when there are none", async () => {
    vi.spyOn(api, "savedCommands").mockResolvedValue([]);
    const { container } = render(<SavedCommands />);
    await new Promise((r) => setTimeout(r, 20));
    expect(container.textContent).toContain("Nothing saved yet");
    expect(container.textContent).toContain("{{env:staging}}");
  });

  it("says how many parts a command needs filled", async () => {
    vi.spyOn(api, "savedCommands").mockResolvedValue([view()]);
    const { container } = render(<SavedCommands />);
    await new Promise((r) => setTimeout(r, 20));
    expect(container.textContent).toContain("2 to fill");
  });

  it("states that Enter does not run the command", async () => {
    // A saved command is often the destructive kind, so which it does has to be explicit.
    vi.spyOn(api, "savedCommands").mockResolvedValue([view()]);
    const { container } = render(<SavedCommands />);
    await new Promise((r) => setTimeout(r, 20));
    expect(container.textContent).toContain("does not run it");
  });

  it("shows the filled-in line and warns about anything left blank", async () => {
    vi.spyOn(api, "savedCommands").mockResolvedValue([view()]);
    const { container } = render(<SavedCommands />);
    await new Promise((r) => setTimeout(r, 20));

    const row = container.querySelector('[role="button"]') as HTMLElement;
    row.click();
    await new Promise((r) => setTimeout(r, 20));

    // The default is prefilled; `service` is not, so its hole stays visible rather than
    // being emptied. An argument silently missing changes what a command does.
    expect(container.textContent).toContain("deploy staging --service {{service}}");
    expect(container.textContent).toContain("not filled in");
  });
});

describe("CommandHistory", () => {
  function hit(over: Partial<api.CommandSuggestion> = {}): api.CommandSuggestion {
    return {
      command: "cargo test --workspace",
      uses: 12,
      age_hours: 3,
      failed_last_time: false,
      ...over,
    };
  }

  it("says when a command failed the last time it ran", async () => {
    // The one thing a shell's Ctrl-R cannot tell you, and the thing most worth knowing
    // before pressing Enter on something from a week ago.
    vi.spyOn(api, "commandHistory").mockResolvedValue([
      hit({ command: "cargo test", failed_last_time: true }),
    ]);
    const { container } = render(<CommandHistory />);
    await new Promise((r) => setTimeout(r, 150));
    expect(container.textContent).toContain("failed last time");
  });

  it("says nothing about a command that succeeded", async () => {
    vi.spyOn(api, "commandHistory").mockResolvedValue([hit()]);
    const { container } = render(<CommandHistory />);
    await new Promise((r) => setTimeout(r, 150));
    expect(container.textContent).toContain("cargo test --workspace");
    expect(container.textContent).not.toContain("failed last time");
  });

  it("states that Enter does not run the command", async () => {
    // Reusing a command from history is exactly when you want to glance at it: it may name
    // a branch that no longer exists.
    vi.spyOn(api, "commandHistory").mockResolvedValue([hit()]);
    const { container } = render(<CommandHistory />);
    await new Promise((r) => setTimeout(r, 150));
    expect(container.textContent).toContain("does not run it");
  });

  it("explains that history fills up by use rather than showing a blank panel", async () => {
    vi.spyOn(api, "commandHistory").mockResolvedValue([]);
    const { container } = render(<CommandHistory />);
    await new Promise((r) => setTimeout(r, 150));
    expect(container.textContent).toContain("as you run them");
  });

  it("searches every project by default, since that is the point", async () => {
    // A shell's history is one machine and one session's ancestry. The reason this beats it
    // is "that command from the other repo", so the default must not be scoped.
    const spy = vi.spyOn(api, "commandHistory").mockResolvedValue([hit()]);
    useWorkspace.setState({
      environment: {
        shell: "/bin/zsh",
        integration: [],
        aliases: {
          aliases: {},
          functions: [],
          global_aliases: {},
          shell: "/bin/zsh",
          notes: [],
          enumerated: true,
        },
        project_root: "/Users/dev/Projects/tervin",
        home: "/Users/dev",
        notices: [],
      },
    });
    render(<CommandHistory />);
    await new Promise((r) => setTimeout(r, 150));
    expect(spy).toHaveBeenCalledWith("", null, 60);
  });
});

describe("GitPanel", () => {
  it("renders with no repository", () => {
    // The state outside a git working tree, which is where a panel that assumes a repo
    // throws on its first read.
    expect(() => render(<GitPanel />)).not.toThrow();
  });

  it("renders a mid-rebase repo with a conflict and a detached HEAD", () => {
    // The combination that matters: during a rebase there is no branch name, HEAD is
    // detached, and a conflicted file is in neither the index nor the worktree stage.
    useWorkspace.setState({
      gitStatus: {
        root: "/Users/dev/proj",
        branch: null,
        head_sha: "4f2a91c",
        detached: true,
        upstream: null,
        ahead: 0,
        behind: 0,
        dirty: true,
        operation_in_progress: "rebase",
        files: [
          {
            path: "src/auth.rs",
            original_path: null,
            stage: "conflicted",
            index_change: null,
            worktree_change: null,
          },
          {
            path: "src/new.rs",
            original_path: null,
            stage: "untracked",
            index_change: null,
            worktree_change: "added",
          },
          {
            path: "src/moved.rs",
            original_path: "src/old.rs",
            stage: "both",
            index_change: "renamed",
            worktree_change: "modified",
          },
        ],
      },
    });
    const { container } = render(<GitPanel />);
    // A rebase in progress changes what committing means, so it has to be stated.
    expect(container.textContent).toContain("rebase");
    expect(container.textContent).toContain("src/auth.rs");
  });

  it("renders a clean repo that has diverged from its upstream", () => {
    useWorkspace.setState({
      gitStatus: {
        root: "/Users/dev/proj",
        branch: "main",
        head_sha: "abc1234",
        detached: false,
        upstream: "origin/main",
        ahead: 2,
        behind: 3,
        dirty: false,
        operation_in_progress: null,
        files: [],
      },
    });
    const { container } = render(<GitPanel />);
    expect(container.textContent).toContain("main");
  });
});

describe("ConnectionsPanel", () => {
  it("renders before the connection scan has returned", () => {
    // It mounts and *then* scans, so the null state is what a user sees first.
    expect(() => render(<ConnectionsPanel />)).not.toThrow();
  });

  it("renders hosts, sessions, and devices, including an unusable SSH pattern", () => {
    useWorkspace.setState({
      connections: {
        shells: [
          {
            id: "zsh",
            name: "zsh",
            program: "/bin/zsh",
            args: ["-l"],
            env: [],
            cwd: null,
            supports_integration: true,
          },
        ],
        // A `Host *.internal` entry is not connectable — every field but the alias is
        // null, and it must render as such rather than as a host you can click.
        ssh_hosts: [
          {
            alias: "*.internal",
            hostname: null,
            user: null,
            port: null,
            identity_file: null,
            proxy_jump: null,
            proxy_command: null,
            forward_agent: null,
            request_tty: null,
            source_file: "~/.ssh/config",
            is_pattern: true,
          },
          {
            alias: "build-box",
            hostname: "10.0.0.4",
            user: "dev",
            port: 2222,
            identity_file: "~/.ssh/id_ed25519",
            proxy_jump: "bastion",
            proxy_command: null,
            forward_agent: true,
            request_tty: "force",
            source_file: "~/.ssh/config",
            is_pattern: false,
          },
        ],
        ssh_warnings: ["Ignored an Include that resolved outside ~/.ssh"],
        multiplexers: [
          { program: "tmux", name: "work", detail: "3 windows", attached: true },
          { program: "zellij", name: "scratch", detail: null, attached: false },
        ],
        serial: [{ path: "/dev/tty.usbmodem1101", label: "USB modem" }],
        wsl: [],
      },
    });
    const { container } = render(<ConnectionsPanel />);
    expect(container.textContent).toContain("build-box");
    expect(container.textContent).toContain("work");
    // A warning about a config it could not fully read must not be swallowed.
    expect(container.textContent).toContain("Include");
  });
});

describe("ProjectInstructions", () => {
  /** Discovery output, with every field overridable so a test can make it awkward. */
  function found(
    over: Partial<api.ProjectInstructions> = {},
  ): api.ProjectInstructions {
    return {
      project_root: "~/Projects/thing",
      discovered: {
        files: [
          {
            path: "/p/AGENTS.md",
            kind: "agents",
            scope: { scope: "project_root" },
            bytes: 16078,
          },
          {
            path: "/p/.cursorrules",
            kind: "cursor_rules",
            scope: { scope: "project_root" },
            bytes: 512,
          },
        ],
        mcp: [],
        truncated: false,
      },
      adoptable: [],
      ...over,
    };
  }

  it("says which files the chosen runtime obeys and which it ignores", async () => {
    vi.spyOn(api, "projectInstructions").mockResolvedValue(found());
    const { findByText, getAllByText } = render(<ProjectInstructions />);

    // Claude Code is the default when no Thread is running.
    expect(await findByText("AGENTS.md")).toBeTruthy();
    expect(getAllByText("in force").length).toBe(1);
    // The Cursor rules file is present but not read, and saying so is the point.
    expect(getAllByText("not read").length).toBe(1);
  });

  it("renders a nested file with the directory it governs, not just its name", async () => {
    // Three nested CLAUDE.md files would otherwise be three identical rows.
    vi.spyOn(api, "projectInstructions").mockResolvedValue(
      found({
        discovered: {
          files: [
            {
              path: "/p/crates/engine/CLAUDE.md",
              kind: "claude_md",
              scope: { scope: "nested", relative_dir: "crates/engine" },
              bytes: 900,
            },
          ],
          mcp: [],
          truncated: false,
        },
      }),
    );
    const { findByText } = render(<ProjectInstructions />);
    expect(await findByText(/crates\/engine/)).toBeTruthy();
  });

  it("reports an unparseable MCP config rather than hiding it", async () => {
    // A runtime that silently ignores its own broken config leaves a user with
    // nothing to go on, which is the whole reason the error is carried through.
    vi.spyOn(api, "projectInstructions").mockResolvedValue(
      found({
        discovered: {
          files: [],
          mcp: [
            {
              path: "/p/.mcp.json",
              kind: "project_mcp_json",
              servers: [],
              error: "not valid JSON: expected value at line 1 column 3",
            },
          ],
          truncated: false,
        },
      }),
    );
    const { findByText } = render(<ProjectInstructions />);
    expect(await findByText(/could not be parsed/)).toBeTruthy();
  });

  it("does not present a capped search as a complete list", async () => {
    vi.spyOn(api, "projectInstructions").mockResolvedValue(
      found({ discovered: { files: [], mcp: [], truncated: true } }),
    );
    const { findByText } = render(<ProjectInstructions />);
    expect(await findByText(/capped/)).toBeTruthy();
  });

  it("says an adoption would replace a server Tervin already has", async () => {
    vi.spyOn(api, "projectInstructions").mockResolvedValue(
      found({
        adoptable: [
          { name: "github", source: "/p/.mcp.json (the .mcp.json convention)", conflicts: true },
          { name: "sentry", source: "/p/.mcp.json (the .mcp.json convention)", conflicts: false },
        ],
      }),
    );
    const { findByText, queryByText } = render(<ProjectInstructions />);
    expect(await findByText(/already has this name/)).toBeTruthy();
    // And does not say it about the one that is genuinely new.
    expect(queryByText("sentry")).toBeTruthy();
  });

  it("explains itself rather than throwing when the project cannot be read", async () => {
    vi.spyOn(api, "projectInstructions").mockRejectedValue(
      new Error("permission denied"),
    );
    const { findByText } = render(<ProjectInstructions />);
    expect(await findByText(/permission denied/)).toBeTruthy();
  });

  it("renders an empty project without claiming anything was found", async () => {
    vi.spyOn(api, "projectInstructions").mockResolvedValue(
      found({ discovered: { files: [], mcp: [], truncated: false } }),
    );
    const { findByText } = render(<ProjectInstructions />);
    expect(await findByText(/No instruction files here/)).toBeTruthy();
  });
});

describe("a timeline with a repeating event", () => {
  /**
   * The case this exists for: a broken hook fires once per tool call, so the real
   * timeline held 106 byte-identical lines. The information in the hundredth repeat
   * is the count, not the text.
   */
  function repeatingThread(): ThreadView {
    const base: ThreadView = {
      id: "thr_repeat",
      profileId: "p1",
      runtimeId: "claude-code",
      title: "a thread whose hook keeps failing",
      state: "executing",
      events: [],
      capabilities: null,
      permissions: null,
      info: null,
      paneId: null,
    };
    const events = [];
    for (let i = 0; i < 30; i++) {
      events.push({
        id: `e${i}`,
        thread_id: base.id,
        ts: new Date(Date.now() + i * 10).toISOString(),
        summary: "PreToolUse:Bash failed (exit 1) — Tervin hook: Tervin did not answer within 5s.",
        payload: { type: "tool.failed" },
      });
    }
    // One different event in the middle, so the run must break rather than swallow it.
    events.splice(15, 0, {
      id: "different",
      thread_id: base.id,
      ts: new Date().toISOString(),
      summary: "cargo test --workspace",
      payload: { type: "command.completed" },
    });
    return { ...base, events: events as unknown as ThreadView["events"] };
  }

  it("collapses a run into one row with a count instead of 30 identical lines", async () => {
    const t = repeatingThread();
    useWorkspace.setState({ threads: { [t.id]: t }, activeThreadId: t.id });
    const { findAllByText, queryAllByText } = render(<ThreadPanel />);

    // Two runs of 15, split by the interleaved event, so two count chips.
    const chips = await findAllByText(/^×15$/);
    expect(chips.length).toBe(2);

    // And the identical line is rendered twice, not thirty times.
    const lines = queryAllByText(/did not answer within 5s/);
    expect(lines.length).toBe(2);
  });

  it("keeps the event that interrupted the run", async () => {
    // Collapsing must never drop an event: only consecutive identical ones merge.
    const t = repeatingThread();
    useWorkspace.setState({ threads: { [t.id]: t }, activeThreadId: t.id });
    const { findByText } = render(<ThreadPanel />);
    expect(await findByText("cargo test --workspace")).toBeTruthy();
  });
});
