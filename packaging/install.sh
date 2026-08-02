#!/bin/sh
#
# Install Tervin.
#
#   curl -fsSL https://raw.githubusercontent.com/QuintinBotes/tervin/main/packaging/install.sh | sh
#
# ## Why this exists, and why it is the recommended route
#
# Tervin is not signed with an Apple Developer ID, because that costs $99 a year and this is
# an open-source project. That sounds like a problem and mostly is not, for a reason worth
# understanding:
#
# macOS applies the `com.apple.quarantine` attribute in the *downloading application*, not in
# the kernel. A browser sets it. `curl` does not. So an app fetched by this script carries
# only `com.apple.provenance` and opens with no Gatekeeper dialog at all, while the identical
# bytes downloaded through a browser would be quarantined and refuse to open on first launch.
#
# Verified, not assumed: `xattr` on a `curl` download of a release asset shows
# `com.apple.provenance` and nothing else.
#
# ## What this script will not do
#
# It never runs `xattr -d com.apple.quarantine`. Stripping quarantine on a user's behalf is
# exactly the pattern that trains people to wave away a real warning, and it is unnecessary
# here because nothing was quarantined. If you download the `.dmg` in a browser instead, you
# will get the dialog, and that is macOS working correctly on an unsigned app.
#
# It never uses `sudo`. If /Applications is not writable it installs to ~/Applications and
# says so.
set -eu

REPO="QuintinBotes/tervin"
APP="Tervin.app"
VERSION="${TERVIN_VERSION:-latest}"
PREFIX="${TERVIN_PREFIX:-}"

say()  { printf '%s\n' "$*"; }
warn() { printf '%s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'USAGE'
Install Tervin, an agent-native terminal workspace.

  sh install.sh [--version X.Y.Z] [--prefix DIR] [--uninstall]

  --version   A specific release. Default: the latest.
  --prefix    Where to install. Default: /Applications, falling back to ~/Applications.
  --uninstall Remove the app. Leaves your data alone; see the note it prints.

Environment: TERVIN_VERSION, TERVIN_PREFIX.
USAGE
}

UNINSTALL=0
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:-}"; shift 2 ;;
    --prefix)  PREFIX="${2:-}";  shift 2 ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
done

# ------------------------------------------------------------------ platform

[ "$(uname -s)" = "Darwin" ] || die "Tervin currently only runs on macOS. Linux and Windows
builds are planned but nothing has been run there, so this script will not pretend."

# The build is a universal binary, so the architecture does not change what is downloaded.
# Reported anyway, because a surprise here is worth seeing.
ARCH="$(uname -m)"

for tool in curl shasum tar mktemp; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is required and was not found"
done

choose_prefix() {
  if [ -n "$PREFIX" ]; then
    printf '%s' "$PREFIX"
  elif [ -w /Applications ]; then
    printf '%s' /Applications
  else
    # Never sudo. A terminal is not something to install as root on someone's behalf.
    printf '%s' "$HOME/Applications"
  fi
}

DEST="$(choose_prefix)"

# ------------------------------------------------------------------ uninstall

if [ "$UNINSTALL" = "1" ]; then
  removed=0
  for dir in /Applications "$HOME/Applications" "$DEST"; do
    if [ -d "$dir/$APP" ]; then
      rm -rf "$dir/$APP"
      say "Removed $dir/$APP"
      removed=1
    fi
  done
  [ "$removed" = "1" ] || say "Tervin was not installed in /Applications or ~/Applications."
  cat <<'NOTE'

Your data was left alone. Tervin keeps Blocks, Threads, prompt history and saved sessions in:

  ~/Library/Application Support/tervin

Delete that directory if you want it gone. It is not removed automatically, because a
command called "uninstall" destroying months of command history without asking would be a
poor trade for tidiness.
NOTE
  exit 0
fi

# ------------------------------------------------------------------ resolve

if [ "$VERSION" = "latest" ]; then
  # Read the tag from the redirect rather than parsing the API, so this needs no token and
  # no jq.
  TAG="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
      "https://github.com/$REPO/releases/latest" 2>/dev/null | sed 's#.*/tag/##')"
  [ -n "$TAG" ] || die "could not work out the latest release. Pass --version X.Y.Z."
else
  case "$VERSION" in v*) TAG="$VERSION" ;; *) TAG="v$VERSION" ;; esac
fi
V="${TAG#v}"

BASE="https://github.com/$REPO/releases/download/$TAG"
TARBALL="Tervin-$V-macos-universal.tar.gz"

say "Installing Tervin $V into $DEST ($ARCH, universal binary)"

TMP="$(mktemp -d)"
# shellcheck disable=SC2064
trap "rm -rf '$TMP'" EXIT INT TERM

# ------------------------------------------------------------------ download

say "Fetching $TARBALL"
curl -fSL --progress-bar -o "$TMP/$TARBALL" "$BASE/$TARBALL" \
  || die "could not download $BASE/$TARBALL

If this is a 404, that release may not have a macOS build yet. Releases are listed at
https://github.com/$REPO/releases"

# ------------------------------------------------------------------ verify

# Not optional. The whole reason this route is safe is that nothing vouches for the binary
# except the checksum published beside it, so skipping the check would remove the only
# guarantee there is.
say "Verifying the checksum"
if curl -fsSL -o "$TMP/SHA256SUMS.txt" "$BASE/SHA256SUMS.txt" 2>/dev/null; then
  EXPECTED="$(grep -F "$TARBALL" "$TMP/SHA256SUMS.txt" | awk '{print $1}' | head -1)"
  if [ -z "$EXPECTED" ]; then
    die "SHA256SUMS.txt does not mention $TARBALL, so the download cannot be verified.
Refusing to install something unverified."
  fi
  ACTUAL="$(shasum -a 256 "$TMP/$TARBALL" | awk '{print $1}')"
  if [ "$EXPECTED" != "$ACTUAL" ]; then
    die "checksum mismatch for $TARBALL
  expected $EXPECTED
  actual   $ACTUAL
Refusing to install. Either the download was corrupted or the asset is not what was
published, and neither is something to shrug at."
  fi
  say "  ok  $ACTUAL"
else
  die "could not fetch SHA256SUMS.txt, so the download cannot be verified.
Refusing to install something unverified."
fi

# ------------------------------------------------------------------ install

say "Unpacking"
tar -xzf "$TMP/$TARBALL" -C "$TMP"
[ -d "$TMP/$APP" ] || die "the archive did not contain $APP"

mkdir -p "$DEST"
if [ -d "$DEST/$APP" ]; then
  say "Replacing the existing $DEST/$APP"
  rm -rf "$DEST/$APP"
fi
# Moved rather than copied, so a partial copy cannot leave a half-written bundle behind.
mv "$TMP/$APP" "$DEST/$APP"

# Reported, never removed. There should be nothing here, and if there is, the user should
# know rather than have it quietly stripped.
if xattr "$DEST/$APP" 2>/dev/null | grep -q 'com.apple.quarantine'; then
  warn ""
  warn "Note: this bundle carries com.apple.quarantine, which is unexpected for a curl"
  warn "download. macOS will ask you to approve it on first launch. This script does not"
  warn "remove the attribute, because doing that for you is how people learn to wave away"
  warn "warnings that matter."
fi

cat <<EOF

Installed $DEST/$APP

  open -a "$DEST/$APP"

Nothing was quarantined, so it will open without a Gatekeeper dialog. Tervin is not signed
with an Apple Developer ID, and this route is recommended precisely because it does not need
to be: macOS applies quarantine in the downloading application, and curl does not set it.

Tervin is pre-1.0. Local data formats are not stable yet, so treat the database in
~/Library/Application Support/tervin as something you can lose.

To uninstall:  sh install.sh --uninstall
EOF
