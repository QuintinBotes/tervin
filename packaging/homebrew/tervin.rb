# Homebrew cask for Tervin.
#
# Lives in this repository rather than only in a tap so the packaging is reviewable
# alongside the code it packages, and so `brew audit` can be run before a release
# rather than after a user finds the problem.
#
# To publish: copy into a tap as `Casks/tervin.rb`, replacing the placeholders below
# with the values printed by `packaging/release.sh`.
cask "tervin" do
  version "0.1.0"

  # Per-architecture checksums. A single `:no_check` would mean Homebrew installs
  # whatever the URL currently serves, which defeats the point of a checksum.
  on_arm do
    sha256 "REPLACE_WITH_ARM64_SHA256"
    url "https://github.com/QuintinBotes/tervin/releases/download/v#{version}/Tervin_#{version}_aarch64.dmg",
        verified: "github.com/QuintinBotes/tervin/"
  end
  on_intel do
    sha256 "REPLACE_WITH_X86_64_SHA256"
    url "https://github.com/QuintinBotes/tervin/releases/download/v#{version}/Tervin_#{version}_x64.dmg",
        verified: "github.com/QuintinBotes/tervin/"
  end

  name "Tervin"
  desc "Agent-native terminal workspace"
  homepage "https://github.com/QuintinBotes/tervin"

  # Pre-1.0: a minor release may change on-disk formats, so an auto-upgrade should be
  # a deliberate act until the schema settles.
  livecheck do
    url :url
    strategy :github_latest
  end

  # The bundle declares 10.13, but Tervin is only tested on Sonoma and later and its
  # PTY tests run against the tools those versions ship. Claiming the lower bound the
  # linker happens to allow would be claiming support that was never exercised.
  depends_on macos: ">= :sonoma"

  app "Tervin.app"

  # Everything Tervin writes, so an uninstall actually uninstalls. Listed explicitly
  # rather than with a glob: a wildcard under Application Support has removed more
  # than it should in other casks.
  # Paths verified against a built bundle: the identifier is `dev.tervin.app`, and on
  # macOS Tervin's config and data both live under Application Support because that is
  # what `dirs` returns there — there is no `~/.config/tervin` on this platform.
  zap trash: [
    "~/Library/Application Support/tervin",
    "~/Library/Caches/dev.tervin.app",
    "~/Library/Saved Application State/dev.tervin.app.savedState",
    "~/Library/WebKit/dev.tervin.app",
  ]

  caveats do
    # Said here rather than discovered at first launch. An unsigned build shows a
    # Gatekeeper warning, and pretending otherwise wastes a user's trust.
    <<~EOS
      Tervin injects its shell integration per pane and never modifies your rc files.
      Disable it with TERVIN_SHELL_INTEGRATION=0 or in Settings.

      Tervin is a terminal, not a sandbox. Agents run with your privileges. See
      SECURITY.md for what its permission gates can and cannot enforce.
    EOS
  end
end
