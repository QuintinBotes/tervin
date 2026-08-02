/**
 * Editing behaviour, asserted as behaviour.
 *
 * Every test names what a command should do to text — `dw deletes to the start of the
 * next word` — rather than which branch it takes. That is the only way to tell whether
 * an emulation is faithful, and faithfulness is the whole requirement: someone with
 * twenty years of vim muscle memory notices a wrong `dw` immediately, and a composer
 * that is *almost* vim is more annoying than one that is plainly not.
 */

import { describe, expect, it } from "vitest";
import {
  applyEmacs,
  applyVimNormal,
  isNewline,
  isSubmit,
  lineEnd,
  lineStart,
  splice,
  wordEnd,
  wordStart,
  type EditState,
  type Keystroke,
} from "./editing";

/** A state written with `|` marking the caret, which reads far better than indices. */
function at(marked: string): EditState {
  const start = marked.indexOf("|");
  expect(start).toBeGreaterThanOrEqual(0);
  const text = marked.replace("|", "");
  return { text, start, end: start };
}

/** Render a state back with `|` at the caret, so failures are readable. */
function show(state: EditState): string {
  return state.text.slice(0, state.start) + "|" + state.text.slice(state.start);
}

const key = (k: string, mods: Partial<Keystroke> = {}): Keystroke => ({
  key: k,
  ctrl: false,
  alt: false,
  meta: false,
  shift: false,
  ...mods,
});

const ctrl = (k: string) => key(k, { ctrl: true });
const alt = (k: string) => key(k, { alt: true });

describe("primitives", () => {
  it("finds line boundaries around a multi-line prompt", () => {
    const text = "first\nsecond\nthird";
    expect(lineStart(text, 8)).toBe(6);
    expect(lineEnd(text, 8)).toBe(12);
    // At the very start and very end, rather than one past them.
    expect(lineStart(text, 0)).toBe(0);
    expect(lineEnd(text, text.length)).toBe(text.length);
  });

  it("skips trailing whitespace when finding a word start", () => {
    // This is what makes C-w and b feel right with the caret just after a word.
    expect(wordStart("cargo test  ", 12)).toBe(6);
    expect(wordStart("cargo test", 10)).toBe(6);
    expect(wordStart("cargo", 3)).toBe(0);
  });

  it("treats a path as several words, because that is how it is edited", () => {
    // `src/lib.rs` — a user deleting a word expects to lose `rs`, not the whole path.
    expect(wordStart("src/lib.rs", 10)).toBe(8);
    expect(wordEnd("src/lib.rs", 0)).toBe(4);
  });
});

describe("emacs bindings", () => {
  it("moves to the start and end of the current line, not the buffer", () => {
    // C-a in a multi-line prompt must not jump to the top.
    const state = at("first\nsec|ond\nthird");
    expect(show(applyEmacs(state, ctrl("a"), "").state)).toBe("first\n|second\nthird");
    expect(show(applyEmacs(state, ctrl("e"), "").state)).toBe("first\nsecond|\nthird");
  });

  it("kills to end of line, and kills the newline when already there", () => {
    // Emacs `kill-line`: from point to end of line — but *at* end of line it takes the
    // newline, which is how repeated C-k walks down a buffer joining lines.
    let result = applyEmacs(at("keep|  drop"), ctrl("k"), "");
    expect(result.state.text).toBe("keep");
    expect(result.yanked).toBe("  drop");

    // The caret is already at end of line here, so this joins immediately.
    result = applyEmacs(at("keep|\nnext"), ctrl("k"), "");
    expect(result.state.text).toBe("keepnext");
    expect(result.yanked).toBe("\n");
  });

  it("kills to the start of the line, not the whole buffer", () => {
    // A C-u that wiped a multi-paragraph prompt would be a data-loss bug.
    const result = applyEmacs(at("first\nsecond|"), ctrl("u"), "");
    expect(result.state.text).toBe("first\n");
    expect(result.yanked).toBe("second");
  });

  it("kills the previous word with C-w, or the selection if there is one", () => {
    expect(applyEmacs(at("cargo test|"), ctrl("w"), "").state.text).toBe("cargo ");

    const selected: EditState = { text: "cargo test", start: 0, end: 5 };
    const result = applyEmacs(selected, ctrl("w"), "");
    expect(result.state.text).toBe(" test");
    expect(result.yanked).toBe("cargo");
  });

  it("yanks what was killed", () => {
    const killed = applyEmacs(at("cargo test|"), ctrl("w"), "");
    const pasted = applyEmacs(at("prefix |"), ctrl("y"), killed.yanked!);
    expect(pasted.state.text).toBe("prefix test");
  });

  it("does not claim C-y with an empty kill ring", () => {
    // Otherwise C-y becomes a key that silently does nothing.
    expect(applyEmacs(at("text|"), ctrl("y"), "").handled).toBe(false);
  });

  it("does not claim C-d at the end of the buffer", () => {
    // In a shell C-d there means EOF; swallowing it would be misleading.
    expect(applyEmacs(at("text|"), ctrl("d"), "").handled).toBe(false);
    expect(applyEmacs(at("te|xt"), ctrl("d"), "").state.text).toBe("tet");
  });

  it("moves and deletes by word with Meta", () => {
    expect(show(applyEmacs(at("cargo test|"), alt("b"), "").state)).toBe("cargo |test");
    expect(show(applyEmacs(at("|cargo test"), alt("f"), "").state)).toBe("cargo |test");
    expect(applyEmacs(at("|cargo test"), alt("d"), "").state.text).toBe("test");
    expect(applyEmacs(at("cargo test|"), alt("Backspace"), "").state.text).toBe("cargo ");
  });

  it("drags the character before point forward over the one at point", () => {
    // Readline's `transpose-chars`, precisely: the caret advances with the dragged
    // character. `ba|cd` would be a plausible-looking guess and is not what C-t does.
    expect(show(applyEmacs(at("ab|cd"), ctrl("t"), "").state)).toBe("acb|d");
    // At the start of the buffer there is nothing to drag, so it is not claimed.
    expect(applyEmacs(at("|abcd"), ctrl("t"), "").handled).toBe(false);
  });

  it("leaves unclaimed keys to the platform", () => {
    // The important negative case: swallowing keys breaks IME, dead keys, and
    // accessibility, and no amount of emulation is worth that.
    for (const k of [key("q"), alt("é"), key("Tab"), key("ArrowLeft"), ctrl("z")]) {
      expect(applyEmacs(at("text|"), k, "").handled).toBe(false);
    }
  });
});

describe("vim normal mode", () => {
  const normal = (marked: string, k: string, pending = "", ring = "") =>
    applyVimNormal(at(marked), key(k), pending, ring);

  it("enters insert mode where each command says it should", () => {
    expect(normal("ab|cd", "i").vimMode).toBe("insert");
    expect(show(normal("ab|cd", "a").state)).toBe("abc|d");
    expect(show(normal("  ind|ented", "I").state)).toBe("  |indented");
    expect(show(normal("li|ne", "A").state)).toBe("line|");
  });

  it("opens a line below with o and above with O", () => {
    const below = normal("first|\nsecond", "o");
    expect(below.state.text).toBe("first\n\nsecond");
    expect(below.vimMode).toBe("insert");

    const above = normal("first\nsec|ond", "O");
    expect(above.state.text).toBe("first\n\nsecond");
    // The caret goes on the new empty line, not after it.
    expect(above.state.start).toBe(6);
  });

  it("moves by character, word, and line boundary", () => {
    expect(show(normal("ab|cd", "h").state)).toBe("a|bcd");
    expect(show(normal("ab|cd", "l").state)).toBe("abc|d");
    expect(show(normal("|cargo test", "w").state)).toBe("cargo |test");
    expect(show(normal("cargo te|st", "b").state)).toBe("cargo |test");
    expect(show(normal("cargo te|st", "0").state)).toBe("|cargo test");
    expect(show(normal("car|go test", "$").state)).toBe("cargo test|");
    expect(show(normal("  ind|ent", "^").state)).toBe("  |indent");
  });

  it("moves between lines keeping the column", () => {
    expect(show(normal("first\nse|cond", "k").state)).toBe("fi|rst\nsecond");
    expect(show(normal("fi|rst\nsecond", "j").state)).toBe("first\nse|cond");
  });

  it("clamps a vertical move to a shorter line rather than overshooting", () => {
    // Moving onto a shorter line must land at its end, not past it into the next.
    expect(show(normal("longer line\nab|", "k").state)).toBe("lo|nger line\nab");
    expect(show(normal("longer li|ne\nab", "j").state)).toBe("longer line\nab|");
  });

  it("does nothing at the first and last line rather than wrapping", () => {
    expect(show(normal("on|ly", "k").state)).toBe("on|ly");
    expect(show(normal("on|ly", "j").state)).toBe("on|ly");
  });

  it("deletes a character with x and to end of line with D", () => {
    expect(normal("ab|cd", "x").state.text).toBe("abd");
    expect(normal("keep| drop", "D").state.text).toBe("keep");
    const changed = normal("keep| drop", "C");
    expect(changed.state.text).toBe("keep");
    expect(changed.vimMode).toBe("insert");
  });

  it("dw deletes to the start of the next word", () => {
    const pending = normal("|cargo test", "d");
    expect(pending.pending).toBe("d");
    const done = applyVimNormal(pending.state, key("w"), "d", "");
    expect(done.state.text).toBe("test");
    expect(done.yanked).toBe("cargo ");
    // `d` stays in normal mode; only `c` changes.
    expect(done.vimMode).toBe("normal");
  });

  it("cw deletes the word and enters insert mode", () => {
    const done = applyVimNormal(at("|cargo test"), key("w"), "c", "");
    expect(done.state.text).toBe("test");
    expect(done.vimMode).toBe("insert");
  });

  it("dd deletes the whole line including its newline", () => {
    const done = applyVimNormal(at("first\nsec|ond\nthird"), key("d"), "d", "");
    expect(done.state.text).toBe("first\nthird");
    expect(done.yanked).toBe("second\n");
  });

  it("yy yanks without changing the text", () => {
    const done = applyVimNormal(at("first\nsec|ond"), key("y"), "y", "");
    expect(done.state.text).toBe("first\nsecond");
    expect(done.yanked).toBe("second");
  });

  it("abandons a pending operator on a key that is not a motion", () => {
    // A `d` followed by a stray key must not delete something arbitrary.
    const done = applyVimNormal(at("|cargo test"), key("z"), "d", "");
    expect(done.state.text).toBe("cargo test");
    expect(done.pending).toBe("");
  });

  it("pastes after the caret with p and before it with P", () => {
    expect(applyVimNormal(at("a|b"), key("p"), "", "X").state.text).toBe("abX");
    expect(applyVimNormal(at("a|b"), key("P"), "", "X").state.text).toBe("aXb");
  });

  it("does not treat printable keys as text in normal mode", () => {
    // The defining property of normal mode. Typing `x` must not insert an x.
    const result = normal("ab|", "z");
    expect(result.handled).toBe(true);
    expect(result.state.text).toBe("ab");
  });

  it("lets modified keys through so application shortcuts still work", () => {
    // ⌘V, ⌘A, and Tervin's own bindings must survive normal mode.
    for (const k of [key("v", { meta: true }), key("k", { ctrl: true }), key("f", { alt: true })]) {
      expect(applyVimNormal(at("ab|"), k, "", "").handled).toBe(false);
    }
  });

  it("round-trips a delete and a paste", () => {
    const deleted = applyVimNormal(at("|cargo test"), key("w"), "d", "");
    const pasted = applyVimNormal(deleted.state, key("P"), "", deleted.yanked!);
    expect(pasted.state.text).toBe("cargo test");
  });
});

describe("submitting", () => {
  it("sends on modifier-Enter and inserts a newline on plain Enter", () => {
    // A prompt is usually several lines. A box where Enter submits makes writing one an
    // exercise in not pressing it.
    expect(isSubmit(key("Enter", { meta: true }))).toBe(true);
    expect(isSubmit(key("Enter", { ctrl: true }))).toBe(true);
    expect(isSubmit(key("Enter"))).toBe(false);

    expect(isNewline(key("Enter"))).toBe(true);
    expect(isNewline(key("Enter", { meta: true }))).toBe(false);
    // Shift-Enter is a newline too, which is what people reach for by habit.
    expect(isNewline(key("Enter", { shift: true }))).toBe(true);
  });
});

describe("splice", () => {
  it("puts the caret after what was inserted", () => {
    expect(show(splice(at("a|d"), 1, 1, "bc"))).toBe("abc|d");
  });

  it("clamps a range that runs past the text", () => {
    // Defensive: a motion at the buffer edge must not produce an invalid slice.
    expect(splice(at("ab|"), 1, 99, "").text).toBe("a");
    expect(splice(at("ab|"), -5, 1, "").text).toBe("b");
  });
});
