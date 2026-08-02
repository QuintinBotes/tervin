#!/usr/bin/env bash
#
# Build the release artefacts and print everything the Homebrew cask needs.
#
# Exists so a release is one reviewable command rather than a sequence someone has to
# remember correctly. It refuses to produce artefacts it cannot describe accurately:
# an unsigned build is reported as unsigned rather than shipped quietly.
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
echo "Tervin ${VERSION}"
echo

# A universal binary so one download works on Apple silicon and Intel. Building both
# and shipping one is the whole reason the cask has per-architecture URLs.
TARGET="universal-apple-darwin"
for arch in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target add "$arch" >/dev/null 2>&1 || true
done

echo "==> Testing before building. A release that was never tested is not a release."
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npx vitest run

echo
echo "==> Building"
npx tauri build --target "$TARGET" --bundles app,dmg

BUNDLE="target/${TARGET}/release/bundle"
APP="${BUNDLE}/macos/Tervin.app"

echo
echo "==> Signing status"
if codesign -dv --verbose=2 "$APP" 2>&1 | grep -q "Authority=Developer ID"; then
  echo "signed with a Developer ID"
  echo "==> Notarisation status"
  # `spctl` is what Gatekeeper itself consults, so this is the check that matters.
  spctl -a -vvv -t install "$APP" 2>&1 || echo "NOT notarised — macOS will warn on first launch"
else
  echo "NOT signed. macOS will warn on first launch."
  echo "Set APPLE_SIGNING_IDENTITY, APPLE_ID, APPLE_PASSWORD, and APPLE_TEAM_ID to sign."
fi

echo
echo "==> Tarball for the npm and curl routes"
# Those routes do not want a disk image — and a file fetched by curl or Node carries no
# quarantine attribute, which is what lets an unsigned build open without a prompt.
tar -czf "${BUNDLE}/dmg/Tervin-${VERSION}-macos-universal.tar.gz" -C "${BUNDLE}/macos" Tervin.app
ls -lh "${BUNDLE}/dmg/"

echo
echo "==> Checksums for the Homebrew cask and the npm manifest"
for artifact in "${BUNDLE}"/dmg/*.dmg "${BUNDLE}"/dmg/*.tar.gz; do
  [ -e "$artifact" ] || continue
  printf '%s\n  %s\n' "$(basename "$artifact")" "$(shasum -a 256 "$artifact" | cut -d' ' -f1)"
done

echo
echo "Next:"
echo "  1. packaging/homebrew/tervin.rb        — version and DMG checksums above"
echo "  2. packaging/homebrew/tervin-formula.rb — source tarball checksum"
echo "  3. packaging/npm/manifest.json         — version, baseUrl, and the tar.gz checksum"
echo
echo "Then: brew audit --cask --strict packaging/homebrew/tervin.rb"
echo "      (cd packaging/npm && npm publish --dry-run)"
echo
echo "The npm manifest checksum is not optional in practice: without it the installer"
echo "warns on every run that the download was not verified."
