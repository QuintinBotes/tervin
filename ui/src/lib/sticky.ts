/**
 * Which command owns the top of the viewport.
 *
 * Pure logic, in `lib/` rather than beside the component, so it can be tested in the fast
 * node environment: importing anything from the terminal component drags in xterm's
 * addons, which need a browser to load at all.
 */

/** A command start, pinned to a scrollback line. */
export interface CommandMark {
  /** xterm's line number, or -1 once the marker has been disposed. */
  line: number;
  command: string;
}

/**
 * The command owning the top visible line, or null when its own line is on screen.
 *
 * Extracted from the effect because the off-by-one lives here: the header must appear only
 * once the command's own line has scrolled *above* the viewport. Showing it while the line
 * is still visible duplicates what the user can already read, which is how a sticky header
 * turns from useful into clutter.
 */
export function stickyCommandFor(
  marks: CommandMark[],
  viewportTop: number,
  alternateScreen: boolean,
): string | null {
  // A full-screen program draws its own interface, and Blocks are not what is on screen.
  if (alternateScreen) return null;

  let found: string | null = null;
  for (const mark of marks) {
    // A disposed marker reports -1; its line was trimmed out of scrollback.
    if (mark.line < 0) continue;
    // Strictly above: at `viewportTop` the command line is the first visible row.
    if (mark.line < viewportTop) found = mark.command;
  }
  return found;
}
