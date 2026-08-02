//! Codex: OpenAI's coding agent, over `codex exec --json`.
//!
//! Codex prints structured JSONL, which makes it a Tier 1 integration for *reading*: every
//! message, command, file change and token count arrives as data rather than as text to be
//! scraped off a screen.
//!
//! What it is not is steerable. `codex exec` is one non-interactive turn — there is no
//! channel to send a follow-up prompt down, and no permission request to answer, because
//! the sandbox and approval policy are decided by the flags it was launched with. So
//! Tervin reports permissions as something it cannot gate here, rather than showing an
//! approval control that would never fire.
//!
//! See [`normalize`] for what the wire format was verified against, and what could not be.

pub mod normalize;

pub use normalize::CodexNormalizer;

/// Flags Tervin passes to every `codex exec` run, and why.
///
/// Kept in one place so the launch path and the documentation cannot drift apart.
pub const EXEC_ARGS: &[&str] = &[
    "exec",
    // The whole integration depends on this. Without it Codex prints for a human.
    "--json",
    // Tervin runs in directories that are not always repositories — a scratch folder, a
    // subdirectory of one. Refusing to start there would be a worse default than warning.
    "--skip-git-repo-check",
];
