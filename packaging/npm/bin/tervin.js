#!/usr/bin/env node
/**
 * `npx tervin` — download Tervin if needed, then launch it.
 *
 * ## Why this channel exists
 *
 * macOS applies its `com.apple.quarantine` attribute in the *downloading application*,
 * not in the kernel. A browser sets it; `curl` and Node do not. So a build fetched this
 * way opens normally, while the identical file downloaded through a browser triggers
 * Gatekeeper's "unidentified developer" wall and a trip to System Settings.
 *
 * That makes this the best distribution route for a build without an Apple Developer ID
 * — not merely a convenience. Verified on macOS 26: a Node-fetched file carries only
 * `com.apple.provenance`, which Gatekeeper does not act on.
 *
 * This script deliberately does not remove a quarantine attribute if one is present.
 * Stripping a security flag on a user's behalf is not a thing an installer should do
 * quietly, and if one appears it means an assumption here is wrong and should be seen.
 *
 * ## Integrity
 *
 * Checksums are baked into `manifest.json` inside the published package rather than
 * fetched at install time. npm is the channel being trusted; downloading a checksum
 * from the same host as the artefact would verify nothing at all.
 */

"use strict";

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const https = require("node:https");
const crypto = require("node:crypto");
const { execFileSync, spawn } = require("node:child_process");

const manifest = require("../manifest.json");

/** Highest redirect chain we will follow, so a misconfigured host cannot loop. */
const MAX_REDIRECTS = 5;

function fail(message, hint) {
  process.stderr.write(`tervin: ${message}\n`);
  if (hint) process.stderr.write(`  ${hint}\n`);
  process.exit(1);
}

/**
 * Where downloads live.
 *
 * A cache directory rather than `node_modules`: `npx` discards its temporary install,
 * and re-downloading a 12 MB app on every invocation would be rude.
 */
function cacheDir() {
  const base =
    process.platform === "darwin"
      ? path.join(os.homedir(), "Library", "Caches")
      : process.env.XDG_CACHE_HOME || path.join(os.homedir(), ".cache");
  return path.join(base, "tervin", manifest.version);
}

function artifactKey() {
  return `${process.platform}-${process.arch}`;
}

function download(url, dest, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (redirects > MAX_REDIRECTS) {
      reject(new Error(`too many redirects fetching ${url}`));
      return;
    }
    https
      .get(url, { headers: { "user-agent": "tervin-npx" } }, (response) => {
        // GitHub releases redirect to object storage; following is required.
        if (
          response.statusCode &&
          response.statusCode >= 300 &&
          response.statusCode < 400 &&
          response.headers.location
        ) {
          response.resume();
          download(response.headers.location, dest, redirects + 1).then(resolve, reject);
          return;
        }
        if (response.statusCode !== 200) {
          response.resume();
          reject(new Error(`${url} returned ${response.statusCode}`));
          return;
        }

        const total = Number(response.headers["content-length"] || 0);
        let seen = 0;
        const file = fs.createWriteStream(dest);
        response.on("data", (chunk) => {
          seen += chunk.length;
          if (total && process.stderr.isTTY) {
            const pct = Math.floor((seen / total) * 100);
            process.stderr.write(`\rtervin: downloading ${pct}%`);
          }
        });
        response.pipe(file);
        file.on("finish", () => {
          if (total && process.stderr.isTTY) process.stderr.write("\r\x1b[K");
          file.close(() => resolve());
        });
        file.on("error", reject);
      })
      .on("error", reject);
  });
}

function sha256(file) {
  const hash = crypto.createHash("sha256");
  hash.update(fs.readFileSync(file));
  return hash.digest("hex");
}

async function ensureApp() {
  const key = artifactKey();
  const entry = manifest.artifacts[key];
  if (!entry) {
    fail(
      `no build for ${key}`,
      "Tervin currently ships macOS builds only. Build from source: https://github.com/QuintinBotes/tervin",
    );
  }

  const dir = cacheDir();
  const appPath = path.join(dir, "Tervin.app");
  if (fs.existsSync(path.join(appPath, "Contents", "MacOS", "tervin"))) {
    return appPath;
  }

  fs.mkdirSync(dir, { recursive: true });
  const archive = path.join(dir, entry.file);

  if (!fs.existsSync(archive)) {
    process.stderr.write(`tervin: fetching ${manifest.version} for ${key}\n`);
    await download(manifest.baseUrl + entry.file, archive);
  }

  if (entry.sha256 && entry.sha256 !== "REPLACE_ME") {
    const actual = sha256(archive);
    if (actual !== entry.sha256) {
      // Removed, so a retry does not keep validating the same bad file.
      fs.rmSync(archive, { force: true });
      fail(
        "the download did not match the checksum published with this npm version",
        `expected ${entry.sha256}, got ${actual}`,
      );
    }
  } else {
    process.stderr.write(
      "tervin: this package was published without a checksum, so the download was not verified\n",
    );
  }

  // `tar` rather than a JS extractor: it is present everywhere this runs, and it
  // preserves the symlinks and executable bits inside a macOS bundle, which several
  // pure-JS extractors quietly do not.
  execFileSync("tar", ["-xzf", archive, "-C", dir], { stdio: "inherit" });

  if (!fs.existsSync(appPath)) {
    fail(`the archive did not contain Tervin.app`, `looked in ${dir}`);
  }

  // Reported, never removed. If this fires, an assumption in the comment at the top of
  // this file is wrong, and silently stripping a security attribute would hide that.
  try {
    const attrs = execFileSync("xattr", [appPath], { encoding: "utf8" });
    if (attrs.includes("com.apple.quarantine")) {
      process.stderr.write(
        "tervin: this download carries a quarantine flag, so macOS may refuse to open it.\n" +
          "  Tervin will not remove it for you. Approve the app in System Settings →\n" +
          "  Privacy & Security, or report this — it is not expected on this install route.\n",
      );
    }
  } catch {
    // `xattr` missing is not a reason to stop.
  }

  return appPath;
}

async function main() {
  const args = process.argv.slice(2);

  if (args.includes("--help") || args.includes("-h")) {
    process.stdout.write(
      [
        "tervin — the agent-native terminal workspace",
        "",
        "  npx tervin              download if needed, then launch",
        "  npx tervin --install    copy into /Applications and launch from there",
        "  npx tervin --where      print the cached bundle's path",
        "  npx tervin --clean      remove the cached download",
        "",
        "Installing this way avoids macOS Gatekeeper: quarantine is applied by the",
        "downloading application, and Node does not set it. A browser download of the",
        "same file would need approval in System Settings.",
        "",
      ].join("\n"),
    );
    return;
  }

  if (process.platform !== "darwin") {
    fail(
      `${process.platform} is not supported yet`,
      "Tervin is written for Unix generally but only tested on macOS. Untested is not supported.",
    );
  }

  if (args.includes("--clean")) {
    fs.rmSync(cacheDir(), { recursive: true, force: true });
    process.stdout.write(`tervin: removed ${cacheDir()}\n`);
    return;
  }

  const cached = await ensureApp();

  if (args.includes("--where")) {
    process.stdout.write(`${cached}\n`);
    return;
  }

  let target = cached;
  if (args.includes("--install")) {
    target = "/Applications/Tervin.app";
    // `ditto` rather than `cp -r`: it is the supported way to copy a bundle, and it
    // preserves the resource forks and extended attributes `cp` can drop.
    execFileSync("rm", ["-rf", target]);
    execFileSync("ditto", [cached, target], { stdio: "inherit" });
    process.stdout.write(`tervin: installed to ${target}\n`);
  }

  // `open` rather than exec'ing the binary directly: a bundle launched through
  // LaunchServices gets its Info.plist, its icon, and a proper application lifecycle.
  // Running the inner Mach-O straight leaves the web view without them.
  const child = spawn("open", ["-a", target, "--args", ...args.filter(isPassThrough)], {
    stdio: "inherit",
    detached: true,
  });
  child.on("error", (e) => fail(`could not launch ${target}: ${e.message}`));
  child.unref();
}

/** Arguments meant for Tervin rather than for this installer. */
function isPassThrough(arg) {
  return !["--install", "--where", "--clean"].includes(arg);
}

main().catch((e) => fail(e.message));
