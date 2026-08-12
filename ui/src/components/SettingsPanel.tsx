/**
 * Settings.
 *
 * Everything here previews live. A theme or font change is applied as CSS
 * variables and pushed into the running terminals in place, so you judge a choice
 * by looking at your actual work rather than at a swatch.
 */

import { useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import { describeError, useWorkspace } from "../lib/store";
import { ProjectInstructions } from "./ProjectInstructions";
import { THEMES } from "../design/themes";

type Section = "appearance" | "shell" | "agents" | "rules" | "about";

const SECTIONS: [Section, string][] = [
  ["appearance", "Appearance"],
  ["shell", "Shell integration"],
  ["agents", "Agents"],
  ["rules", "Tervin Rules"],
  ["about", "About"],
];

export function SettingsPanel() {
  const s = useWorkspace();
  const [section, setSection] = useState<Section>("appearance");
  const dialogRef = useRef<HTMLDivElement | null>(null);

  // Take the keyboard on open. Without this, focus stays in the terminal pane
  // underneath and everything typed here — including Return — goes to the shell.
  useEffect(() => {
    dialogRef.current?.focus();
  }, []);

  return (
    <div
      ref={dialogRef}
      role="dialog"
      aria-modal="true"
      aria-label="Settings"
      tabIndex={-1}
      onClick={() => s.setSettings(false)}
      style={{
        position: "fixed",
        inset: 0,
        background: "color-mix(in srgb, var(--tervin-bg) 70%, transparent)",
        display: "grid",
        placeItems: "center",
        padding: "var(--sp-6)",
        zIndex: 150,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="row"
        style={{
          width: "min(940px, 96vw)",
          height: "min(660px, 88vh)",
          background: "var(--tervin-panel)",
          border: "1px solid var(--tervin-line)",
          borderRadius: "var(--radius-lg)",
          overflow: "hidden",
          alignItems: "stretch",
        }}
      >
        <nav
          className="col"
          style={{
            width: 178,
            borderRight: "1px solid var(--tervin-line)",
            padding: "var(--sp-2)",
            flex: "none",
            gap: 1,
          }}
        >
          {SECTIONS.map(([id, label]) => (
            <button
              key={id}
              className="btn btn-ghost"
              onClick={() => setSection(id)}
              style={{
                justifyContent: "flex-start",
                background: section === id ? "var(--tervin-raised)" : "transparent",
                color: section === id ? "var(--tervin-ink)" : "var(--tervin-muted)",
              }}
            >
              {label}
            </button>
          ))}
          <div className="grow" />
          <button className="btn" onClick={() => s.setSettings(false)}>
            Close
          </button>
        </nav>

        <div className="grow" style={{ overflow: "auto", padding: "var(--sp-4)", minHeight: 0 }}>
          {section === "appearance" && <AppearanceSection />}
          {section === "shell" && <ShellSection />}
          {section === "agents" && <AgentsSection />}
          {section === "rules" && <RulesSection />}
          {section === "about" && <AboutSection />}
        </div>
      </div>
    </div>
  );
}

function AppearanceSection() {
  const s = useWorkspace();
  const a = s.appearance;

  return (
    <div className="col" style={{ gap: "var(--sp-5)" }}>
      <Field label="Theme" hint={`${THEMES.length} themes. Each carries a full 16-colour terminal palette, so prompt frameworks like powerlevel10k and starship render correctly.`}>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(196px, 1fr))", gap: "var(--sp-2)" }}>
          {THEMES.map((t) => (
            <button
              key={t.id}
              onClick={() => s.setAppearance({ themeId: t.id })}
              className="col"
              title={t.note}
              style={{
                border: `1px solid ${a.themeId === t.id ? "var(--tervin-accent)" : "var(--tervin-line)"}`,
                borderRadius: "var(--radius-md)",
                overflow: "hidden",
                textAlign: "left",
              }}
            >
              {/* A real preview: the theme's own surfaces and ANSI colours. */}
              <div style={{ background: t.surface.terminalBg, padding: "var(--sp-2)" }}>
                <div className="row" style={{ gap: 3, marginBottom: 6 }}>
                  {[t.ansi.red, t.ansi.green, t.ansi.yellow, t.ansi.blue, t.ansi.magenta, t.ansi.cyan].map(
                    (c) => (
                      <span key={c} style={{ width: 10, height: 10, borderRadius: 2, background: c }} />
                    ),
                  )}
                </div>
                <div
                  className="mono"
                  style={{ color: t.surface.terminalFg, fontSize: 11, whiteSpace: "nowrap", overflow: "hidden" }}
                >
                  <span style={{ color: t.ansi.brightCyan }}>~/proj</span>{" "}
                  <span style={{ color: t.ansi.brightGreen }}>main</span>{" "}
                  <span style={{ color: t.surface.accent }}>❯</span> cargo test
                </div>
              </div>
              <div className="row" style={{ padding: "var(--sp-1) var(--sp-2)", background: t.surface.panel }}>
                <span style={{ color: t.surface.ink, fontSize: "var(--text-meta)" }}>{t.name}</span>
                <div className="grow" />
                <span style={{ color: t.surface.muted, fontSize: 10 }}>{t.appearance}</span>
              </div>
            </button>
          ))}
        </div>
      </Field>

      <Field
        label="Terminal font"
        hint="A Nerd Font is needed for powerlevel10k and starship glyphs. Without one, prompts show replacement boxes — that is a missing font, not a broken prompt."
      >
        <input
          value={a.fontFamily}
          onChange={(e) => s.setAppearance({ fontFamily: e.target.value })}
          style={{ width: "100%" }}
          spellCheck={false}
        />
        <div className="row" style={{ marginTop: "var(--sp-2)", gap: "var(--sp-2)", flexWrap: "wrap" }}>
          {[
            "MesloLGS NF",
            "JetBrainsMono Nerd Font",
            "Berkeley Mono",
            "JetBrains Mono",
            "Iosevka",
            "Maple Mono",
            "SF Mono",
            "Menlo",
          ].map((font) => (
            <button
              key={font}
              className="btn"
              onClick={() => s.setAppearance({ fontFamily: `"${font}", ui-monospace, monospace` })}
              style={{ fontFamily: `"${font}", monospace` }}
            >
              {font}
            </button>
          ))}
        </div>
        <div
          className="mono"
          style={{
            marginTop: "var(--sp-2)",
            padding: "var(--sp-2)",
            border: "1px solid var(--tervin-line)",
            borderRadius: "var(--radius-sm)",
            background: "var(--tervin-terminal-bg)",
            fontFamily: a.fontFamily,
            fontSize: a.fontSize,
            lineHeight: a.lineHeight,
            fontVariantLigatures: a.ligatures ? "common-ligatures" : "none",
          }}
        >
          {"0Oo1lI |=> != >= <= --> ~/proj       日本語 🚀"}
        </div>
        <div className="meta" style={{ marginTop: "var(--sp-1)" }}>
          If the four glyphs after the arrows show as boxes, the selected font is
          not a Nerd Font.
        </div>
      </Field>

      <div className="row" style={{ gap: "var(--sp-5)", flexWrap: "wrap", alignItems: "flex-start" }}>
        <Field label={`Size · ${a.fontSize}px`}>
          <input
            type="range"
            min={9}
            max={24}
            value={a.fontSize}
            onChange={(e) => s.setAppearance({ fontSize: Number(e.target.value) })}
          />
        </Field>
        <Field label={`Line height · ${a.lineHeight.toFixed(2)}`}>
          <input
            type="range"
            min={1}
            max={2}
            step={0.05}
            value={a.lineHeight}
            onChange={(e) => s.setAppearance({ lineHeight: Number(e.target.value) })}
          />
        </Field>
        <Field label="Cursor">
          <select
            value={a.cursorStyle}
            onChange={(e) => s.setAppearance({ cursorStyle: e.target.value as typeof a.cursorStyle })}
          >
            <option value="block">Block</option>
            <option value="underline">Underline</option>
            <option value="bar">Bar</option>
          </select>
        </Field>
      </div>

      <div className="col" style={{ gap: "var(--sp-2)" }}>
        <Toggle
          checked={a.ligatures}
          onChange={(ligatures) => s.setAppearance({ ligatures })}
          label="Font ligatures"
          hint="Renders => and != as single glyphs, where the font provides them."
        />
        <Toggle
          checked={a.cursorBlink}
          onChange={(cursorBlink) => s.setAppearance({ cursorBlink })}
          label="Blinking cursor"
        />
        <Toggle
          checked={a.copyOnSelect}
          onChange={(copyOnSelect) => s.setAppearance({ copyOnSelect })}
          label="Copy on select"
        />
      </div>

      <Field
        label="Reopen last session"
        hint="Tabs, splits, each pane's directory and its recent output come back on launch. The processes do not — they exited with the app — so each pane starts a fresh shell below its old output, and says so. Recent output is held in the same local database as Blocks, ages out on the same retention window, and is deleted as soon as this is switched off."
      >
        <Toggle
          checked={a.restoreSession}
          onChange={(restoreSession) => {
            s.setAppearance({ restoreSession });
            // Said now rather than discovered later: switching this off deletes what was
            // saved, and that is not something to find out afterwards.
            if (!restoreSession) {
              s.pushNotice(
                "Saved layout and terminal output have been deleted. New sessions will not be saved.",
              );
            }
          }}
          label={a.restoreSession ? "Reopening the last session" : "Starting with one empty pane"}
        />
      </Field>

      <Field
        label="Layout"
        hint="Where the tab strip and the file explorer live. A vertical tab strip is the only arrangement that stays readable with twenty tabs open, which is why all four sides are offered rather than just top and bottom."
      >
        <div className="row" style={{ gap: "var(--sp-2)", flexWrap: "wrap" }}>
          <span className="meta" style={{ width: 90 }}>Tabs</span>
          {(["top", "bottom", "left", "right"] as const).map((pos) => (
            <button
              key={pos}
              className="btn"
              onClick={() => s.setAppearance({ tabBarPosition: pos })}
              style={{
                borderColor: a.tabBarPosition === pos ? "var(--tervin-accent)" : undefined,
                color: a.tabBarPosition === pos ? "var(--tervin-accent)" : undefined,
              }}
            >
              {pos}
            </button>
          ))}
        </div>

        <div className="row" style={{ gap: "var(--sp-2)", marginTop: "var(--sp-2)", flexWrap: "wrap" }}>
          <span className="meta" style={{ width: 90 }}>The + button</span>
          {(
            [
              ["tab", "New tab"],
              ["pane", "New pane"],
            ] as const
          ).map(([action, label]) => (
            <button
              key={action}
              className="btn"
              onClick={() => s.setAppearance({ newButtonAction: action })}
              title={
                action === "tab"
                  ? "A new tab, with a shell in it"
                  : "Split the focused pane instead of opening a tab"
              }
              style={{
                borderColor:
                  a.newButtonAction === action ? "var(--tervin-accent)" : undefined,
                color: a.newButtonAction === action ? "var(--tervin-accent)" : undefined,
              }}
            >
              {label}
            </button>
          ))}
        </div>

        <div className="row" style={{ gap: "var(--sp-2)", marginTop: "var(--sp-2)", flexWrap: "wrap" }}>
          <span className="meta" style={{ width: 90 }}>File explorer</span>
          <Toggle
            checked={a.explorerVisible}
            onChange={(explorerVisible) => s.setAppearance({ explorerVisible })}
            label="Show"
          />
          {a.explorerVisible &&
            (["left", "right"] as const).map((side) => (
              <button
                key={side}
                className="btn"
                onClick={() => s.setAppearance({ explorerSide: side })}
                style={{
                  borderColor: a.explorerSide === side ? "var(--tervin-accent)" : undefined,
                  color: a.explorerSide === side ? "var(--tervin-accent)" : undefined,
                }}
              >
                {side}
              </button>
            ))}
        </div>
        <div className="meta" style={{ marginTop: "var(--sp-2)", textWrap: "pretty" }}>
          {/* Says what a click does, because "inserts the path" is not the guess most
              people would make of a file tree. */}
          Clicking a file in the explorer types its path into the focused pane rather
          than opening an editor — this is a terminal, and a path is usually wanted
          inside a command.
        </div>
      </Field>

      <Field
        label="Prompt editing"
        hint="How the agent prompt box behaves. This affects the composer only — vim, emacs, and everything else in a terminal pane always get every keystroke untouched. ⌘⏎ sends in every mode; plain ⏎ is a newline, because a prompt is usually several lines."
      >
        <div className="row" style={{ gap: "var(--sp-2)", flexWrap: "wrap" }}>
          {(
            [
              ["native", "Native", "Exactly what every other text field on this system does."],
              ["emacs", "Emacs / readline", "C-a, C-e, C-k, C-u, C-w, C-y, M-b, M-f — what your shell already does."],
              ["vim", "Vim", "Normal and insert modes, with the motions and operators that carry the weight."],
            ] as const
          ).map(([mode, label, hint]) => (
            <button
              key={mode}
              className="btn"
              title={hint}
              onClick={() => s.setAppearance({ composerMode: mode })}
              style={{
                borderColor:
                  a.composerMode === mode ? "var(--tervin-accent)" : undefined,
                color: a.composerMode === mode ? "var(--tervin-accent)" : undefined,
              }}
            >
              {label}
            </button>
          ))}
        </div>
        <div className="meta" style={{ marginTop: "var(--sp-2)", textWrap: "pretty" }}>
          {/* Default is native on purpose: guessing wrong makes a text box eat
              keystrokes, which is worse than not guessing. */}
          {a.composerMode === "vim"
            ? "Escape leaves insert mode. The composer shows NORMAL or INSERT, because an invisible modal state is worse than no modal editing."
            : a.composerMode === "emacs"
              ? "Unrecognised keys still reach the platform, so dead keys and IME input keep working."
              : "No bindings are claimed. Pick a mode if your hands expect one."}
        </div>
      </Field>
    </div>
  );
}

function ShellSection() {
  const s = useWorkspace();
  const env = s.environment;
  const [busy, setBusy] = useState<string | null>(null);

  async function install(shell: string) {
    setBusy(shell);
    try {
      await api.shellIntegrationInstall(shell);
      await s.refreshEnvironment();
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="col" style={{ gap: "var(--sp-5)" }}>
      <Field
        label="Shell integration"
        hint="Optional. Tervin works without it, but with it every command becomes a Block carrying its exact text, exit status, duration, and working directory. Tervin only ever appends one fenced line, and backs the file up first."
      >
        {(env?.integration ?? []).map((status) => (
          <div
            key={status.shell}
            className="row"
            style={{
              padding: "var(--sp-2)",
              borderBottom: "1px solid var(--tervin-line)",
              gap: "var(--sp-2)",
            }}
          >
            <span className={`dot ${status.installed ? "dot-green" : "dot-muted"}`} />
            <span style={{ width: 96 }}>{status.shell}</span>
            <code className="mono meta truncate grow selectable" title={status.proposed_line}>
              {status.installed ? status.rc_path : status.proposed_line}
            </code>
            {status.installed ? (
              <button
                className="btn"
                disabled={busy === status.shell}
                onClick={() =>
                  void api
                    .shellIntegrationUninstall(status.shell)
                    .then(() => s.refreshEnvironment())
                    .catch((e) => s.pushNotice(describeError(e)))
                }
              >
                Remove
              </button>
            ) : (
              <button className="btn btn-primary" disabled={busy === status.shell} onClick={() => void install(status.shell)}>
                Install
              </button>
            )}
          </div>
        ))}
      </Field>

      <Field
        label="Aliases and functions"
        hint="Read from your shell so Tervin can expand an alias before classifying its risk. An alias like deploy='kubectl apply --context prod' is only dangerous once expanded, so Tervin expands first and shows you what will actually run."
      >
        <div className="row" style={{ gap: "var(--sp-2)", marginBottom: "var(--sp-2)" }}>
          <span className="meta tabular">
            {Object.keys(env?.aliases.aliases ?? {}).length} aliases ·{" "}
            {(env?.aliases.functions ?? []).length} functions
          </span>
          <div className="grow" />
          <button
            className="btn"
            onClick={() => void api.aliasesReload().then(() => s.refreshEnvironment())}
          >
            Reload from shell
          </button>
        </div>
        {(env?.aliases.notes ?? []).map((note) => (
          <div key={note} className="meta tone-amber" style={{ marginBottom: "var(--sp-1)" }}>
            {note}
          </div>
        ))}
        {/* "No aliases" and "could not check" are the same empty list otherwise — and
            they are not the same thing, because this is how a second agent account gets
            discovered. */}
        {env?.aliases && !env.aliases.enumerated && (
          <div className="meta tone-amber" style={{ marginBottom: "var(--sp-1)" }}>
            Tervin could not read your aliases, so any agent accounts defined by one were
            not offered. This is not the same as having none.
          </div>
        )}
        {env?.aliases?.enumerated &&
          Object.keys(env.aliases.aliases).length === 0 && (
            <div className="meta" style={{ marginBottom: "var(--sp-1)" }}>
              Your shell reported no aliases.
            </div>
          )}
        <div style={{ maxHeight: 220, overflow: "auto", border: "1px solid var(--tervin-line)", borderRadius: "var(--radius-sm)" }}>
          {Object.entries(env?.aliases.aliases ?? {}).map(([name, expansion]) => (
            <div key={name} className="row" style={{ padding: "2px var(--sp-2)", gap: "var(--sp-2)" }}>
              <code className="mono" style={{ width: 150, flex: "none", fontSize: "var(--text-meta)" }}>
                {name}
              </code>
              <code className="mono meta truncate grow selectable">{expansion}</code>
            </div>
          ))}
        </div>
      </Field>
    </div>
  );
}

function AgentsSection() {
  const s = useWorkspace();
  const agents = s.agents;
  // Arrives after the profiles above, and may never arrive at all. Everything read
  // from it therefore has to render sensibly while it is still null.
  const discovery = s.agentsDiscovery;

  return (
    <div className="col" style={{ gap: "var(--sp-5)" }}>
      <Field
        label="What this project already tells agents"
        hint="Other tools write instructions into a repository, and Tervin reads them rather than asking you to repeat yourself. The useful part is the right-hand column: whether the runtime you pick will actually obey each file. The same CLAUDE.md is in force for Claude Code and ignored by Codex, so one undifferentiated list would let you believe a file is governing an agent that has never seen it."
      >
        <ProjectInstructions />
      </Field>

      <Field
        label="Agent profiles"
        hint="One profile per install or account. Shell aliases cannot be used here: Tervin launches agents as direct child processes, so their environment is set explicitly. A profile fully determines which account runs — an ambient CLAUDE_CONFIG_DIR is cleared, never inherited."
      >
        {(agents?.profiles ?? []).map((p) => (
          <div
            key={p.id}
            className="row"
            style={{ padding: "var(--sp-2)", borderBottom: "1px solid var(--tervin-line)", gap: "var(--sp-2)" }}
          >
            <input
              type="radio"
              name="default-profile"
              checked={agents?.default_profile === p.id}
              onChange={() =>
                void api
                  .agentsSaveProfiles(agents?.profiles ?? [], p.id)
                  .then(() => s.refreshAgents())
                  .catch((e) => s.pushNotice(describeError(e)))
              }
              title="Use as the default"
            />
            <span style={{ width: 150 }}>{p.name}</span>
            <code className="mono meta truncate grow">
              {[
                ...Object.entries(p.env).map(([k, v]) => `${k}=${v}`),
                // Named, never valued. There is no value to print: it is read from
                // the environment at launch and is not written to agents.toml.
                ...(p.secrets_from_env ?? []).map((k) => `${k}=<from environment>`),
              ].join(" ") || p.binary}
            </code>
            {p.sensitive && <span className="chip tone-amber">shared account</span>}
          </div>
        ))}
        {(agents?.profiles ?? []).some((p) => (p.secrets_from_env ?? []).length > 0) && (
          <div className="meta" style={{ marginTop: "var(--sp-2)" }}>
            A variable shown as <code className="mono">&lt;from environment&gt;</code> is read
            from the environment Tervin was launched in, each time a Thread starts. Tervin does
            not store its value. If it is not set there, the Thread does not start and says
            which variable is missing.
          </div>
        )}
        <div className="meta" style={{ marginTop: "var(--sp-2)" }}>
          {/* The real path, from the backend: it differs by platform, and naming the
              wrong one sends the user looking somewhere that does not exist. */}
          Profiles live in <code className="mono">{agents?.profiles_path ?? "the Tervin config directory"}</code>.
          MCP servers for ACP agents live in{" "}
          <code className="mono">{agents?.mcp_path ?? "mcp.json"}</code>.
        </div>
      </Field>

      {(discovery?.import_candidates.length ?? 0) > 0 && (
        <Field
          label="Found on this machine"
          hint="Tervin read these from your shell aliases and config directories. Nothing is adopted automatically — adopting a profile decides which account an agent runs as."
        >
          {discovery!.import_candidates.map((c) => (
            <div
              key={c.profile.id}
              className="row"
              style={{ padding: "var(--sp-2)", borderBottom: "1px solid var(--tervin-line)", gap: "var(--sp-2)" }}
            >
              <span style={{ width: 150 }}>{c.profile.name}</span>
              <span className="meta truncate grow">{c.source}</span>
              <button
                className="btn btn-primary"
                onClick={() =>
                  void api
                    .agentsSaveProfiles(
                      [...(agents?.profiles ?? []), c.profile],
                      agents?.default_profile ?? null,
                    )
                    .then(() => s.refreshAgents())
                    .catch((e) => s.pushNotice(describeError(e)))
                }
              >
                Add profile
              </button>
            </div>
          ))}
        </Field>
      )}

      <AddAcpAgent />

      <AddLocalModel />

      <Field label="Discovered runtimes" hint="Every agent Tervin recognises, whether or not it has a deep adapter.">
        {/* Said rather than shown as an empty list: "nothing installed" and "not
            finished looking" are different answers, and only one of them is news. */}
        {discovery === null && (
          <div className="meta" style={{ padding: "var(--sp-2)" }}>
            Looking for installed agents…
          </div>
        )}
        {(discovery?.discovered ?? []).map((d) => (
          <div key={d.runtime_id} style={{ padding: "var(--sp-2)", borderBottom: "1px solid var(--tervin-line)" }}>
            <div className="row" style={{ gap: "var(--sp-2)" }}>
              <span className={`dot ${d.available ? "dot-green" : "dot-muted"}`} />
              <span style={{ width: 150 }}>{d.display_name}</span>
              <span className="meta tabular">{d.version ?? ""}</span>
              <div className="grow" />
              {/* The one capability worth calling out in a list: whether a "Deny"
                  here actually stops anything. */}
              {d.capabilities.native_permission_bridge.level === "supported" && (
                <span className="chip tone-green" title="This runtime asks Tervin before acting, so Tervin Rules decide.">
                  Tervin gates
                </span>
              )}
              <span className="chip">{tierLabel(d.capabilities.tier)}</span>
            </div>
            {d.notes.map((note) => (
              <div key={note} className="meta" style={{ paddingLeft: 22, marginTop: 2 }}>
                {note}
              </div>
            ))}
          </div>
        ))}
      </Field>
    </div>
  );
}

/**
 * Register a model endpoint.
 *
 * Separate from adding an agent because it is a different kind of thing: a model
 * answers questions about the workspace and can carry context between agents, but it
 * cannot run a command or change a file. Presenting the two in one form would invite
 * the assumption that they are interchangeable.
 */
function AddLocalModel() {
  const s = useWorkspace();
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  const canAdd = name.trim().length > 0 && url.trim().length > 0 && !busy;

  async function add() {
    setBusy(true);
    setResult(null);
    try {
      const discovery = await api.agentsAddLocalModel(
        name.trim(),
        url.trim(),
        key.trim() || null,
      );
      // The notes already say what was found or why nothing answered, so they are
      // shown verbatim rather than reworded into something vaguer.
      setResult(discovery.notes.join(" "));
      if (discovery.available) {
        setName("");
        setUrl("");
        setKey("");
      }
      await s.refreshAgents();
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Field
      label="Add a model endpoint"
      hint="Anything speaking the OpenAI dialect: LM Studio, Ollama, vLLM, llama.cpp, or a remote server. A model answers questions about your work and carries context between agents — it cannot run commands or edit files. `/v1` is added if you leave it off."
    >
      <div className="row" style={{ gap: "var(--sp-2)", flexWrap: "wrap" }}>
        <input
          style={{ width: 150 }}
          placeholder="Name"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="mono grow"
          style={{ minWidth: 200 }}
          placeholder="http://127.0.0.1:1234"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          spellCheck={false}
        />
        <input
          type="password"
          style={{ width: 140 }}
          placeholder="API key (optional)"
          value={key}
          onChange={(e) => setKey(e.target.value)}
          spellCheck={false}
        />
        <button className="btn btn-primary" disabled={!canAdd} onClick={() => void add()}>
          {busy ? "Checking…" : "Add"}
        </button>
      </div>
      {/* A password field implies somewhere to keep the password. There is nowhere:
          `agents_add_local_model` registers the endpoint in the running app and
          creates no profile, so nothing here survives a restart. Saying so is the
          honest half of a decision made on purpose — storing it would mean writing a
          credential to disk, which is the thing agents.toml stopped doing. */}
      <div className="meta" style={{ marginTop: "var(--sp-2)", textWrap: "pretty" }}>
        The address and key are held for this session only. Tervin writes neither to
        disk, so both have to be entered again after a restart.
      </div>
      {result && (
        <div className="meta" style={{ marginTop: "var(--sp-2)", textWrap: "pretty" }}>
          {result}
        </div>
      )}
    </Field>
  );
}

/**
 * Register an agent Tervin has never heard of.
 *
 * This exists because Tervin integrates with a protocol rather than with vendors:
 * anything that speaks ACP gets plans, tool events, and a real permission gate from
 * a command line typed here. No release required, no adapter written.
 */
function AddAcpAgent() {
  const s = useWorkspace();
  const [name, setName] = useState("");
  const [command, setCommand] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  // The command is typed as one line, the way it would be run in a shell.
  const parts = command.trim().split(/\s+/).filter(Boolean);
  const canAdd = name.trim().length > 0 && parts.length > 0 && !busy;

  async function add() {
    setBusy(true);
    setResult(null);
    try {
      const discovery = await api.agentsAddAcp(name.trim(), parts[0]!, parts.slice(1));
      setResult(
        discovery.available
          ? `Added ${discovery.display_name}${discovery.version ? ` (${discovery.version})` : ""}.`
          : // Registered but not runnable: say so now rather than at launch.
            `Added, but \`${parts[0]}\` was not found on PATH. Threads will fail to start until it is.`,
      );
      setName("");
      setCommand("");
      await s.refreshAgents();
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Field
      label="Add an ACP agent"
      hint="Any agent that speaks the Agent Client Protocol works here as a full structured integration — including a real permission gate, because the agent asks Tervin before it acts. Give the command that starts it in ACP mode; over 25 agents support it, and the list above will always lag behind. Examples: `gemini --experimental-acp`, `copilot --acp`, `claude-code-acp`."
    >
      <div className="row" style={{ gap: "var(--sp-2)", flexWrap: "wrap" }}>
        <input
          style={{ width: 150 }}
          placeholder="Name"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="mono grow"
          style={{ minWidth: 220 }}
          placeholder="gemini --experimental-acp"
          value={command}
          onChange={(e) => setCommand(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && canAdd) void add();
          }}
        />
        <button className="btn btn-primary" disabled={!canAdd} onClick={() => void add()}>
          {busy ? "Adding…" : "Add"}
        </button>
      </div>
      {result && (
        <div className="meta" style={{ marginTop: "var(--sp-2)", textWrap: "pretty" }}>
          {result}
        </div>
      )}
    </Field>
  );
}

function RulesSection() {
  const [rules, setRules] = useState<api.PolicyRule[]>([]);
  const [audit, setAudit] = useState<api.AuditRecord[]>([]);

  useEffect(() => {
    void api.rulesList().then(setRules).catch(() => {});
    void api.auditRecent(60).then(setAudit).catch(() => {});
  }, []);

  return (
    <div className="col" style={{ gap: "var(--sp-5)" }}>
      <Field
        label="Policy rules"
        hint="Tervin owns approvals, whichever agent is acting. An approval is keyed to the exact action: approving `rm -rf build` never approves `rm -rf /`."
      >
        {rules.map((r) => (
          <div key={r.id} className="row" style={{ padding: "var(--sp-2)", borderBottom: "1px solid var(--tervin-line)", gap: "var(--sp-2)" }}>
            <span className={`chip tone-${r.effect === "deny" ? "red" : r.effect === "allow" ? "green" : "amber"}`}>
              {r.effect.replace(/_/g, " ")}
            </span>
            <span style={{ width: 170 }}>{r.name}</span>
            <span className="meta truncate grow">{r.reason}</span>
          </div>
        ))}
      </Field>

      <Field label="Audit log" hint="Append-only. Records what was requested, what was decided, by whom, and what ran.">
        <div style={{ maxHeight: 260, overflow: "auto", border: "1px solid var(--tervin-line)", borderRadius: "var(--radius-sm)" }}>
          {audit.length === 0 ? (
            <div className="empty">Nothing recorded yet.</div>
          ) : (
            audit.map((r) => (
              <div key={r.id} className="row meta" style={{ padding: "2px var(--sp-2)", gap: "var(--sp-2)" }}>
                <span className="tabular" style={{ width: 132, flex: "none" }}>
                  {new Date(r.ts).toLocaleString()}
                </span>
                <span style={{ width: 80, flex: "none" }}>{r.actor}</span>
                <span style={{ width: 68, flex: "none" }}>{r.phase}</span>
                <span
                  style={{ width: 62, flex: "none" }}
                  className={r.decision === "denied" ? "tone-red" : r.decision === "allowed" ? "tone-green" : ""}
                >
                  {r.decision ?? ""}
                </span>
                <code className="mono truncate grow selectable">{r.action}</code>
              </div>
            ))
          )}
        </div>
      </Field>
    </div>
  );
}

function AboutSection() {
  const s = useWorkspace();
  return (
    <div className="col" style={{ gap: "var(--sp-4)" }}>
      <div>
        <div style={{ fontSize: "var(--text-heading)", fontWeight: 600 }}>Tervin</div>
        <div className="meta">The agent-native terminal workspace.</div>
      </div>
      <Field label="Local-first" hint="Nothing leaves this machine unless you run an agent or integration that needs it.">
        <div className="meta col" style={{ gap: 2 }}>
          <span>Workspace database, Blocks, events, and audit log are stored locally.</span>
          <span>No telemetry. No cloud sync. No account.</span>
          <span className="mono selectable">{s.environment?.home}/.local/share/tervin</span>
        </div>
      </Field>
      <Field label="Keyboard">
        <div className="meta" style={{ display: "grid", gridTemplateColumns: "auto 1fr", gap: "2px var(--sp-3)" }}>
          {[
            ["⌘K", "Command palette"],
            ["⌘T", "New pane"],
            ["⌘D / ⇧⌘D", "Split horizontally / vertically"],
            ["⌘I", "Toggle inspector"],
            ["⌘B", "Toggle activity rail"],
            ["⇧⌘F", "Search"],
            ["⌘,", "Settings"],
          ].map(([key, label]) => (
            <div key={key} style={{ display: "contents" }}>
              <kbd className="mono">{key}</kbd>
              <span>{label}</span>
            </div>
          ))}
        </div>
      </Field>
    </div>
  );
}

// ------------------------------------------------------------------ helpers

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="col" style={{ gap: "var(--sp-2)" }}>
      <div>
        <div style={{ fontWeight: 600, fontSize: "var(--text-title)" }}>{label}</div>
        {hint && <div className="meta" style={{ maxWidth: "68ch", marginTop: 2 }}>{hint}</div>}
      </div>
      {children}
    </div>
  );
}

function Toggle({
  checked,
  onChange,
  label,
  hint,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label: string;
  hint?: string;
}) {
  return (
    <label className="row" style={{ gap: "var(--sp-2)", cursor: "pointer" }}>
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span>{label}</span>
      {hint && <span className="meta">— {hint}</span>}
    </label>
  );
}

function tierLabel(tier: api.Capabilities["tier"]): string {
  switch (tier) {
    case "structured":
      return "Tier 1 · Structured";
    case "enhanced_cli":
      return "Tier 2 · Enhanced CLI";
    // Not a rung on the same ladder: a model endpoint answers and cannot act, so
    // numbering it would imply it is a worse agent rather than a different thing.
    case "conversational":
      return "Answers · cannot act";
    default:
      return "Tier 3 · Generic terminal";
  }
}
