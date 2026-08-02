/**
 * The sticky command header's one real decision.
 *
 * Long output is where a terminal loses you: three screens into a build log, the command
 * that produced it is gone and nothing on screen says what you are reading. The header
 * answers that, and the off-by-one is the whole feature: showing it while the command's
 * own line is still visible duplicates what the user can already read, which turns a
 * useful label into clutter.
 */

import { describe, expect, it } from "vitest";
import { stickyCommandFor, type CommandMark } from "./sticky";

const marks = (...pairs: [number, string][]): CommandMark[] =>
  pairs.map(([line, command]) => ({ line, command }));

describe("stickyCommandFor", () => {
  it("shows nothing when the command's own line is still visible", () => {
    // The command is the first visible row, so the header would be a duplicate of it.
    expect(stickyCommandFor(marks([10, "cargo test"]), 10, false)).toBeNull();
    // And when it is below the top of the viewport, it is plainly on screen.
    expect(stickyCommandFor(marks([12, "cargo test"]), 10, false)).toBeNull();
  });

  it("shows the command once its line has scrolled above the viewport", () => {
    expect(stickyCommandFor(marks([10, "cargo test"]), 11, false)).toBe("cargo test");
    expect(stickyCommandFor(marks([10, "cargo test"]), 400, false)).toBe("cargo test");
  });

  it("picks the nearest command above, not the first", () => {
    // Scrolled into the third command's output, the answer is the third command. Taking
    // the first match would label a build log with whatever ran an hour earlier.
    const all = marks([10, "first"], [200, "second"], [500, "third"]);
    expect(stickyCommandFor(all, 600, false)).toBe("third");
    expect(stickyCommandFor(all, 300, false)).toBe("second");
    expect(stickyCommandFor(all, 50, false)).toBe("first");
  });

  it("ignores a marker whose line has been trimmed from scrollback", () => {
    // xterm reports -1 for a disposed marker. Treating that as line 0 would make the
    // oldest command win every comparison and pin a header that never changes.
    const all = marks([-1, "long gone"], [200, "current"]);
    expect(stickyCommandFor(all, 600, false)).toBe("current");
    expect(stickyCommandFor(marks([-1, "long gone"]), 600, false)).toBeNull();
  });

  it("shows nothing over a full-screen program", () => {
    // vim draws its own interface, and what is on screen is not a Block's output.
    expect(stickyCommandFor(marks([10, "vim notes.md"]), 400, true)).toBeNull();
  });

  it("shows nothing when no command has run", () => {
    expect(stickyCommandFor([], 400, false)).toBeNull();
  });

  it("survives markers arriving out of order", () => {
    // Reflow renumbers markers, so the array is not guaranteed to stay sorted. The answer
    // must be the nearest one above regardless of order.
    const all = marks([500, "third"], [10, "first"], [200, "second"]);
    expect(stickyCommandFor(all, 300, false)).toBe("second");
  });
});
