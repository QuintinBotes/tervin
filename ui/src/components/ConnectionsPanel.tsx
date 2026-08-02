/**
 * Connections: everything a pane can be attached to.
 *
 * The SSH list is read from the user's own `~/.ssh/config` rather than being a
 * second list to maintain. Tervin then launches the user's own `ssh`, so their
 * agent forwarding, jump hosts, credential helpers and `Match` rules all behave
 * exactly as they already do — see `session-manager` for why that is a deliberate
 * choice rather than a shortcut.
 *
 * Every row shows the command it will run before it runs. A connection is an
 * outward action, and "what is about to happen" should never be a surprise.
 */

import { useEffect, useState } from "react";
import * as api from "../lib/api";
import { describeError, useWorkspace } from "../lib/store";

export function ConnectionsPanel() {
  const s = useWorkspace();
  const [preview, setPreview] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  // Per host, and only when asked. A config can name a hundred machines across several
  // networks; probing them all on open would be a port scan of the user's own
  // infrastructure — slow, noisy in someone's logs, and a conversation with their
  // security team.
  const [reach, setReach] = useState<Record<string, api.Reachability>>({});
  const [checking, setChecking] = useState<string | null>(null);

  async function check(alias: string) {
    setChecking(alias);
    try {
      const found = await api.sshProbe(alias);
      setReach((prev) => ({ ...prev, [alias]: found }));
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setChecking(null);
    }
  }

  useEffect(() => {
    if (!s.connections) void s.refreshConnections();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const c = s.connections;

  /** Show what will run, before it runs. */
  async function showPreview(key: string, kind: api.SessionKind) {
    if (preview[key]) return;
    try {
      const spec = await api.connectionLaunchSpec(kind);
      setPreview((p) => ({
        ...p,
        [key]: [spec.program, ...spec.args].join(" "),
      }));
    } catch {
      // A preview is a nicety; failing to build one must not block connecting.
    }
  }

  async function open(kind: api.SessionKind, title: string) {
    setBusy(true);
    try {
      const spec = await api.connectionLaunchSpec(kind);
      s.addPane({
        id: crypto.randomUUID(),
        title,
        cwd: spec.cwd ?? s.environment?.project_root ?? ".",
        threadId: null,
        exited: false,
        exitCode: null,
        program: spec.program,
        args: spec.args,
        env: spec.env,
        remote: kind.kind === "ssh" || kind.kind === "serial",
      });
    } catch (e) {
      s.pushNotice(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  if (!c) {
    return <div className="empty">Looking for shells, hosts, sessions and devices…</div>;
  }

  return (
    <div className="col" style={{ minHeight: 0 }}>
      <Group
        label="Shells"
        hint="Your installed shells. Tervin runs them as login shells, so your prompt, aliases and plugins are untouched."
      >
        {c.shells.map((shell) => (
          <Row
            key={shell.program}
            title={shell.name}
            detail={shell.program}
            trailing={
              shell.supports_integration ? undefined : (
                <span className="chip" title="Tervin's shell hook does not support this shell">
                  no Blocks
                </span>
              )
            }
            preview={preview[`shell:${shell.id}`]}
            onHover={() => void showPreview(`shell:${shell.id}`, { kind: "shell", profile_id: shell.id })}
            onOpen={() => void open({ kind: "shell", profile_id: shell.id }, shell.name)}
            disabled={busy}
          />
        ))}
      </Group>

      <Group
        label="SSH hosts"
        hint={
          c.ssh_hosts.length > 0
            ? "Read from ~/.ssh/config. Tervin launches your own ssh, so jump hosts and agent forwarding work as configured."
            : "No hosts found in ~/.ssh/config."
        }
      >
        {c.ssh_warnings.map((warning) => (
          <div key={warning} className="meta tone-amber" style={{ padding: "var(--sp-2) var(--sp-6)" }}>
            {warning}
          </div>
        ))}
        {c.ssh_hosts.map((host) => (
          <Row
            key={host.alias}
            title={host.alias}
            detail={sshDetail(host)}
            trailing={
              <>
                {host.proxy_jump && (
                  <span className="chip" title={`Jumps via ${host.proxy_jump}`}>
                    via {host.proxy_jump}
                  </span>
                )}
                {host.identity_file && (
                  <span className="chip" title={`Uses ${host.identity_file}`}>
                    key
                  </span>
                )}
                <ReachabilityChip alias={host.alias} state={reach[host.alias]} />
                <button
                  className="btn btn-xs btn-ghost"
                  title="Check whether this host answers on its SSH port"
                  disabled={checking === host.alias}
                  onClick={(e) => {
                    e.stopPropagation();
                    void check(host.alias);
                  }}
                >
                  {checking === host.alias ? "checking…" : "check"}
                </button>
              </>
            }
            preview={preview[`ssh:${host.alias}`]}
            onHover={() => void showPreview(`ssh:${host.alias}`, { kind: "ssh", alias: host.alias })}
            onOpen={() => void open({ kind: "ssh", alias: host.alias }, host.alias)}
            disabled={busy}
          />
        ))}
      </Group>

      {c.multiplexers.length > 0 && (
        <Group
          label="tmux and zellij"
          hint="Existing sessions. Attaching reuses the session rather than starting a new one."
        >
          {c.multiplexers.map((session) => (
            <Row
              key={`${session.program}:${session.name}`}
              title={session.name}
              detail={`${session.program}${session.detail ? ` · ${session.detail}` : ""}`}
              trailing={session.attached ? <span className="chip chip-teal">attached</span> : undefined}
              preview={preview[`mux:${session.program}:${session.name}`]}
              onHover={() =>
                void showPreview(`mux:${session.program}:${session.name}`, {
                  kind: "multiplexer",
                  program: session.program,
                  session: session.name,
                })
              }
              onOpen={() =>
                void open(
                  { kind: "multiplexer", program: session.program, session: session.name },
                  session.name,
                )
              }
              disabled={busy}
            />
          ))}
        </Group>
      )}

      {c.serial.length > 0 && (
        <SerialGroup devices={c.serial} onOpen={open} busy={busy} />
      )}

      {c.wsl.length > 0 && (
        <Group label="WSL" hint="Windows Subsystem for Linux distributions.">
          {c.wsl.map((distro) => (
            <Row
              key={distro.name}
              title={distro.name}
              detail={distro.is_default ? "default distribution" : ""}
              preview={preview[`wsl:${distro.name}`]}
              onHover={() =>
                void showPreview(`wsl:${distro.name}`, {
                  kind: "wsl",
                  distribution: distro.name,
                })
              }
              onOpen={() => void open({ kind: "wsl", distribution: distro.name }, distro.name)}
              disabled={busy}
            />
          ))}
        </Group>
      )}

      <div className="row" style={{ padding: "var(--sp-5) var(--sp-6)" }}>
        <span className="meta grow">
          Secrets are never shown. Tervin records which identity file a host names,
          never its contents.
        </span>
        <button className="btn btn-sm" onClick={() => void s.refreshConnections()}>
          Rescan
        </button>
      </div>
    </div>
  );
}

/** Serial needs a baud rate, so it gets its own control rather than a plain row. */
function SerialGroup({
  devices,
  onOpen,
  busy,
}: {
  devices: api.SerialDevice[];
  onOpen: (kind: api.SessionKind, title: string) => void;
  busy: boolean;
}) {
  const [baud, setBaud] = useState(115200);

  return (
    <Group label="Serial" hint="Devices for embedded work.">
      <div className="row" style={{ padding: "var(--sp-2) var(--sp-6)" }}>
        <span className="meta">Baud</span>
        <select value={baud} onChange={(e) => setBaud(Number(e.target.value))} aria-label="Baud rate">
          {[9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600].map((rate) => (
            <option key={rate} value={rate}>
              {rate}
            </option>
          ))}
        </select>
      </div>
      {devices.map((device) => (
        <Row
          key={device.path}
          title={device.label}
          detail={device.path}
          onOpen={() => onOpen({ kind: "serial", device: device.path, baud }, device.label)}
          disabled={busy}
        />
      ))}
    </Group>
  );
}

function Group({
  label,
  hint,
  children,
}: {
  label: string;
  hint: string;
  children: React.ReactNode;
}) {
  return (
    <section>
      <div className="panel-header col" style={{ alignItems: "flex-start", height: "auto", padding: "var(--sp-4) var(--sp-6)" }}>
        <span className="label">{label}</span>
        <span className="meta" style={{ textWrap: "pretty" }}>
          {hint}
        </span>
      </div>
      {children}
    </section>
  );
}

function Row({
  title,
  detail,
  trailing,
  preview,
  onHover,
  onOpen,
  disabled,
}: {
  title: string;
  detail: string;
  trailing?: React.ReactNode;
  preview?: string;
  onHover?: () => void;
  onOpen: () => void;
  disabled?: boolean;
}) {
  return (
    <button
      className="list-row col"
      onMouseEnter={onHover}
      onFocus={onHover}
      onClick={onOpen}
      disabled={disabled}
      style={{ alignItems: "stretch", gap: 2 }}
      // The command that will run, so nothing is a surprise.
      title={preview ? `Runs: ${preview}` : undefined}
    >
      <div className="row">
        <span className="truncate grow" style={{ fontSize: "var(--text-control)" }}>
          {title}
        </span>
        {trailing}
      </div>
      <span className="mono meta truncate" style={{ textAlign: "left" }}>
        {preview ?? detail}
      </span>
    </button>
  );
}

function sshDetail(host: api.SshHostInfo): string {
  const target = host.hostname ?? host.alias;
  const user = host.user ? `${host.user}@` : "";
  const port = host.port && host.port !== 22 ? `:${host.port}` : "";
  return `${user}${target}${port}`;
}

/**
 * What a probe found, in as many words as it actually justifies.
 *
 * A connect time is labelled as a connect time. SSH exposes no round-trip time, so the one
 * thing this must never do is render a TCP measurement as latency — a number with the wrong
 * name is worse than no number, because it will be trusted.
 */
function ReachabilityChip({
  alias,
  state,
}: {
  alias: string;
  state: api.Reachability | undefined;
}) {
  if (!state) return null;

  switch (state.state) {
    case "multiplexed":
      // The only state Tervin knows rather than infers: `ssh -O check` asked the running
      // master. So it carries no number — one would imply a measurement that never happened.
      return (
        <span className="chip tone-green" title={`A multiplexed connection to ${alias} is already open`}>
          connected
        </span>
      );
    case "open":
      return (
        <span
          className="chip tone-green tabular"
          title="Time to establish a TCP connection to the SSH port — not SSH round-trip time, which SSH does not report"
        >
          {state.connect_ms} ms to connect
        </span>
      );
    case "refused":
      // Meaningfully different from silence: something answered.
      return (
        <span className="chip tone-amber" title="The host answered but nothing is listening on that port">
          refused
        </span>
      );
    case "timeout":
      return (
        <span
          className="chip tone-amber"
          title="Nothing answered. Firewalled, asleep or gone — indistinguishable from here"
        >
          no answer
        </span>
      );
    case "unresolved":
      return (
        <span className="chip tone-red" title="The hostname did not resolve">
          unknown name
        </span>
      );
    case "skipped":
      // The honest case, and the one a naive implementation gets wrong: a host behind a
      // jump may only be routable from the jump host, so a failed direct connection would
      // report "unreachable" about a host that works.
      return (
        <span className="chip" title={state.reason}>
          not checkable
        </span>
      );
  }
}
