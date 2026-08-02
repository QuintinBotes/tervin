# tervin

**The agent-native terminal workspace.** A terminal that treats coding agents as
first-class inhabitants — and never lies to you about what they are doing.

```sh
npx tervin
```

## Why install this way

macOS applies its `com.apple.quarantine` attribute in the *downloading application* — a
browser sets it, `curl` and Node do not. So a build fetched by `npx` opens normally,
while the identical file downloaded through a browser hits Gatekeeper's "unidentified
developer" wall and needs approving in System Settings.

Checksums are baked into this package rather than fetched at install time: npm is the
channel being trusted, and downloading a checksum from the same host as the artefact
would verify nothing.

This installer never removes a quarantine attribute on your behalf. If one appears, it
says so — silently stripping a security flag is not a thing an installer should do.

## Commands

```sh
npx tervin              # download if needed, then launch
npx tervin --install    # copy into /Applications and launch from there
npx tervin --where      # print the cached bundle's path
npx tervin --clean      # remove the cached download
```

## What it is

A real terminal first — `vim`, `less`, `tmux`, `ssh`, oh-my-zsh, powerlevel10k, Sixel
images, bracketed paste — plus three things a normal terminal cannot do:

- **Blocks.** Every command becomes a searchable unit with its output, exit code,
  duration, diagnostics, and test results.
- **Threads.** Every agent — Claude Code, any ACP agent, local models — normalises into
  one provider-neutral event stream.
- **Tervin Rules.** Risk classification and, where the runtime allows it, a real
  pre-execution gate. Approving `rm -rf build` never approves `rm -rf /`.

macOS only for now. The code is written for Unix generally, but untested is not
supported.

Source, documentation, and issues: https://github.com/QuintinBotes/tervin
