/**
 * Every component must be reachable from the app.
 *
 * This is the test for the class of bug rather than an instance of it. `BlocksPanel` was
 * complete, correct on inspection, and imported nowhere — so no code path reached it, no
 * test exercised it, and it looked finished on every review. The first time it mounted it
 * killed the WebKit content process.
 *
 * A component nobody imports is either dead code or a feature that was never wired up.
 * Both are worth failing a build over, and neither is visible in a diff.
 *
 * Deliberately a static check rather than a runtime one: it needs no DOM, runs in
 * milliseconds, and catches the problem at the moment the wiring is forgotten instead of
 * whenever someone happens to click the right tab.
 */

import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const COMPONENTS = new URL("./", import.meta.url).pathname;
const SRC = join(COMPONENTS, "..");

/** Every `.tsx` component, excluding test files. */
function componentFiles(): string[] {
  return readdirSync(COMPONENTS)
    .filter((name) => name.endsWith(".tsx"))
    .filter((name) => !name.includes(".test."));
}

/** Every source file that could import one, including the components themselves. */
function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...sourceFiles(path));
    } else if (/\.tsx?$/.test(entry.name)) {
      out.push(path);
    }
  }
  return out;
}

describe("every component is reachable", () => {
  const files = sourceFiles(SRC);
  // Read once: this walks the whole UI tree and the assertion runs per component.
  const contents = new Map(files.map((f) => [f, readFileSync(f, "utf8")]));

  for (const component of componentFiles()) {
    const moduleName = component.replace(/\.tsx$/, "");

    it(`${moduleName} is imported somewhere`, () => {
      const importers = [...contents.entries()].filter(([file, text]) => {
        // A file importing itself proves nothing.
        if (file.endsWith(`/components/${component}`)) return false;
        // Matches `./Name`, `../components/Name`, and `components/Name`, with or
        // without an extension.
        return new RegExp(`from\\s+["'][^"']*\\b${moduleName}(\\.tsx)?["']`).test(text);
      });

      expect(
        importers.length,
        `${component} is imported by nothing. Either wire it into a surface or delete it — ` +
          `an unreachable component is untested by construction, which is how the History ` +
          `crash shipped.`,
      ).toBeGreaterThan(0);
    });
  }

  it("finds a plausible number of components, so a broken glob cannot pass silently", () => {
    // Without this, a path mistake makes the loop above iterate zero times and the whole
    // file reports success while checking nothing.
    expect(componentFiles().length).toBeGreaterThan(8);
  });
});
