/**
 * Desktop end-to-end tests: WebdriverIO driving the real Tervin window.
 *
 * macOS has no WKWebView driver, so `tauri-driver` cannot be used here. The
 * embedded provider is what makes this work: `tauri-plugin-wdio-webdriver` is
 * compiled into the binary and serves W3C WebDriver from inside the app itself.
 * That plugin is behind the `e2e` Cargo feature, so only the binary built by
 * `pnpm e2e:build` can be driven — see crates/tervin-app/Cargo.toml.
 */
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const configDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(configDir, "..", "..");

// Everything a run leaves behind lands under one directory, which .gitignore
// covers whole. A screenshot of a developer's window is not a repository artefact.
const artifactDir = path.join(repoRoot, "artifacts", "e2e");
const screenshotDir = path.join(artifactDir, "screenshots");
const logDir = path.join(artifactDir, "logs");

// Overridable so a bisect or a release-candidate check can point at another build
// without editing this file; the default is what `pnpm e2e:build` produces.
const appBinaryPath =
  process.env.TERVIN_E2E_BINARY ?? path.join(repoRoot, "target", "debug", "tervin");

// A throwaway HOME, so a run reads and writes its own workspace.sqlite3 rather
// than the developer's. Tervin resolves every path it persists to from the
// platform config and data directories, and both are rooted at HOME, so this one
// variable relocates the entire profile — see crates/tervin-core/src/paths.rs.
//
// Computed once and written back to the environment because the launcher forks a
// worker per capability: without this the worker would mkdtemp a second directory
// and assert against a profile the app never wrote to.
const profileHome =
  process.env.TERVIN_E2E_HOME ?? fs.mkdtempSync(path.join(os.tmpdir(), "tervin-e2e-"));
process.env.TERVIN_E2E_HOME = profileHome;

/** Filesystem-safe name for a test title, so a screenshot can be found by eye. */
function slug(title: string): string {
  return title.replace(/[^a-z0-9]+/gi, "-").replace(/^-|-$/g, "").toLowerCase().slice(0, 80);
}

export const config: WebdriverIO.Config = {
  runner: "local",
  specs: [path.join(configDir, "*.e2e.ts")],

  // One window at a time. The app owns a PTY, a SQLite file, and a WebDriver port;
  // a second instance would contend for all three.
  maxInstances: 1,

  capabilities: [{ browserName: "tauri" }],

  services: [
    [
      "@wdio/tauri-service",
      {
        appBinaryPath,
        // Explicit rather than inferred, so the failure when the binary was built
        // without `--features e2e` reads as "embedded server never came up" instead
        // of the service quietly looking for a tauri-driver that macOS cannot have.
        driverProvider: "embedded",
        // Merged over process.env when the app is spawned.
        env: { HOME: profileHome },
        captureBackendLogs: true,
        captureFrontendLogs: true,
      },
    ],
  ],

  framework: "mocha",
  reporters: ["spec"],
  // WebDriver protocol logs; the flag `--logLevel debug` makes these worth reading.
  outputDir: logDir,
  logLevel: "info",
  bail: 0,

  // Local-development timeouts. Generous because a debug build cold-starts a
  // window, a shell, and a SQLite migration before the first selector resolves.
  waitforTimeout: 15_000,
  connectionRetryTimeout: 120_000,
  connectionRetryCount: 3,

  mochaOpts: {
    ui: "bdd",
    timeout: 120_000,
  },

  onPrepare() {
    fs.mkdirSync(screenshotDir, { recursive: true });
    fs.mkdirSync(logDir, { recursive: true });
    // Printed because the profile is a temporary directory: when a test asserts
    // against persisted state, this is the only way to find what it wrote.
    console.log(`[e2e] app binary   : ${appBinaryPath}`);
    console.log(`[e2e] test profile : ${profileHome}`);
    console.log(`[e2e] artifacts    : ${artifactDir}`);
  },

  /**
   * A failure that leaves no picture behind costs another full run to diagnose.
   */
  async afterTest(test, _context, result) {
    if (result.passed) return;
    const file = path.join(screenshotDir, `${slug(test.parent)}--${slug(test.title)}.png`);
    try {
      await browser.saveScreenshot(file);
      console.log(`[e2e] screenshot: ${file}`);
    } catch (e) {
      // Never mask the assertion failure with a screenshot failure — a window that
      // has already crashed cannot be photographed, and that is itself the evidence.
      console.log(`[e2e] screenshot failed: ${(e as Error).message}`);
    }
  },
};
