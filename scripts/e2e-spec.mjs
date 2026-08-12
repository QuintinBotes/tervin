#!/usr/bin/env node
/**
 * Run one end-to-end spec: `pnpm test:e2e:spec -- tests/e2e/settings-font.e2e.ts`.
 *
 * This wrapper exists because pnpm forwards the `--` separator along with the
 * arguments, so a script ending in `--spec` receives `--spec -- <path>`: wdio takes
 * `--` as the value of `--spec`, matches nothing, and silently runs the whole suite
 * instead of the one file asked for. Stripping the separator here is what makes the
 * single-spec command actually single.
 */
import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2).filter((a) => a !== "--");

if (args.length === 0) {
  console.error("usage: pnpm test:e2e:spec -- <path-to-spec> [wdio flags]");
  process.exit(2);
}

// Anything that looks like a flag is passed through untouched; every bare path
// becomes its own --spec, so several files can be named in one command.
const argv = ["run", path.join("tests", "e2e", "wdio.conf.ts")];
for (const arg of args) {
  if (arg.startsWith("-")) argv.push(arg);
  else argv.push("--spec", arg);
}

const wdio = path.join(repoRoot, "node_modules", ".bin", "wdio");
spawn(wdio, argv, { stdio: "inherit", cwd: repoRoot }).on("exit", (code) => {
  process.exit(code ?? 1);
});
