/**
 * Every IPC command the UI invokes must exist on the Rust side.
 *
 * This is the test for the class of bug rather than an instance of it. `commands.rs` carried
 * `#[tauri::command]` on seventy-three functions and `generate_handler![...]` in `lib.rs`
 * listed sixty-three. The ten left out — saved commands, command history, directory jump, SSH
 * reachability and key status, and the DEC 2031 colour-scheme reply — compiled, type-checked,
 * and failed at runtime with a command-not-found error the moment anyone opened the surface.
 * `store.ts` swallowed the colour-scheme one with `.catch(() => {})`, so that one failed in
 * silence.
 *
 * The reason it was invisible is worth stating: `components/surfaces.dom.test.tsx` spies on the
 * `api` module, which is the right thing for a component test to do and also means every one of
 * those surfaces had a green test while its backend command did not exist. No test in the tree
 * crossed the boundary, so nothing noticed the boundary had a hole in it.
 *
 * Deliberately a static check. Registration is a fact about two source files, and reading them
 * catches the omission at the moment the wiring is forgotten rather than whenever someone
 * happens to open Saved Commands.
 *
 * It only sees command names written as string literals at the `invoke` call. A name assembled
 * at runtime would pass unexamined; there are none today, and adding one would be worth an
 * argument.
 */

import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const LIB = new URL("./", import.meta.url).pathname;
const UI_SRC = join(LIB, "..");
const HOST_LIB_RS = join(LIB, "..", "..", "..", "crates", "tervin-app", "src", "lib.rs");

/**
 * `invoke("name")`, with or without a type argument.
 *
 * The type argument is skipped non-greedily so nested generics — `invoke<Record<string,
 * unknown>>(...)`, `invoke<[string, KeyStatus][]>(...)` — close on the right `>`.
 */
const INVOCATION = /\binvoke\s*(?:<[\s\S]*?>)?\s*\(\s*["'`]([A-Za-z_][A-Za-z0-9_]*)["'`]/g;

/** `commands::name` inside `generate_handler![ ... ]`. */
const REGISTRATION = /commands::([a-z_0-9]+)/g;

/** Every shipping source file, excluding tests: a test may name a command that never ships. */
function shippingSources(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...shippingSources(path));
    } else if (/\.tsx?$/.test(entry.name) && !entry.name.includes(".test.")) {
      out.push(path);
    }
  }
  return out;
}

/** Command names the UI asks the host for, mapped to the files that ask. */
function invoked(): Map<string, string[]> {
  const found = new Map<string, string[]>();
  for (const file of shippingSources(UI_SRC)) {
    const text = readFileSync(file, "utf8");
    for (const m of text.matchAll(INVOCATION)) {
      const name = m[1]!;
      found.set(name, [...(found.get(name) ?? []), file]);
    }
  }
  return found;
}

/** Command names `generate_handler!` actually hands to Tauri. */
function registered(): Set<string> {
  const rust = readFileSync(HOST_LIB_RS, "utf8");
  const macro = rust.match(/generate_handler!\[([\s\S]*?)\]/);
  if (!macro) return new Set();
  // Group headings inside the macro are comments, not entries, and none of them happen to
  // contain `commands::` today — stripping them keeps that from becoming load-bearing.
  const body = macro[1]!.replace(/\/\/[^\n]*/g, "");
  return new Set([...body.matchAll(REGISTRATION)].map((m) => m[1]!));
}

describe("the IPC surface", () => {
  it("every_command_the_ui_invokes_is_registered", () => {
    const asked = invoked();
    const handled = registered();

    // Guards first: a regex that matches nothing would otherwise make the subset assertion
    // below trivially true and report success while checking nothing at all.
    expect(
      asked.size,
      `no invoke() calls found under ${UI_SRC} — the scan is broken`,
    ).toBeGreaterThan(0);
    expect(
      handled.size,
      `no commands:: entries found in ${HOST_LIB_RS} — the scan is broken`,
    ).toBeGreaterThan(0);

    const missing = [...asked.keys()]
      .filter((name) => !handled.has(name))
      .sort()
      .map((name) => `  ${name}  (invoked from ${[...new Set(asked.get(name))].join(", ")})`);

    expect(
      missing,
      `The UI invokes ${missing.length} command(s) that generate_handler! does not register:\n` +
        `${missing.join("\n")}\n` +
        `Each one fails at runtime with a command-not-found error. Add it to the list in ` +
        `crates/tervin-app/src/lib.rs, or stop invoking it.`,
    ).toEqual([]);
  });

  it("a_broken_scan_cannot_report_success", () => {
    // A pattern change that still matched one call would satisfy the non-empty guard above and
    // check almost nothing. These floors are well under the counts at the time of writing
    // (73 invoked, 73 registered) and are here to catch a scan that collapsed, not to freeze
    // the surface.
    expect(invoked().size).toBeGreaterThan(50);
    expect(registered().size).toBeGreaterThan(50);
  });
});
