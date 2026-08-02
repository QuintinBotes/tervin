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
import { cleanup, render } from "@testing-library/react";
import * as api from "../lib/api";
import { DEFAULT_APPEARANCE, useWorkspace, type ThreadView } from "../lib/store";
import { BlocksPanel } from "./BlocksPanel";
import { ThreadPanel } from "./ThreadPanel";
import { GitPanel } from "./GitPanel";
import { ConnectionsPanel } from "./ConnectionsPanel";

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

  it("labels a connect time as a connect time, never as latency", async () => {
    // SSH reports no round-trip time, so a number called "latency" would be a measurement
    // of something else wearing the wrong name.
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
    // Known rather than inferred, so a millisecond figure would imply a measurement that
    // never happened.
    vi.spyOn(api, "sshProbe").mockResolvedValue({ state: "multiplexed" });
    withHost();
    const { container } = render(<ConnectionsPanel />);
    [...container.querySelectorAll("button")].find((b) => b.textContent === "check")!.click();
    await new Promise((r) => setTimeout(r, 30));

    expect(container.textContent).toContain("connected");
    expect(container.textContent).not.toContain("ms");
  });

  it("says a jumped host is not checkable rather than calling it unreachable", async () => {
    // The case a naive probe gets confidently wrong: the address may only be routable from
    // the jump host.
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
