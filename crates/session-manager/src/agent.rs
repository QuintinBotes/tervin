//! Which SSH keys are loaded, so a prompt is never a surprise.
//!
//! ## Why this is not a credential store
//!
//! The obvious feature here is "let Tervin remember your SSH passphrase". It is the wrong
//! feature. Storing someone's passphrase makes Tervin a place worth attacking, duplicates
//! something the operating system already does properly, and buys nothing that
//! `ssh-add --apple-use-keychain` does not already give you.
//!
//! The actual problem is narrower and more annoying: you open a connection, and it stops
//! to ask for a passphrase you were not expecting, because the key is not in the agent.
//! Knowing that *before* connecting is the whole win, and it needs no secret storage at
//! all.
//!
//! ## Nothing here reads a private key
//!
//! Identity is established by fingerprint. `ssh-add -l` reports the fingerprints the agent
//! holds; `ssh-keygen -lf` computes one from a **public** key file. Comparing those two
//! answers the question exactly, and the private key is never opened, never read, and
//! never held in memory.
//!
//! Matching by the agent's comment field would have been easier and is what a quick
//! implementation does. It is also wrong: the comment is free text, usually but not always
//! the path, and a host whose key is loaded under a different comment would be reported as
//! missing.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One identity the agent is holding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedKey {
    /// `SHA256:…`, as both `ssh-add` and `ssh-keygen` print it.
    pub fingerprint: String,
    /// Free text the key was created with. Shown, never matched on.
    pub comment: String,
}

/// What the agent is doing, as far as Tervin can tell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentState {
    /// Reachable, with these identities loaded.
    Running { keys: Vec<LoadedKey> },
    /// Reachable and empty. Distinct from unreachable: every key will prompt, and that is
    /// a different thing to tell someone than "I could not ask".
    NoIdentities,
    /// No agent, or `SSH_AUTH_SOCK` is not set.
    NotRunning,
    /// `ssh-add` could not be run at all.
    Unavailable { reason: String },
}

/// What Tervin can say about one host's key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum KeyStatus {
    /// The key this host names is in the agent. Connecting will not prompt for it.
    Loaded { comment: String },
    /// The key exists but is not loaded, so connecting will ask for its passphrase.
    NotLoaded { path: String },
    /// The host names no `IdentityFile`, so ssh will try its defaults and Tervin cannot
    /// say which key applies without reimplementing ssh's own selection.
    NoIdentityNamed,
    /// The public half is missing, so its fingerprint cannot be computed.
    ///
    /// Reported rather than treated as "not loaded": the key may well be in the agent, and
    /// saying it is not would send someone looking for a problem that is not there.
    CannotFingerprint { path: String, reason: String },
    /// The agent could not be consulted, so nothing is known either way.
    Unknown,
}

impl KeyStatus {
    /// One line for the UI.
    pub fn summary(&self) -> String {
        match self {
            Self::Loaded { comment } if comment.is_empty() => "key loaded".to_string(),
            Self::Loaded { comment } => format!("key loaded ({comment})"),
            Self::NotLoaded { .. } => "key not loaded, will ask for a passphrase".to_string(),
            Self::NoIdentityNamed => "no key named; ssh will pick".to_string(),
            Self::CannotFingerprint { reason, .. } => format!("cannot check: {reason}"),
            Self::Unknown => "agent not reachable".to_string(),
        }
    }
}

/// Ask the agent what it is holding.
pub fn agent_state() -> AgentState {
    // Without this, `ssh-add` falls back to a default socket path and can hang.
    if std::env::var_os("SSH_AUTH_SOCK").is_none() {
        return AgentState::NotRunning;
    }

    let output = match Command::new("ssh-add")
        .arg("-l")
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            return AgentState::Unavailable {
                reason: format!("could not run ssh-add: {e}"),
            }
        }
    };

    match output.status.code() {
        // Reachable and empty. `ssh-add` uses 1 for this.
        Some(1) => AgentState::NoIdentities,
        Some(0) => {
            let keys = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(parse_key_line)
                .collect::<Vec<_>>();
            if keys.is_empty() {
                AgentState::NoIdentities
            } else {
                AgentState::Running { keys }
            }
        }
        // 2 is "cannot connect", and anything else is not something to guess at.
        _ => AgentState::NotRunning,
    }
}

/// `256 SHA256:abc… a comment here (ED25519)`
///
/// The comment can contain spaces, so it is whatever sits between the fingerprint and the
/// trailing parenthesised type rather than a fixed field.
fn parse_key_line(line: &str) -> Option<LoadedKey> {
    let line = line.trim();
    let mut parts = line.splitn(3, ' ');
    let _bits = parts.next()?;
    let fingerprint = parts.next()?.to_string();
    if !fingerprint.starts_with("SHA256:") && !fingerprint.starts_with("MD5:") {
        return None;
    }
    let rest = parts.next().unwrap_or("").trim();
    // The trailing `(ED25519)` is the key type, not part of the comment. A key with no
    // comment leaves the type as the entire remainder, which must read as empty rather
    // than as a comment of "(ED25519)".
    let comment = match rest.rfind(" (") {
        Some(i) => &rest[..i],
        None if rest.starts_with('(') && rest.ends_with(')') => "",
        None => rest,
    };
    Some(LoadedKey {
        fingerprint,
        comment: comment.trim().to_string(),
    })
}

/// Whether this host's key is in the agent.
pub fn key_status(host: &crate::ssh::SshHost, agent: &AgentState) -> KeyStatus {
    let Some(identity) = host.identity_file.as_deref() else {
        return KeyStatus::NoIdentityNamed;
    };

    let keys = match agent {
        AgentState::Running { keys } => keys,
        // Reachable and empty is a definite answer: nothing is loaded, so this will prompt.
        AgentState::NoIdentities => {
            return KeyStatus::NotLoaded {
                path: identity.to_string(),
            }
        }
        AgentState::NotRunning | AgentState::Unavailable { .. } => return KeyStatus::Unknown,
    };

    let path = expand_tilde(identity);
    match fingerprint_of(&path) {
        Ok(fingerprint) => match keys.iter().find(|k| k.fingerprint == fingerprint) {
            Some(key) => KeyStatus::Loaded {
                comment: key.comment.clone(),
            },
            None => KeyStatus::NotLoaded {
                path: identity.to_string(),
            },
        },
        Err(reason) => KeyStatus::CannotFingerprint {
            path: identity.to_string(),
            reason,
        },
    }
}

/// Fingerprint a key from its **public** half.
///
/// `ssh-keygen -lf` accepts either half, but Tervin only ever hands it the `.pub`: reading
/// a private key is not something this program needs to do, so it does not.
fn fingerprint_of(identity: &Path) -> Result<String, String> {
    let public = if identity.extension().is_some_and(|e| e == "pub") {
        identity.to_path_buf()
    } else {
        PathBuf::from(format!("{}.pub", identity.display()))
    };

    if !public.exists() {
        return Err(format!("{} is not there", public.display()));
    }

    let output = Command::new("ssh-keygen")
        .arg("-l")
        .arg("-f")
        .arg(&public)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run ssh-keygen: {e}"))?;

    if !output.status.success() {
        return Err(format!("ssh-keygen could not read {}", public.display()));
    }

    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| "ssh-keygen printed nothing recognisable".to_string())
}

fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(rest),
            None => PathBuf::from(path),
        },
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::SshHost;

    fn host(identity: Option<&str>) -> SshHost {
        SshHost {
            alias: "build-box".to_string(),
            identity_file: identity.map(str::to_string),
            ..Default::default()
        }
    }

    fn key(fingerprint: &str, comment: &str) -> LoadedKey {
        LoadedKey {
            fingerprint: fingerprint.to_string(),
            comment: comment.to_string(),
        }
    }

    #[test]
    fn parses_the_real_ssh_add_format() {
        // Copied from actual `ssh-add -l` output rather than written from the man page.
        let parsed = parse_key_line(
            "256 SHA256:Jsi8kCNV0cGQriMOiUwSvRYALZAQfN6h1teHjlRHcFA someone@example.com (ED25519)",
        )
        .expect("should parse");
        assert_eq!(
            parsed.fingerprint,
            "SHA256:Jsi8kCNV0cGQriMOiUwSvRYALZAQfN6h1teHjlRHcFA"
        );
        assert_eq!(parsed.comment, "someone@example.com");
    }

    #[test]
    fn a_comment_containing_spaces_survives() {
        // Comments are free text, and "work laptop key" is a perfectly ordinary one.
        let parsed = parse_key_line("4096 SHA256:abc work laptop key (RSA)").unwrap();
        assert_eq!(parsed.comment, "work laptop key");
    }

    #[test]
    fn a_key_with_no_comment_parses_without_swallowing_the_type() {
        let parsed = parse_key_line("256 SHA256:abc (ED25519)").unwrap();
        assert_eq!(parsed.comment, "");
    }

    #[test]
    fn a_line_that_is_not_a_key_is_ignored() {
        assert!(parse_key_line("The agent has no identities.").is_none());
        assert!(parse_key_line("").is_none());
        assert!(parse_key_line("garbage").is_none());
    }

    #[test]
    fn a_host_naming_no_key_says_ssh_will_choose() {
        // Reimplementing ssh's own identity selection to guess would be a large amount of
        // work to produce an answer that is sometimes wrong.
        let agent = AgentState::Running { keys: vec![] };
        assert_eq!(key_status(&host(None), &agent), KeyStatus::NoIdentityNamed);
    }

    #[test]
    fn an_unreachable_agent_means_unknown_rather_than_not_loaded() {
        // The distinction that matters: "not loaded" tells someone to run ssh-add, and
        // saying that when Tervin simply could not ask would send them somewhere useless.
        for agent in [
            AgentState::NotRunning,
            AgentState::Unavailable {
                reason: "no ssh-add".to_string(),
            },
        ] {
            assert_eq!(
                key_status(&host(Some("~/.ssh/id_ed25519")), &agent),
                KeyStatus::Unknown
            );
        }
    }

    #[test]
    fn an_empty_agent_is_a_definite_not_loaded() {
        // Reachable and holding nothing is an answer, not an absence of one.
        match key_status(&host(Some("~/.ssh/id_ed25519")), &AgentState::NoIdentities) {
            KeyStatus::NotLoaded { path } => assert_eq!(path, "~/.ssh/id_ed25519"),
            other => panic!("expected a definite not-loaded, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_public_half_is_reported_rather_than_called_not_loaded() {
        // The key may well be in the agent. Claiming otherwise sends someone looking for a
        // problem that is not there.
        let dir = tempfile::tempdir().unwrap();
        let identity = dir.path().join("id_absent");
        let agent = AgentState::Running {
            keys: vec![key("SHA256:something", "")],
        };
        match key_status(&host(Some(&identity.display().to_string())), &agent) {
            KeyStatus::CannotFingerprint { reason, .. } => assert!(reason.contains("not there")),
            other => panic!("expected a cannot-check, got {other:?}"),
        }
    }

    /// The real thing: generate a key, fingerprint it, and check both answers.
    #[test]
    fn a_generated_key_is_recognised_when_loaded_and_not_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let identity = dir.path().join("id_test");
        let generated = Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-N", "", "-C", "tervin test key", "-f"])
            .arg(&identity)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !matches!(generated, Ok(status) if status.success()) {
            // No ssh-keygen here; the parsing tests still cover the logic.
            return;
        }

        let fingerprint =
            fingerprint_of(&identity).expect("a freshly generated key must fingerprint");
        assert!(fingerprint.starts_with("SHA256:"));

        // Loaded: matched by fingerprint, and the comment comes back for display.
        let loaded = AgentState::Running {
            keys: vec![key(&fingerprint, "tervin test key")],
        };
        assert_eq!(
            key_status(&host(Some(&identity.display().to_string())), &loaded),
            KeyStatus::Loaded {
                comment: "tervin test key".to_string()
            }
        );

        // A different key in the agent is not this one, however similar the comment.
        let other = AgentState::Running {
            keys: vec![key("SHA256:completely-different", "tervin test key")],
        };
        assert!(matches!(
            key_status(&host(Some(&identity.display().to_string())), &other),
            KeyStatus::NotLoaded { .. }
        ));
    }

    #[test]
    fn matching_is_by_fingerprint_and_not_by_comment() {
        // The shortcut a quick implementation takes. A key loaded under a different comment
        // would be reported missing, and a passphrase prompt would then arrive anyway.
        let dir = tempfile::tempdir().unwrap();
        let identity = dir.path().join("id_test");
        let ok = Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "the original comment",
                "-f",
            ])
            .arg(&identity)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !matches!(ok, Ok(status) if status.success()) {
            return;
        }

        let fingerprint = fingerprint_of(&identity).unwrap();
        // Same key, wholly unrelated comment, as happens when it was added elsewhere.
        let agent = AgentState::Running {
            keys: vec![key(&fingerprint, "added on another machine")],
        };
        assert!(
            matches!(
                key_status(&host(Some(&identity.display().to_string())), &agent),
                KeyStatus::Loaded { .. }
            ),
            "a key loaded under a different comment is still loaded"
        );
    }

    #[test]
    fn every_status_has_a_summary_and_only_a_missing_key_mentions_a_passphrase() {
        let statuses = [
            KeyStatus::Loaded {
                comment: "me@host".to_string(),
            },
            KeyStatus::NotLoaded {
                path: "~/.ssh/id".to_string(),
            },
            KeyStatus::NoIdentityNamed,
            KeyStatus::CannotFingerprint {
                path: "~/.ssh/id".to_string(),
                reason: "not there".to_string(),
            },
            KeyStatus::Unknown,
        ];
        for status in &statuses {
            assert!(!status.summary().is_empty(), "{status:?} has no summary");
        }
        assert_eq!(
            statuses
                .iter()
                .filter(|s| s.summary().contains("passphrase"))
                .count(),
            1
        );
    }

    #[test]
    fn asking_a_real_agent_never_panics_whatever_it_answers() {
        // CI has no agent; a developer machine has one. Both must be fine, and neither
        // result is asserted because both are legitimate.
        let state = agent_state();
        match state {
            AgentState::Running { ref keys } => assert!(!keys.is_empty()),
            AgentState::NoIdentities | AgentState::NotRunning => {}
            AgentState::Unavailable { ref reason } => assert!(!reason.is_empty()),
        }
    }
}
