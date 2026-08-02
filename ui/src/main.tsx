/**
 * Entry point, plus the last line of defence against a blank window.
 *
 * A React tree that throws during render unmounts itself, and a Tauri webview
 * shows the result as a blank white rectangle with nothing in it — no message, no
 * stack, nothing to report. That is the worst failure mode this app has, because
 * it turns a one-line bug into an unfalsifiable "it doesn't work".
 *
 * So three things are installed before the app mounts:
 *
 *  1. An error boundary, so a render crash shows the error instead of nothing.
 *  2. Global `error` and `unhandledrejection` handlers, for failures outside
 *     React's reach.
 *  3. A watchdog for the case where nothing paints at all — which catches a
 *     bundle that never even evaluated.
 */

import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./design/base.css";
import { clearRendererAttempt } from "./lib/renderer";
import { uiLog } from "./lib/api";

const root = document.getElementById("root");
if (!root) throw new Error("#root is missing from index.html");

/**
 * Render a failure as readable, copyable text.
 *
 * Deliberately plain DOM with inline styles: it has to work when the stylesheet,
 * the theme, or React itself is the thing that failed.
 */
function showFatal(title: string, detail: string, hint?: string): void {
  // Log first: the overlay is for the user, the log is what can be pasted into a
  // bug report and what is readable when the window itself is the problem.
  uiLog("error", title, detail);
  if (document.getElementById("tervin-fatal")) return;

  const panel = document.createElement("div");
  panel.id = "tervin-fatal";
  panel.setAttribute("role", "alert");
  panel.style.cssText = [
    "position:fixed",
    "inset:0",
    "z-index:99999",
    "background:#141514",
    "color:#E5E8E5",
    "font:13px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace",
    "padding:28px",
    "overflow:auto",
    "user-select:text",
    "-webkit-user-select:text",
  ].join(";");

  const heading = document.createElement("div");
  heading.style.cssText = "color:#D77D79;font-weight:600;font-size:15px;margin-bottom:6px";
  heading.textContent = title;

  const lead = document.createElement("div");
  lead.style.cssText = "color:#909894;margin-bottom:16px;max-width:80ch";
  lead.textContent =
    hint ??
    "Tervin hit an error it could not recover from. The details below are the whole of what it knows.";

  const pre = document.createElement("pre");
  pre.style.cssText = [
    "margin:0",
    "padding:14px",
    "background:#1B1D1C",
    "border:1px solid #323634",
    "border-radius:6px",
    "white-space:pre-wrap",
    "word-break:break-word",
    "color:#AEB5B1",
    "max-width:120ch",
  ].join(";");
  pre.textContent = detail;

  const actions = document.createElement("div");
  actions.style.cssText = "margin-top:16px;display:flex;gap:8px;flex-wrap:wrap";

  const secondary =
    "height:28px;padding:0 12px;border:1px solid #323634;border-radius:5px;" +
    "background:transparent;color:#E5E8E5;font:inherit;cursor:pointer";

  const reload = document.createElement("button");
  reload.textContent = "Reload";
  reload.style.cssText =
    "height:28px;padding:0 12px;border:none;border-radius:5px;background:#68AEA5;" +
    "color:#141514;font:inherit;font-weight:600;cursor:pointer";
  reload.onclick = () => window.location.reload();

  const safeMode = document.createElement("button");
  safeMode.textContent = "Reload without GPU rendering";
  safeMode.title =
    "Restarts using the DOM renderer. Slower, but it rules out the GPU path as the cause.";
  safeMode.style.cssText = secondary;
  safeMode.onclick = () => {
    try {
      localStorage.setItem("tervin.renderer.force", "dom");
    } catch {
      // Storage may be unavailable; reloading is still worth a try.
    }
    window.location.reload();
  };

  const copy = document.createElement("button");
  copy.textContent = "Copy details";
  copy.style.cssText = secondary;
  copy.onclick = () => {
    void navigator.clipboard?.writeText(`${title}\n\n${detail}`);
    copy.textContent = "Copied";
  };

  actions.append(reload, safeMode, copy);
  panel.append(heading, lead, pre, actions);
  document.body.appendChild(panel);
}

/** Format anything that was thrown, including non-`Error` values. */
function describe(value: unknown): string {
  if (value instanceof Error) {
    return `${value.name}: ${value.message}\n\n${value.stack ?? "(no stack)"}`;
  }
  try {
    return typeof value === "string" ? value : JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    // The component stack usually beats the JS stack for a render crash, because
    // it names the component that failed.
    showFatal(
      "Tervin could not render the workspace",
      `${describe(error)}\n\nComponent stack:${info.componentStack ?? "\n(unavailable)"}`,
    );
  }

  render() {
    // The overlay is real DOM outside React, so rendering nothing here is safe —
    // the user still sees the error.
    return this.state.error ? null : this.props.children;
  }
}

/**
 * Whether the app has painted at all.
 *
 * This is what separates "Tervin is unusable" from "something threw once".
 * Before first paint, any error is fatal — there is nothing on screen and no way
 * to recover. After first paint the app demonstrably works, so an isolated async
 * error is reported without destroying a working window.
 */
let hasPainted = false;

/**
 * Errors that are known to be harmless.
 *
 * xterm.js queues viewport work and can run it after a terminal is disposed,
 * which React StrictMode guarantees will happen on every mount in development.
 * The disposed terminal is already gone, so the access is inert — but escalating
 * it to a full-screen overlay would blank a perfectly working app, which is a
 * far worse bug than the one being reported.
 */
const BENIGN = [
  // Post-dispose renderer access from xterm's viewport.
  /_renderer\.value\.dimensions/,
  // A WebGL context going away while a frame is in flight.
  /Context Lost|WebGL context/i,
  // ResizeObserver reporting a loop it already recovered from.
  /ResizeObserver loop/i,
];

function isBenign(detail: string): boolean {
  return BENIGN.some((pattern) => pattern.test(detail));
}

/** Route a runtime failure to the right place. */
function report(title: string, detail: string): void {
  if (isBenign(detail)) {
    uiLog("warn", `${title} (known-benign, ignored)`, detail);
    return;
  }
  if (hasPainted) {
    // The app is up and usable. Record it, but do not tear the window down over
    // one async failure.
    uiLog("error", title, detail);
    return;
  }
  showFatal(title, detail);
}

window.addEventListener("error", (event) => {
  // Resource load failures arrive here too and are not fatal on their own.
  if (!event.error && !event.message) return;
  report("Tervin hit an unhandled error", describe(event.error ?? event.message));
});

window.addEventListener("unhandledrejection", (event) => {
  report("Tervin hit an unhandled promise rejection", describe(event.reason));
});

/**
 * Watchdog for the case where nothing renders at all.
 *
 * If the bundle failed to evaluate, no error fires and no component mounts — the
 * window just stays blank. This notices, and reports the one thing that most
 * often explains it.
 */
const watchdog = window.setTimeout(() => {
  if (root.childElementCount === 0 && !document.getElementById("tervin-fatal")) {
    showFatal(
      "Tervin started but nothing rendered",
      [
        "The window opened and the script ran, but no UI mounted within 8 seconds.",
        "",
        `location: ${window.location.href}`,
        `tauri bridge: ${"__TAURI_INTERNALS__" in window ? "present" : "MISSING"}`,
      ].join("\n"),
      "A missing Tauri bridge means this page was opened in a plain browser, where Tervin cannot reach the terminal. Run `pnpm app` rather than opening the dev-server URL directly.",
    );
  }
}, 8000);

uiLog("debug", "ui: bundle evaluated, mounting");

ReactDOM.createRoot(root).render(
  // StrictMode double-invokes effects in development. The terminal effect is
  // written to tolerate that — it disposes its xterm instance and closes its PTY
  // on cleanup — so keeping it on catches lifecycle bugs early.
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);

// Two frames after mount the renderer has actually drawn, so this run's choice is
// safe to keep. Until this clears, the next launch falls back — see lib/renderer.
requestAnimationFrame(() => {
  window.clearTimeout(watchdog);
  requestAnimationFrame(() => {
    clearRendererAttempt();
    hasPainted = true;
    uiLog("debug", "ui: first frame painted");
  });
});
