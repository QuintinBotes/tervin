/**
 * Commands worth keeping, with the varying parts named.
 *
 * Half the commands anyone runs are a shape with one thing changed: deploy to *this*
 * environment, tail *that* service, reset *this* branch. Shell history gives you the last
 * one you happened to type, which is the wrong instance of the shape more often than not
 * — so people keep a scratch file of commands and copy out of it. This is that file, with
 * the holes made explicit:
 *
 * ```text
 * kubectl logs -f {{service}} --namespace {{env:staging}}
 * ```
 *
 * ## It fills in, it does not run
 *
 * Accepting a command writes it into the pane and leaves the newline to the user — the
 * same rule as the directory picker, and it matters more here: a saved command is often
 * the destructive kind, and seeing the filled-in line before sending it is the whole
 * safeguard. A picker that ran `deploy prod` on Enter would be a different and much worse
 * product.
 *
 * ## Parsing happens in Rust
 *
 * What counts as a hole is decided in one place. A second implementation here would
 * eventually disagree with the backend about `${HOME}` or `awk '{print $1}'`, and the
 * disagreement would show up as a corrupted command.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as api from "../lib/api";
import { describeError, useWorkspace } from "../lib/store";
import { writeToPane } from "./TerminalPane";

type Mode = { kind: "list" } | { kind: "fill"; command: api.SavedCommandView } | { kind: "new" };

export function SavedCommands() {
  const s = useWorkspace();
  const [commands, setCommands] = useState<api.SavedCommandView[]>([]);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const [mode, setMode] = useState<Mode>({ kind: "list" });
  const inputRef = useRef<HTMLInputElement | null>(null);

  const close = useCallback(() => s.setSavedCommands(false), [s]);

  const load = useCallback(() => {
    api
      .savedCommands()
      .then(setCommands)
      .catch((e) => s.pushNotice(describeError(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    load();
    inputRef.current?.focus();
  }, [load]);

  // Filtered here rather than in the backend: the list is small, and filtering locally
  // keeps typing instant.
  const shown = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return commands;
    return commands.filter((c) =>
      `${c.name} ${c.template} ${c.description ?? ""}`.toLowerCase().includes(needle),
    );
  }, [commands, query]);

  useEffect(() => setSelected(0), [query]);

  function accept(view: api.SavedCommandView) {
    // A command with holes needs them filled first; one without goes straight in.
    if (view.parameters.length > 0) {
      setMode({ kind: "fill", command: view });
      return;
    }
    void send(view, []);
  }

  async function send(view: api.SavedCommandView, values: [string, string][]) {
    const tab = s.tabs.find((t) => t.id === s.activeTabId);
    const paneId = tab?.activePaneId;
    if (!paneId) {
      s.pushNotice("There is no pane to send a command to.");
      return;
    }
    try {
      const line = await api.savedCommandRender(view.id, view.template, values);
      // Typed, not run. The newline is the user's to send.
      writeToPane(paneId, line);
      close();
    } catch (e) {
      s.pushNotice(describeError(e));
    }
  }

  if (mode.kind === "fill") {
    return (
      <Overlay onClose={close} label={`Fill in ${mode.command.name}`}>
        <FillForm
          command={mode.command}
          onCancel={() => setMode({ kind: "list" })}
          onSubmit={(values) => void send(mode.command, values)}
        />
      </Overlay>
    );
  }

  if (mode.kind === "new") {
    return (
      <Overlay onClose={close} label="Save a command">
        <NewForm
          onCancel={() => setMode({ kind: "list" })}
          onSaved={() => {
            setMode({ kind: "list" });
            load();
          }}
        />
      </Overlay>
    );
  }

  return (
    <Overlay onClose={close} label="Saved commands">
      <input
        ref={inputRef}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            setSelected((i) => Math.min(i + 1, Math.max(0, shown.length - 1)));
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setSelected((i) => Math.max(0, i - 1));
          } else if (e.key === "Enter") {
            e.preventDefault();
            const hit = shown[selected];
            if (hit) accept(hit);
          } else if (e.key === "Escape") {
            e.preventDefault();
            close();
          }
        }}
        placeholder="Search saved commands"
        aria-label="Search saved commands"
        spellCheck={false}
        style={{
          border: "none",
          borderBottom: "1px solid var(--tervin-line)",
          borderRadius: 0,
          padding: "var(--sp-3)",
          fontSize: "var(--text-body)",
          background: "transparent",
        }}
      />

      <div className="grow" style={{ overflow: "auto", minHeight: 0 }}>
        {shown.length === 0 ? (
          <div className="empty">
            {query ? (
              <>Nothing saved matches “{query}”.</>
            ) : (
              <>
                Nothing saved yet. A saved command is a command with the parts that change
                named — <code>{"deploy {{env:staging}}"}</code> — so you fill in the one
                thing rather than retyping the line.
              </>
            )}
          </div>
        ) : (
          shown.map((view, i) => (
            <div
              key={view.id}
              className="block-row"
              role="button"
              tabIndex={-1}
              aria-selected={i === selected}
              onMouseEnter={() => setSelected(i)}
              onClick={() => accept(view)}
              style={{
                cursor: "pointer",
                background: i === selected ? "var(--tervin-hover)" : undefined,
                borderLeft:
                  i === selected ? "2px solid var(--tervin-accent)" : "2px solid transparent",
              }}
            >
              <div className="row" style={{ padding: "6px 12px", gap: "var(--sp-2)" }}>
                <span style={{ fontSize: "var(--text-body)", flex: "none" }}>{view.name}</span>
                <span className="meta mono truncate grow" title={view.template}>
                  {view.template}
                </span>
                {view.parameters.length > 0 && (
                  <span
                    className="chip"
                    title={view.parameters.map((p) => p.name).join(", ")}
                  >
                    {view.parameters.length} to fill
                  </span>
                )}
                <button
                  className="btn btn-xs btn-ghost"
                  title="Forget this command"
                  onClick={(e) => {
                    e.stopPropagation();
                    void api
                      .savedCommandDelete(view.id)
                      .then(load)
                      .catch((err) => s.pushNotice(describeError(err)));
                  }}
                >
                  Forget
                </button>
              </div>
              {view.description && (
                <div className="meta" style={{ padding: "0 12px 6px 12px" }}>
                  {view.description}
                </div>
              )}
            </div>
          ))
        )}
      </div>

      <div
        className="row"
        style={{
          flex: "none",
          padding: "var(--sp-2) var(--sp-3)",
          borderTop: "1px solid var(--tervin-line)",
          gap: "var(--sp-2)",
        }}
      >
        <span className="meta grow">
          Enter fills the command into the pane; it does not run it.
        </span>
        <button className="btn btn-xs" onClick={() => setMode({ kind: "new" })}>
          Save a command
        </button>
        <button className="btn btn-xs" onClick={close}>
          Close
        </button>
      </div>
    </Overlay>
  );
}

/** The shared shell, so the three states cannot drift apart visually. */
function Overlay({
  children,
  onClose,
  label,
}: {
  children: React.ReactNode;
  onClose: () => void;
  label: string;
}) {
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={label}
      onClick={onClose}
      style={{
        position: "fixed",
        inset: 0,
        background: "color-mix(in srgb, var(--tervin-bg) 70%, transparent)",
        display: "grid",
        placeItems: "start center",
        paddingTop: "12vh",
        zIndex: 160,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="col"
        style={{
          width: "min(700px, 94vw)",
          maxHeight: "72vh",
          background: "var(--tervin-panel)",
          border: "1px solid var(--tervin-line)",
          borderRadius: "var(--radius-lg)",
          overflow: "hidden",
        }}
      >
        {children}
      </div>
    </div>
  );
}

/** One field per hole, defaults prefilled so the common case is one keystroke. */
function FillForm({
  command,
  onCancel,
  onSubmit,
}: {
  command: api.SavedCommandView;
  onCancel: () => void;
  onSubmit: (values: [string, string][]) => void;
}) {
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(command.parameters.map((p) => [p.name, p.default ?? ""])),
  );
  const firstRef = useRef<HTMLInputElement | null>(null);
  useEffect(() => firstRef.current?.focus(), []);

  const entries = command.parameters.map(
    (p) => [p.name, values[p.name] ?? ""] as [string, string],
  );
  // Shown live, because the point of the confirmation step is seeing the actual line.
  const preview = command.parameters.reduce(
    (line, p) => line.replaceAll(holeOf(p), values[p.name] || holeOf(p)),
    command.template,
  );
  const unfilled = command.parameters.filter((p) => !values[p.name]);

  return (
    <>
      <div className="panel-header">
        <span className="label">{command.name}</span>
        <span className="meta truncate grow">{command.description ?? ""}</span>
      </div>

      <div className="col grow" style={{ padding: "var(--sp-3)", gap: "var(--sp-2)", overflow: "auto" }}>
        {command.parameters.map((p, i) => (
          <label key={p.name} className="col" style={{ gap: 4 }}>
            <span className="meta">{p.name}</span>
            <input
              ref={i === 0 ? firstRef : undefined}
              value={values[p.name] ?? ""}
              onChange={(e) => setValues((v) => ({ ...v, [p.name]: e.target.value }))}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  onSubmit(entries);
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  onCancel();
                }
              }}
              placeholder={p.default ?? ""}
              spellCheck={false}
            />
          </label>
        ))}

        <div className="col" style={{ gap: 4, marginTop: "var(--sp-2)" }}>
          <span className="meta">This is what will be typed</span>
          <pre
            className="mono selectable"
            style={{
              margin: 0,
              padding: "var(--sp-2)",
              background: "var(--tervin-bg)",
              border: "1px solid var(--tervin-line)",
              borderRadius: "var(--radius-md)",
              whiteSpace: "pre-wrap",
              wordBreak: "break-all",
              fontSize: "var(--text-meta)",
            }}
          >
            {preview}
          </pre>
          {unfilled.length > 0 && (
            // Stated rather than silently emptied. An unfilled hole stays visible in the
            // command so it fails loudly instead of running with an argument missing.
            <span className="meta tone-amber">
              {unfilled.map((p) => p.name).join(", ")} not filled in — left in the command
              rather than removed, so it will not run with an argument missing.
            </span>
          )}
        </div>
      </div>

      <div
        className="row"
        style={{
          flex: "none",
          padding: "var(--sp-2) var(--sp-3)",
          borderTop: "1px solid var(--tervin-line)",
          gap: "var(--sp-2)",
        }}
      >
        <span className="meta grow">It will be typed into the pane, not run.</span>
        <button className="btn btn-xs" onClick={onCancel}>
          Back
        </button>
        <button className="btn btn-xs" onClick={() => onSubmit(entries)}>
          Fill in the pane
        </button>
      </div>
    </>
  );
}

/** Saving a new command, with a live read-out of the holes it found. */
function NewForm({ onCancel, onSaved }: { onCancel: () => void; onSaved: () => void }) {
  const s = useWorkspace();
  const [name, setName] = useState("");
  const [template, setTemplate] = useState("");
  const [description, setDescription] = useState("");
  const nameRef = useRef<HTMLInputElement | null>(null);
  useEffect(() => nameRef.current?.focus(), []);

  // A rough count for feedback only. The backend's parser is authoritative, and the
  // saved command's real parameters come back from it — this is deliberately not a
  // second implementation of what a hole is.
  const holes = [...template.matchAll(/\{\{\s*([A-Za-z0-9_-]+)\s*(?::[^{}]*)?\}\}/g)].map(
    (m) => m[1]!,
  );
  const unique = [...new Set(holes)];

  function save() {
    void api
      .savedCommandUpsert(name, template, description)
      .then(onSaved)
      .catch((e) => s.pushNotice(describeError(e)));
  }

  return (
    <>
      <div className="panel-header">
        <span className="label">Save a command</span>
      </div>
      <div className="col grow" style={{ padding: "var(--sp-3)", gap: "var(--sp-2)", overflow: "auto" }}>
        <label className="col" style={{ gap: 4 }}>
          <span className="meta">Name</span>
          <input ref={nameRef} value={name} onChange={(e) => setName(e.target.value)} spellCheck={false} />
        </label>
        <label className="col" style={{ gap: 4 }}>
          <span className="meta">
            Command — name the parts that change with <code>{"{{like_this}}"}</code>, or{" "}
            <code>{"{{like_this:default}}"}</code>
          </span>
          <textarea
            value={template}
            onChange={(e) => setTemplate(e.target.value)}
            rows={3}
            spellCheck={false}
            className="mono"
          />
        </label>
        <label className="col" style={{ gap: 4 }}>
          <span className="meta">What it does (optional)</span>
          <input value={description} onChange={(e) => setDescription(e.target.value)} />
        </label>
        <span className="meta">
          {unique.length === 0
            ? "No parts to fill in — this will go straight into the pane."
            : `You will be asked for: ${unique.join(", ")}`}
        </span>
      </div>
      <div
        className="row"
        style={{
          flex: "none",
          padding: "var(--sp-2) var(--sp-3)",
          borderTop: "1px solid var(--tervin-line)",
          gap: "var(--sp-2)",
        }}
      >
        <div className="grow" />
        <button className="btn btn-xs" onClick={onCancel}>
          Back
        </button>
        <button className="btn btn-xs" onClick={save} disabled={!name.trim() || !template.trim()}>
          Save
        </button>
      </div>
    </>
  );
}

/** The hole as it appears in the template, for the live preview. */
function holeOf(p: api.SavedParameter): string {
  return p.default === null || p.default === undefined
    ? `{{${p.name}}}`
    : `{{${p.name}:${p.default}}}`;
}
