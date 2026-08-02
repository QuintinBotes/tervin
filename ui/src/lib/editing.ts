/**
 * Text editing for the composer.
 *
 * A prompt is not a chat message. It is often several paragraphs describing a task,
 * with paths and code in it, and it gets revised before it is sent — so the box it is
 * written in has to behave like an editor rather than a search field.
 *
 * Three modes, because a terminal user already has muscle memory and the wrong default
 * is actively hostile:
 *
 * - **`native`** — the platform's own behaviour, untouched. Anyone who has not asked
 *   for a mode gets exactly what every other macOS text field does.
 * - **`emacs`** — `C-a`, `C-e`, `C-k`, `C-w`, `M-b`, `M-f`, and the rest. This is what
 *   readline does, so it is what a shell user's hands already do.
 * - **`vim`** — normal and insert mode, with the motions and operators that carry the
 *   weight in practice.
 *
 * ## Why this is a pure module
 *
 * Every function here takes a text-and-selection state and returns a new one. No DOM,
 * no React. That makes the behaviour testable as behaviour — `dw` deletes a word — and
 * keeps the component free of editing logic, which is where this kind of code normally
 * turns into an unmaintainable pile of `if (e.ctrlKey)`.
 *
 * ## What is deliberately not implemented
 *
 * Registers, marks, macros, counts on operators (`d2w`), visual block mode, and undo
 * trees. A composer is not an editor, and a half-working `q` register would be worse
 * than an absent one — you would reach for it and lose text. The commands here are the
 * ones whose absence is *felt*; everything else is left to the real editor, which is
 * one pane away.
 */

/** The editable state of the composer. */
export interface EditState {
  text: string;
  /** Caret position, or the anchor of a selection. */
  start: number;
  /** End of the selection. Equal to `start` when there is no selection. */
  end: number;
}

export type EditMode = "native" | "emacs" | "vim";

/** Vim's two modes. Ignored entirely in the other editing modes. */
export type VimMode = "insert" | "normal";

/**
 * What a keystroke did.
 *
 * `handled: false` is the important case: the key was not ours, so the caller must let
 * the platform have it. Swallowing unrecognised keys is how an editor emulation breaks
 * IME input, dead keys, and accessibility.
 */
export interface EditResult {
  handled: boolean;
  state: EditState;
  /** Set when the keystroke changed vim's mode. */
  vimMode?: VimMode;
  /** Text the command cut, for a subsequent paste. */
  yanked?: string;
  /** Set when the command asks the caller to submit. */
  submit?: boolean;
}

/** The subset of a keyboard event this module needs. */
export interface Keystroke {
  key: string;
  ctrl: boolean;
  alt: boolean;
  meta: boolean;
  shift: boolean;
}

const unchanged = (state: EditState): EditResult => ({ handled: false, state });

// ------------------------------------------------------------------ primitives

/** Characters that end a word. Matches the terminal's own word separators. */
function isWordChar(ch: string): boolean {
  return /[\p{L}\p{N}_]/u.test(ch);
}

/** Start of the line the offset sits on. */
export function lineStart(text: string, at: number): number {
  const before = text.lastIndexOf("\n", Math.max(0, at - 1));
  return before === -1 ? 0 : before + 1;
}

/** End of the line the offset sits on, before its newline. */
export function lineEnd(text: string, at: number): number {
  const after = text.indexOf("\n", at);
  return after === -1 ? text.length : after;
}

/**
 * Start of the word at or before `at`.
 *
 * Skips trailing whitespace first, which is what makes `C-w` and `b` feel right when
 * the caret sits just after a word rather than inside it.
 */
export function wordStart(text: string, at: number): number {
  let i = at;
  while (i > 0 && !isWordChar(text[i - 1]!)) i--;
  while (i > 0 && isWordChar(text[i - 1]!)) i--;
  return i;
}

/** Start of the next word after `at`. */
export function wordEnd(text: string, at: number): number {
  let i = at;
  while (i < text.length && isWordChar(text[i]!)) i++;
  while (i < text.length && !isWordChar(text[i]!)) i++;
  return i;
}

/** Replace `[from, to)` with `insert`, putting the caret after it. */
export function splice(
  state: EditState,
  from: number,
  to: number,
  insert = "",
): EditState {
  const lo = Math.max(0, Math.min(from, to));
  const hi = Math.min(state.text.length, Math.max(from, to));
  const text = state.text.slice(0, lo) + insert + state.text.slice(hi);
  const caret = lo + insert.length;
  return { text, start: caret, end: caret };
}

function moveTo(state: EditState, at: number, extend = false): EditState {
  const caret = Math.max(0, Math.min(state.text.length, at));
  return extend
    ? { ...state, end: caret }
    : { text: state.text, start: caret, end: caret };
}

// ----------------------------------------------------------------------- emacs

/**
 * Readline's bindings, which are what a shell user's hands already do.
 *
 * `C-a`, `C-e`, `C-k`, `C-u`, `C-w`, `C-y`, `C-d`, `C-t`, `M-b`, `M-f`, `M-d`, `M-⌫`.
 * These work in bash, zsh, and every readline prompt — so a composer that ignores them
 * is the one thing in the workspace that does.
 */
export function applyEmacs(
  state: EditState,
  key: Keystroke,
  killRing: string,
): EditResult {
  const { text, start } = state;
  const at = Math.min(start, state.end);
  const hasSelection = state.start !== state.end;

  // Meta is Option on macOS, which also produces accented characters. Only the
  // combinations below are claimed; anything else falls through so dead keys still work.
  if (key.alt && !key.ctrl && !key.meta) {
    switch (key.key.toLowerCase()) {
      case "b":
        return { handled: true, state: moveTo(state, wordStart(text, at), key.shift) };
      case "f":
        return { handled: true, state: moveTo(state, wordEnd(text, at), key.shift) };
      case "d": {
        const to = wordEnd(text, at);
        return {
          handled: true,
          state: splice(state, at, to),
          yanked: text.slice(at, to),
        };
      }
      case "backspace": {
        const from = wordStart(text, at);
        return {
          handled: true,
          state: splice(state, from, at),
          yanked: text.slice(from, at),
        };
      }
      default:
        return unchanged(state);
    }
  }

  if (!key.ctrl || key.meta) return unchanged(state);

  switch (key.key.toLowerCase()) {
    case "a":
      return { handled: true, state: moveTo(state, lineStart(text, at), key.shift) };
    case "e":
      return { handled: true, state: moveTo(state, lineEnd(text, at), key.shift) };
    case "b":
      return { handled: true, state: moveTo(state, at - 1, key.shift) };
    case "f":
      return { handled: true, state: moveTo(state, at + 1, key.shift) };
    case "k": {
      // Kill to end of line — or the newline itself when already there, which is how
      // readline lets repeated C-k join lines.
      const end = lineEnd(text, at);
      const to = end === at ? Math.min(text.length, at + 1) : end;
      return { handled: true, state: splice(state, at, to), yanked: text.slice(at, to) };
    }
    case "u": {
      // In readline C-u kills to the start of the line, not the whole buffer.
      const from = lineStart(text, at);
      return {
        handled: true,
        state: splice(state, from, at),
        yanked: text.slice(from, at),
      };
    }
    case "w": {
      const from = hasSelection ? Math.min(state.start, state.end) : wordStart(text, at);
      const to = hasSelection ? Math.max(state.start, state.end) : at;
      return {
        handled: true,
        state: splice(state, from, to),
        yanked: text.slice(from, to),
      };
    }
    case "y":
      // Paste the kill ring. Empty is a no-op rather than a handled key, so C-y still
      // reaches the platform when there is nothing to yank.
      return killRing
        ? { handled: true, state: splice(state, at, state.end, killRing) }
        : unchanged(state);
    case "d":
      // Delete forward. Not claimed at the end of the buffer: in a shell C-d there
      // means EOF, and quietly doing nothing would be confusing.
      return at < text.length
        ? { handled: true, state: splice(state, at, at + 1) }
        : unchanged(state);
    case "t": {
      // Transpose the two characters around the caret.
      if (at === 0 || text.length < 2) return unchanged(state);
      const i = Math.min(at, text.length - 1);
      const swapped =
        text.slice(0, i - 1) + text[i]! + text[i - 1]! + text.slice(i + 1);
      return { handled: true, state: { text: swapped, start: i + 1, end: i + 1 } };
    }
    default:
      return unchanged(state);
  }
}

// ------------------------------------------------------------------------- vim

/**
 * Normal-mode commands.
 *
 * Motions, `d`/`c`/`y` with a motion, and the handful of single-key edits that carry
 * the weight. Operators are resolved through one pending-operator value rather than a
 * parser, because the composer needs `dw` and `cc` — not a language.
 */
export function applyVimNormal(
  state: EditState,
  key: Keystroke,
  pending: string,
  killRing: string,
): EditResult & { pending?: string } {
  const { text } = state;
  const at = Math.min(state.start, state.end);

  // Modified keys are not vim commands. Letting them through means ⌘V, ⌘A, and the
  // application's own shortcuts keep working in normal mode.
  if (key.ctrl || key.meta || key.alt) return { ...unchanged(state), pending };

  const k = key.key;

  // An operator is waiting for its motion.
  if (pending) {
    const range = motionRange(text, at, k, pending);
    if (!range) {
      // Not a motion. Abandon the operator rather than guessing — a `d` followed by a
      // stray key must not delete something arbitrary.
      return { handled: true, state, pending: "" };
    }
    const [from, to] = range;
    const cut = text.slice(from, to);
    if (pending === "y") {
      // Yank does not change the text, and the caret goes to the start of the range.
      return {
        handled: true,
        state: moveTo(state, from),
        yanked: cut,
        pending: "",
      };
    }
    return {
      handled: true,
      state: splice(state, from, to),
      yanked: cut,
      pending: "",
      // `c` changes, so it ends in insert mode; `d` stays in normal.
      vimMode: pending === "c" ? "insert" : "normal",
    };
  }

  switch (k) {
    // --- entering insert mode ---
    case "i":
      return { handled: true, state, vimMode: "insert", pending: "" };
    case "a":
      return {
        handled: true,
        state: moveTo(state, at + 1),
        vimMode: "insert",
        pending: "",
      };
    case "I":
      return {
        handled: true,
        state: moveTo(state, firstNonBlank(text, at)),
        vimMode: "insert",
        pending: "",
      };
    case "A":
      return {
        handled: true,
        state: moveTo(state, lineEnd(text, at)),
        vimMode: "insert",
        pending: "",
      };
    case "o": {
      const end = lineEnd(text, at);
      return {
        handled: true,
        state: splice(state, end, end, "\n"),
        vimMode: "insert",
        pending: "",
      };
    }
    case "O": {
      const begin = lineStart(text, at);
      const next = splice(state, begin, begin, "\n");
      return {
        handled: true,
        state: { ...next, start: begin, end: begin },
        vimMode: "insert",
        pending: "",
      };
    }

    // --- motions ---
    case "h":
      return { handled: true, state: moveTo(state, at - 1), pending: "" };
    case "l":
      return { handled: true, state: moveTo(state, at + 1), pending: "" };
    case "j":
      return { handled: true, state: moveTo(state, verticalTarget(text, at, 1)), pending: "" };
    case "k":
      return { handled: true, state: moveTo(state, verticalTarget(text, at, -1)), pending: "" };
    case "0":
      return { handled: true, state: moveTo(state, lineStart(text, at)), pending: "" };
    case "$":
      return { handled: true, state: moveTo(state, lineEnd(text, at)), pending: "" };
    case "^":
      return { handled: true, state: moveTo(state, firstNonBlank(text, at)), pending: "" };
    case "w":
      return { handled: true, state: moveTo(state, wordEnd(text, at)), pending: "" };
    case "b":
      return { handled: true, state: moveTo(state, wordStart(text, at)), pending: "" };
    case "G":
      return { handled: true, state: moveTo(state, text.length), pending: "" };
    case "g":
      // Only `gg` is offered, and it is reached by pressing g twice.
      return { handled: true, state, pending: "g" };

    // --- edits ---
    case "x":
      return {
        handled: true,
        state: splice(state, at, at + 1),
        yanked: text.slice(at, at + 1),
        pending: "",
      };
    case "D": {
      const end = lineEnd(text, at);
      return {
        handled: true,
        state: splice(state, at, end),
        yanked: text.slice(at, end),
        pending: "",
      };
    }
    case "C": {
      const end = lineEnd(text, at);
      return {
        handled: true,
        state: splice(state, at, end),
        yanked: text.slice(at, end),
        vimMode: "insert",
        pending: "",
      };
    }
    case "p":
      return killRing
        ? { handled: true, state: splice(state, at + 1, at + 1, killRing), pending: "" }
        : { handled: true, state, pending: "" };
    case "P":
      return killRing
        ? { handled: true, state: splice(state, at, at, killRing), pending: "" }
        : { handled: true, state, pending: "" };

    // --- operators ---
    case "d":
    case "c":
    case "y":
      return { handled: true, state, pending: k };

    default:
      // Unrecognised keys are swallowed in normal mode — that is what normal mode is —
      // but a printable key is not treated as text, which is the point.
      return { handled: true, state, pending: "" };
  }
}

/** The range a motion covers when used with an operator. */
function motionRange(
  text: string,
  at: number,
  motion: string,
  operator: string,
): [number, number] | null {
  switch (motion) {
    case "w":
      return [at, wordEnd(text, at)];
    case "b":
      return [wordStart(text, at), at];
    case "e":
      return [at, endOfWord(text, at) + 1];
    case "$":
      return [at, lineEnd(text, at)];
    case "0":
      return [lineStart(text, at), at];
    case "^":
      return [firstNonBlank(text, at), at];
    // `dd`, `cc`, `yy`: the doubled operator means the whole line.
    case operator: {
      const begin = lineStart(text, at);
      const end = Math.min(text.length, lineEnd(text, at) + 1);
      return [begin, end];
    }
    default:
      return null;
  }
}

/** Last character of the word the caret is in. */
function endOfWord(text: string, at: number): number {
  let i = at;
  while (i < text.length - 1 && isWordChar(text[i + 1]!)) i++;
  return i;
}

/** First non-blank character of the current line. */
function firstNonBlank(text: string, at: number): number {
  let i = lineStart(text, at);
  while (i < text.length && (text[i] === " " || text[i] === "\t")) i++;
  return i;
}

/**
 * The offset one line up or down, keeping the column.
 *
 * Column is measured from the line start rather than remembered across moves. A sticky
 * column would be more faithful to vim, and getting it half-right is worse than not
 * having it — the caret would drift in ways nobody could predict.
 */
function verticalTarget(text: string, at: number, direction: 1 | -1): number {
  const begin = lineStart(text, at);
  const column = at - begin;

  if (direction === -1) {
    if (begin === 0) return at;
    const prevBegin = lineStart(text, begin - 1);
    return Math.min(prevBegin + column, begin - 1);
  }

  const end = lineEnd(text, at);
  if (end >= text.length) return at;
  const nextBegin = end + 1;
  return Math.min(nextBegin + column, lineEnd(text, nextBegin));
}

// ------------------------------------------------------------------ dispatch

/**
 * Whether a keystroke should submit the prompt.
 *
 * `Enter` inserts a newline and `⌘Enter` sends. A prompt is usually several lines, and
 * a box where `Enter` submits makes writing one an exercise in not pressing it —
 * whereas a box where `Enter` is a newline needs a deliberate send, which is what a
 * modifier is for.
 */
export function isSubmit(key: Keystroke): boolean {
  return key.key === "Enter" && (key.meta || key.ctrl) && !key.alt;
}

/** Whether a keystroke should insert a newline rather than submit. */
export function isNewline(key: Keystroke): boolean {
  return key.key === "Enter" && !key.meta && !key.ctrl;
}
