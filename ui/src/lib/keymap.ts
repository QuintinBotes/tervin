/**
 * Keybindings.
 *
 * Tervin is keyboard-first, which means the bindings have to be data rather than
 * a switch statement: they are listed in the palette, shown in a reference sheet,
 * editable in settings, and persisted. A hard-coded `if (e.key === "k")` cannot
 * be any of those things.
 *
 * `mod` is the portable primary modifier — Command on macOS, Control elsewhere —
 * so a default binding does not have to be written twice, and a user's custom
 * binding moves with them between machines.
 *
 * A binding can be scoped with `when`, because the same chord means different
 * things in different places. `Escape` closes an overlay when one is open and
 * must otherwise reach the shell, where it is a real keystroke that programs
 * depend on.
 */

export type KeyContext =
  | "global"
  /** A terminal pane has focus. */
  | "terminal"
  /** An overlay (palette, search, modal) is open. */
  | "overlay"
  /** A Thread composer has focus. */
  | "composer";

export interface KeyBinding {
  /** Stable identifier, referenced by settings and the palette. */
  id: string;
  /** Human label, shown in the reference sheet. */
  label: string;
  /** Chord, e.g. `mod+shift+p`. */
  keys: string;
  /** Where the binding applies. `global` matches everywhere. */
  when: KeyContext;
}

/** A parsed chord. */
export interface Chord {
  key: string;
  mod: boolean;
  shift: boolean;
  alt: boolean;
  /** Control, when explicitly requested rather than via `mod`. */
  ctrl: boolean;
}

const isApple = (): boolean =>
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform ?? "");

/**
 * The default bindings.
 *
 * These follow the design system's shortcut table. Every essential action has
 * one, because a keyboard-first product where some action is mouse-only is not
 * keyboard-first.
 */
export const DEFAULT_BINDINGS: KeyBinding[] = [
  // Panes and tabs.
  { id: "pane.new", label: "New pane", keys: "mod+t", when: "global" },
  { id: "pane.split.right", label: "Split right", keys: "mod+d", when: "global" },
  { id: "pane.split.down", label: "Split down", keys: "mod+shift+d", when: "global" },
  { id: "pane.close", label: "Close pane", keys: "mod+w", when: "global" },
  { id: "pane.zoom", label: "Zoom pane", keys: "mod+shift+enter", when: "global" },
  { id: "pane.focus.next", label: "Focus next pane", keys: "mod+]", when: "global" },
  { id: "pane.focus.prev", label: "Focus previous pane", keys: "mod+[", when: "global" },
  { id: "pane.swap", label: "Swap pane with next", keys: "mod+shift+]", when: "global" },
  { id: "pane.duplicate", label: "Duplicate pane", keys: "mod+shift+t", when: "global" },
  { id: "tab.new", label: "New tab", keys: "mod+shift+n", when: "global" },

  // Overlays.
  { id: "palette.open", label: "Command palette", keys: "mod+k", when: "global" },
  { id: "search.open", label: "Find in terminal", keys: "mod+f", when: "global" },
  // `mod+shift+s` — "saved". Not a Tab override, for the same reason as the directory
  // picker: the shell's own completion is better than anything Tervin would write.
  { id: "commands.saved", label: "Saved commands", keys: "mod+shift+s", when: "global" },
  // `mod+j` for jump. Deliberately not a Tab override: zsh and fish completion is better
  // than anything Tervin would write for arbitrary commands, and taking Tab would replace
  // something good with something worse. `mod+d` is Split right, by iTerm convention.
  { id: "directory.jump", label: "Jump to a directory", keys: "mod+j", when: "global" },
  // `mod+r` for "run again", matching the muscle memory of a shell's reverse search while
  // being a different key, because Ctrl-R still belongs to the shell.
  { id: "commands.history", label: "Command history", keys: "mod+r", when: "global" },
  { id: "search.next", label: "Find next", keys: "mod+g", when: "global" },
  { id: "search.prev", label: "Find previous", keys: "mod+shift+g", when: "global" },
  { id: "overlay.close", label: "Close overlay", keys: "escape", when: "overlay" },

  // Workspace.
  { id: "inspector.toggle", label: "Toggle inspector", keys: "mod+b", when: "global" },
  { id: "rail.toggle", label: "Toggle activity rail", keys: "mod+shift+b", when: "global" },
  { id: "settings.open", label: "Settings", keys: "mod+,", when: "global" },

  // Surfaces, numbered in the order the switcher shows them.
  //
  // A five-surface workspace with no keyboard route is mouse-dependent, which is the
  // opposite of what a terminal-first product should be. `mod+N` rather than
  // `mod+shift+N` because these are the most frequent moves in the app.
  { id: "surface.terminal", label: "Terminal surface", keys: "mod+1", when: "global" },
  { id: "surface.plan", label: "Plan surface", keys: "mod+2", when: "global" },
  { id: "surface.agents", label: "Agents surface", keys: "mod+3", when: "global" },
  { id: "surface.review", label: "Review surface", keys: "mod+4", when: "global" },
  { id: "surface.history", label: "History surface", keys: "mod+5", when: "global" },
  { id: "connections.open", label: "Connections", keys: "mod+shift+o", when: "global" },

  // Agents.
  // `mod+shift+n` is New tab. `mod+shift+i` is free and sits next to the Agents
  // surface on `mod+3` in muscle memory rather than fighting tab creation.
  { id: "thread.new", label: "New Thread", keys: "mod+shift+i", when: "global" },
  { id: "agent.profile", label: "Switch agent profile", keys: "mod+shift+p", when: "global" },
  { id: "agent.approve", label: "Approve pending request", keys: "mod+shift+a", when: "global" },
  { id: "agent.stop", label: "Stop Thread", keys: "mod+.", when: "global" },
  { id: "agent.mode", label: "Cycle agent mode", keys: "shift+tab", when: "composer" },
  { id: "composer.send", label: "Send to agent", keys: "mod+enter", when: "composer" },

  // Terminal.
  { id: "terminal.copy", label: "Copy", keys: "mod+c", when: "terminal" },
  { id: "terminal.paste", label: "Paste", keys: "mod+v", when: "terminal" },
  { id: "terminal.selectAll", label: "Select all", keys: "mod+a", when: "terminal" },
  { id: "terminal.clear", label: "Clear terminal", keys: "mod+k", when: "terminal" },
  { id: "terminal.zoomIn", label: "Increase font size", keys: "mod+=", when: "global" },
  { id: "terminal.zoomOut", label: "Decrease font size", keys: "mod+-", when: "global" },
  { id: "terminal.zoomReset", label: "Reset font size", keys: "mod+0", when: "global" },

  // Blocks.
  { id: "block.prev", label: "Previous block", keys: "mod+up", when: "terminal" },
  { id: "block.next", label: "Next block", keys: "mod+down", when: "terminal" },
];

/**
 * Parse a chord string into its parts.
 *
 * Returns `null` for an unparseable chord rather than throwing: a typo in a
 * user's keymap should disable one binding, not break startup.
 */
export function parseChord(chord: string): Chord | null {
  const parts = chord
    .toLowerCase()
    .split("+")
    .map((p) => p.trim())
    .filter(Boolean);
  if (parts.length === 0) return null;

  const key = parts[parts.length - 1]!;
  const modifiers = parts.slice(0, -1);

  // A chord that is only modifiers cannot fire.
  if (["mod", "shift", "alt", "option", "ctrl", "control", "cmd", "meta"].includes(key)) {
    return null;
  }

  return {
    key: normaliseKey(key),
    mod: modifiers.includes("mod") || modifiers.includes("cmd") || modifiers.includes("meta"),
    shift: modifiers.includes("shift"),
    alt: modifiers.includes("alt") || modifiers.includes("option"),
    ctrl: modifiers.includes("ctrl") || modifiers.includes("control"),
  };
}

/** Canonical key names, so `esc`, `escape`, and `Escape` are one binding. */
function normaliseKey(key: string): string {
  const aliases: Record<string, string> = {
    esc: "escape",
    return: "enter",
    del: "delete",
    ins: "insert",
    space: " ",
    plus: "=",
    up: "arrowup",
    down: "arrowdown",
    left: "arrowleft",
    right: "arrowright",
    pgup: "pageup",
    pgdn: "pagedown",
  };
  return aliases[key] ?? key;
}

/** Whether a keyboard event matches a chord. */
export function matchesChord(event: KeyboardEvent, chord: Chord): boolean {
  const primary = isApple() ? event.metaKey : event.ctrlKey;
  // On non-Apple platforms `mod` is Control, so an explicit ctrl requirement is
  // already satisfied by the primary modifier and must not be double-counted.
  const explicitCtrl = isApple() ? event.ctrlKey : false;

  if (chord.mod !== primary) return false;
  if (chord.shift !== event.shiftKey) return false;
  if (chord.alt !== event.altKey) return false;
  if (chord.ctrl && !explicitCtrl && !event.ctrlKey) return false;
  if (!chord.ctrl && explicitCtrl) return false;

  const key = event.key.toLowerCase();
  if (key === chord.key) return true;

  // `code` covers layouts where the printed character differs from the physical
  // key — `mod+=` on a layout where `=` needs shift, for example.
  const code = event.code.toLowerCase();
  if (code === `key${chord.key}` || code === `digit${chord.key}`) return true;
  if (chord.key === "=" && code === "equal") return true;
  if (chord.key === "-" && code === "minus") return true;
  if (chord.key === "[" && code === "bracketleft") return true;
  if (chord.key === "]" && code === "bracketright") return true;
  if (chord.key === "," && code === "comma") return true;
  if (chord.key === "." && code === "period") return true;

  return false;
}

/** A resolved keymap: bindings compiled once, matched many times. */
export class Keymap {
  private compiled: { binding: KeyBinding; chord: Chord }[] = [];
  /** Bindings that failed to parse, surfaced in settings rather than swallowed. */
  readonly invalid: { binding: KeyBinding; reason: string }[] = [];

  constructor(bindings: KeyBinding[] = DEFAULT_BINDINGS) {
    for (const binding of bindings) {
      const chord = parseChord(binding.keys);
      if (!chord) {
        this.invalid.push({ binding, reason: `“${binding.keys}” is not a valid chord.` });
        continue;
      }
      this.compiled.push({ binding, chord });
    }
  }

  /**
   * Find the action for an event in a context.
   *
   * Context-specific bindings are checked before global ones, so a binding that
   * only applies in the terminal can shadow a global one — which is how `mod+k`
   * clears the terminal but opens the palette everywhere else.
   */
  resolve(event: KeyboardEvent, context: KeyContext): string | null {
    const scoped = this.compiled.find(
      (c) => c.binding.when === context && matchesChord(event, c.chord),
    );
    if (scoped) return scoped.binding.id;

    const global = this.compiled.find(
      (c) => c.binding.when === "global" && matchesChord(event, c.chord),
    );
    return global?.binding.id ?? null;
  }

  /** Every binding, for the reference sheet and settings. */
  all(): KeyBinding[] {
    return this.compiled.map((c) => c.binding);
  }

  /** The chord for an action, for displaying a hint next to a menu item. */
  keysFor(id: string): string | null {
    const found = this.compiled.find((c) => c.binding.id === id);
    return found ? formatChord(found.binding.keys) : null;
  }

  /**
   * Bindings that collide: the same chord in the same context.
   *
   * Reported rather than silently resolved, because which one wins would
   * otherwise depend on list order, which a user cannot see.
   */
  conflicts(): { keys: string; context: KeyContext; ids: string[] }[] {
    const groups = new Map<string, KeyBinding[]>();
    for (const { binding, chord } of this.compiled) {
      const key = `${binding.when}::${chord.mod}${chord.shift}${chord.alt}${chord.ctrl}${chord.key}`;
      const list = groups.get(key) ?? [];
      list.push(binding);
      groups.set(key, list);
    }
    return [...groups.values()]
      .filter((list) => list.length > 1)
      .map((list) => ({
        keys: list[0]!.keys,
        context: list[0]!.when,
        ids: list.map((b) => b.id),
      }));
  }
}

/** Render a chord for display: `mod+shift+p` becomes `⇧⌘P` on macOS. */
export function formatChord(chord: string): string {
  const parsed = parseChord(chord);
  if (!parsed) return chord;

  const apple = isApple();
  const parts: string[] = [];
  if (parsed.ctrl) parts.push(apple ? "⌃" : "Ctrl");
  if (parsed.alt) parts.push(apple ? "⌥" : "Alt");
  if (parsed.shift) parts.push(apple ? "⇧" : "Shift");
  if (parsed.mod) parts.push(apple ? "⌘" : "Ctrl");

  const names: Record<string, string> = {
    escape: "Esc",
    enter: apple ? "↵" : "Enter",
    arrowup: "↑",
    arrowdown: "↓",
    arrowleft: "←",
    arrowright: "→",
    " ": "Space",
    tab: "⇥",
    backspace: "⌫",
    delete: "⌦",
  };
  const key = names[parsed.key] ?? parsed.key.toUpperCase();

  return apple ? parts.join("") + key : [...parts, key].join("+");
}
