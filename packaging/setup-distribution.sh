#!/usr/bin/env bash
#
# One-time setup for the distribution channels.
#
# Everything after this is automatic: a `v*` tag builds, releases, publishes to npm, and
# updates the Homebrew tap with checksums computed during that build. This script exists
# because the accounts and repositories cannot be created by a workflow that needs them
# to already exist.
set -euo pipefail

REPO="QuintinBotes/tervin"

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
ok()  { printf '  \xe2\x9c\x93 %s\n' "$*"; }
todo(){ printf '  \xe2\x86\x92 %s\n' "$*"; }
# A deliberate non-configuration, so it reads as settled rather than outstanding. Using
# todo() here would put "signing" on a checklist it is never coming off.
note(){ printf '  \xc2\xb7 %s\n' "$*"; }

command -v gh >/dev/null || {
  echo "This needs the GitHub CLI: brew install gh && gh auth login"
  exit 1
}

say "1. The repository"
if gh repo view "$REPO" >/dev/null 2>&1; then
  ok "$REPO exists"
else
  todo "creating $REPO"
  gh repo create "$REPO" --public --source=. --remote=origin --push
fi

say "2. The Homebrew tap"
# There is no second repository. A tap is a git repo with casks in `Casks/` and formulae
# in `Formula/`, and `brew tap <name> <url>` accepts any URL — the `homebrew-` prefix is
# only what the one-argument shortcut assumes.
ok "this repository is the tap (Casks/, Formula/)"
todo "users install with:"
todo "  brew tap quintinbotes/tervin https://github.com/QuintinBotes/tervin"
todo "  brew install --cask tervin"

say "3. Letting the release workflow update the tap"
# The cask is committed to the default branch, which is protected. `GITHUB_TOKEN` cannot
# do it: the GitHub Actions app can only bypass a ruleset on an organisation-owned
# repository, and a pull request opened with that token does not trigger CI, so its
# required checks would never report. A deploy key is the narrowest thing that works —
# one repository, no user identity, no reach into the rest of the account.
if gh api "repos/${REPO}/branches/main/protection" >/dev/null 2>&1; then
  todo "main still uses legacy branch protection, which nothing can push through."
  todo "  Replace it with a ruleset (same rules, plus a DeployKey bypass actor), then:"
  todo "    gh api -X DELETE repos/${REPO}/branches/main/protection"
else
  ok "main is not using legacy branch protection"
fi

bypass=""
for id in $(gh api "repos/${REPO}/rulesets" --jq '.[].id' 2>/dev/null); do
  found="$(gh api "repos/${REPO}/rulesets/${id}" \
    --jq '.bypass_actors[]? | select(.actor_type == "DeployKey") | .actor_type' 2>/dev/null)"
  [ -n "$found" ] && bypass="yes"
done
if [ -n "$bypass" ]; then
  ok "a ruleset lets a deploy key update the tap"
else
  todo "no ruleset grants a DeployKey bypass, so the homebrew job will fail at the push"
fi

if gh repo deploy-key list --repo "$REPO" 2>/dev/null | grep -q read-write; then
  ok "a read-write deploy key exists"
else
  todo "no read-write deploy key. Create one:"
  todo "  ssh-keygen -t ed25519 -N '' -C tervin-release-tap -f /tmp/tap_key"
  todo "  gh repo deploy-key add /tmp/tap_key.pub --repo $REPO --allow-write"
  todo "  gh secret set TAP_DEPLOY_KEY --repo $REPO < /tmp/tap_key && rm -f /tmp/tap_key*"
fi

say "4. Secrets"
existing="$(gh secret list --repo "$REPO" 2>/dev/null | awk '{print $1}' || true)"
have() { echo "$existing" | grep -qx "$1"; }

if have NPM_TOKEN; then
  ok "NPM_TOKEN set"
else
  todo "NPM_TOKEN: an npm automation token (npmjs.com › Access Tokens › Granular, publish scope)"
  todo "  gh secret set NPM_TOKEN --repo $REPO"
fi

if have TAP_DEPLOY_KEY; then
  ok "TAP_DEPLOY_KEY set"
else
  todo "TAP_DEPLOY_KEY: the private half of the read-write deploy key above"
fi

# Signing is deliberately not done, so its absence is reported as a decision rather than
# as an outstanding task. A fork with a Developer ID can still set these and the release
# workflow will use them. See SECURITY.md for why Tervin itself does not.
if have APPLE_SIGNING_IDENTITY; then
  ok "Apple signing configured"
else
  note "Apple signing not configured, which is intended and not a missing step."
  note "  npx, the curl installer, and 'brew install --formula' open with no prompt"
  note "  regardless, because quarantine is set by the downloading application."
  note "  Only a browser-downloaded .dmg and the cask ask for approval."
  note "  A fork with a Developer ID can set: APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD"
  note "                  APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID"
fi

say "5. Releasing"
V="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
todo "the workflow requires the tag to match the workspace version, currently ${V}"
todo "  git tag v${V} && git push origin v${V}"
echo
todo "a manual run builds and verifies without publishing:"
todo "  gh workflow run release.yml --repo $REPO"
echo
