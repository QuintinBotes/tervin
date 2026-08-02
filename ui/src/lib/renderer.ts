/**
 * Choosing a terminal renderer, and surviving one that crashes.
 *
 * xterm.js can draw through WebGL, a 2D canvas, or the DOM. WebGL is what keeps a
 * streaming build log at frame rate, so it is the right default — but it runs
 * inside a system webview talking to a real GPU driver, and that path can fail in
 * ways JavaScript cannot catch. A `try`/`catch` around addon construction only
 * handles a *thrown* error; if the GPU process aborts, the whole web content
 * process dies and the window goes blank with no exception to catch.
 *
 * So the choice is made durable instead of hopeful:
 *
 *  1. Before creating a renderer, record which one is being attempted.
 *  2. Once the app has mounted and painted a frame, clear the record.
 *  3. On startup, a record that is still present means the previous run died
 *     while using that renderer — so drop to the next-safest one and say so.
 *
 * This is the only mechanism available that survives a native crash, because it
 * is the only state that outlives the process.
 */

export type RendererMode = "webgl" | "canvas" | "dom";

const ATTEMPT_KEY = "tervin.renderer.attempt";
const FORCE_KEY = "tervin.renderer.force";
const FALLBACK_KEY = "tervin.renderer.fellBack";

/** Safest to fastest; a crash walks this list backwards. */
const ORDER: RendererMode[] = ["dom", "canvas", "webgl"];

/** `localStorage` can throw in a locked-down webview, so every access is guarded. */
function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Without storage there is no crash recovery, only the default. Acceptable:
    // the alternative is refusing to start.
  }
}

function remove(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    // As above.
  }
}

function isMode(value: string | null): value is RendererMode {
  return value === "webgl" || value === "canvas" || value === "dom";
}

/**
 * Decide which renderer this run should use.
 *
 * Call once at startup, before any terminal is created.
 */
export function chooseRenderer(): { mode: RendererMode; reason: string | null } {
  const forced = read(FORCE_KEY);
  if (isMode(forced)) {
    return {
      mode: forced,
      reason: `Renderer pinned to ${forced} in settings.`,
    };
  }

  const attempted = read(ATTEMPT_KEY);
  if (isMode(attempted)) {
    // The previous run set this and never cleared it, so it did not survive to
    // paint a frame. Step down rather than repeating the crash.
    const index = ORDER.indexOf(attempted);
    const next = ORDER[Math.max(index - 1, 0)]!;
    remove(ATTEMPT_KEY);
    write(FALLBACK_KEY, next);
    return {
      mode: next,
      reason:
        `The previous run stopped responding while using the ${attempted} renderer, ` +
        `so Tervin has switched to ${next}. Settings → Appearance can pin a renderer.`,
    };
  }

  const fellBack = read(FALLBACK_KEY);
  if (isMode(fellBack)) {
    // Stay on the safe renderer for subsequent runs rather than re-testing the
    // one that crashed on every launch.
    return {
      mode: fellBack,
      reason: null,
    };
  }

  return { mode: "webgl", reason: null };
}

/** Record that a renderer is being attempted. Called before construction. */
export function markRendererAttempt(mode: RendererMode): void {
  write(ATTEMPT_KEY, mode);
}

/**
 * Confirm the current renderer works. Called after the first painted frame.
 *
 * Until this runs, the attempt marker is what makes the next launch fall back.
 */
export function clearRendererAttempt(): void {
  remove(ATTEMPT_KEY);
}

/** Pin a renderer, or clear the pin with `null`. */
export function forceRenderer(mode: RendererMode | null): void {
  if (mode) {
    write(FORCE_KEY, mode);
  } else {
    remove(FORCE_KEY);
    remove(FALLBACK_KEY);
  }
}

/** Whether Tervin previously stepped down from a faster renderer. */
export function rendererFellBack(): RendererMode | null {
  const value = read(FALLBACK_KEY);
  return isMode(value) ? value : null;
}

/** Clear the fallback record so the fast renderer is tried again. */
export function resetRendererFallback(): void {
  remove(FALLBACK_KEY);
  remove(ATTEMPT_KEY);
}
