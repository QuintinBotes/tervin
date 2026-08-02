import { describe, expect, it, vi } from "vitest";
import {
  DEFAULT_BINDINGS,
  Keymap,
  formatChord,
  matchesChord,
  parseChord,
} from "./keymap";

/** Build a KeyboardEvent-shaped object without needing a DOM. */
function key(
  k: string,
  mods: { meta?: boolean; ctrl?: boolean; shift?: boolean; alt?: boolean; code?: string } = {},
): KeyboardEvent {
  return {
    key: k,
    code: mods.code ?? `Key${k.toUpperCase()}`,
    metaKey: mods.meta ?? false,
    ctrlKey: mods.ctrl ?? false,
    shiftKey: mods.shift ?? false,
    altKey: mods.alt ?? false,
  } as KeyboardEvent;
}

/** The tests assume macOS, where `mod` is Command. */
function onApple() {
  vi.stubGlobal("navigator", { platform: "MacIntel" });
}

describe("parseChord", () => {
  it("parses modifiers and a key", () => {
    expect(parseChord("mod+shift+p")).toEqual({
      key: "p",
      mod: true,
      shift: true,
      alt: false,
      ctrl: false,
    });
  });

  it("normalises aliases so one binding covers every spelling", () => {
    expect(parseChord("esc")!.key).toBe("escape");
    expect(parseChord("Escape")!.key).toBe("escape");
    expect(parseChord("return")!.key).toBe("enter");
    expect(parseChord("mod+up")!.key).toBe("arrowup");
  });

  it("accepts cmd and option as aliases", () => {
    expect(parseChord("cmd+k")!.mod).toBe(true);
    expect(parseChord("option+x")!.alt).toBe(true);
  });

  it("rejects a chord that is only modifiers", () => {
    // It could never fire, and would otherwise swallow every modifier press.
    expect(parseChord("mod+shift")).toBeNull();
    expect(parseChord("mod")).toBeNull();
  });

  it("rejects empty input rather than throwing", () => {
    expect(parseChord("")).toBeNull();
    expect(parseChord("   ")).toBeNull();
  });
});

describe("matchesChord", () => {
  it("matches an exact chord", () => {
    onApple();
    const chord = parseChord("mod+k")!;
    expect(matchesChord(key("k", { meta: true }), chord)).toBe(true);
  });

  it("does not match when an extra modifier is held", () => {
    // ⇧⌘K must not fire a ⌘K binding.
    onApple();
    const chord = parseChord("mod+k")!;
    expect(matchesChord(key("k", { meta: true, shift: true }), chord)).toBe(false);
  });

  it("does not match when a required modifier is missing", () => {
    onApple();
    const chord = parseChord("mod+k")!;
    expect(matchesChord(key("k"), chord)).toBe(false);
  });

  it("matches punctuation keys via the physical code", () => {
    // On layouts where `=` requires shift, `event.key` is not `=`.
    onApple();
    const chord = parseChord("mod+=")!;
    expect(matchesChord(key("+", { meta: true, code: "Equal" }), chord)).toBe(true);
  });

  it("distinguishes bracket keys", () => {
    onApple();
    expect(
      matchesChord(key("]", { meta: true, code: "BracketRight" }), parseChord("mod+]")!),
    ).toBe(true);
    expect(
      matchesChord(key("[", { meta: true, code: "BracketLeft" }), parseChord("mod+]")!),
    ).toBe(false);
  });
});

describe("Keymap", () => {
  it("resolves a global binding", () => {
    onApple();
    const map = new Keymap();
    expect(map.resolve(key("k", { meta: true }), "global")).toBe("palette.open");
  });

  it("lets a context binding shadow a global one", () => {
    // ⌘K clears the terminal when a pane has focus, and opens the palette
    // everywhere else. Both are real bindings; context decides.
    onApple();
    const map = new Keymap();
    expect(map.resolve(key("k", { meta: true }), "terminal")).toBe("terminal.clear");
    expect(map.resolve(key("k", { meta: true }), "composer")).toBe("palette.open");
  });

  it("only applies an overlay binding while an overlay is open", () => {
    // Escape is a real keystroke that terminal programs depend on.
    onApple();
    const map = new Keymap();
    const escape = key("Escape", { code: "Escape" });
    expect(map.resolve(escape, "overlay")).toBe("overlay.close");
    expect(map.resolve(escape, "terminal")).toBeNull();
  });

  it("returns null for an unbound chord", () => {
    onApple();
    expect(new Keymap().resolve(key("q", { meta: true, alt: true }), "global")).toBeNull();
  });

  it("reports an invalid binding instead of failing to load", () => {
    const map = new Keymap([
      { id: "broken", label: "Broken", keys: "mod+", when: "global" },
      { id: "fine", label: "Fine", keys: "mod+j", when: "global" },
    ]);
    expect(map.invalid).toHaveLength(1);
    expect(map.invalid[0]!.binding.id).toBe("broken");
    // The valid binding still works.
    expect(map.all().map((b) => b.id)).toEqual(["fine"]);
  });

  it("detects conflicting bindings in the same context", () => {
    const map = new Keymap([
      { id: "a", label: "A", keys: "mod+j", when: "global" },
      { id: "b", label: "B", keys: "mod+j", when: "global" },
    ]);
    const conflicts = map.conflicts();
    expect(conflicts).toHaveLength(1);
    expect(conflicts[0]!.ids).toEqual(["a", "b"]);
  });

  it("does not report the same chord in different contexts as a conflict", () => {
    // That is the shadowing mechanism, not a mistake.
    const map = new Keymap([
      { id: "a", label: "A", keys: "mod+k", when: "global" },
      { id: "b", label: "B", keys: "mod+k", when: "terminal" },
    ]);
    expect(map.conflicts()).toHaveLength(0);
  });

  it("ships defaults with no accidental conflicts", () => {
    expect(new Keymap().conflicts()).toEqual([]);
  });

  it("ships defaults that all parse", () => {
    expect(new Keymap().invalid).toEqual([]);
  });

  it("covers every essential action named in the spec", () => {
    // Keyboard-first is not real if an essential action is mouse-only.
    const ids = new Set(DEFAULT_BINDINGS.map((b) => b.id));
    for (const required of [
      "pane.new",
      "pane.split.right",
      "pane.focus.next",
      "pane.zoom",
      "palette.open",
      "search.open",
      "inspector.toggle",
      "rail.toggle",
      "block.next",
      "block.prev",
      "agent.profile",
      "agent.approve",
      "agent.stop",
      "agent.mode",
    ]) {
      expect(ids, `missing binding: ${required}`).toContain(required);
    }
  });

  it("exposes the chord for an action so menus can show a hint", () => {
    onApple();
    expect(new Keymap().keysFor("palette.open")).toBe("⌘K");
    expect(new Keymap().keysFor("nope")).toBeNull();
  });
});

describe("formatChord", () => {
  it("renders macOS symbols in the conventional order", () => {
    onApple();
    expect(formatChord("mod+shift+p")).toBe("⇧⌘P");
    expect(formatChord("mod+k")).toBe("⌘K");
    expect(formatChord("escape")).toBe("Esc");
    expect(formatChord("mod+arrowup")).toBe("⌘↑");
  });

  it("renders words on other platforms", () => {
    vi.stubGlobal("navigator", { platform: "Win32" });
    expect(formatChord("mod+shift+p")).toBe("Shift+Ctrl+P");
  });

  it("passes an unparseable chord through unchanged", () => {
    expect(formatChord("mod+")).toBe("mod+");
  });
});
