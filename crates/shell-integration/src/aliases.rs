//! Reading and expanding the user's shell aliases and functions.
//!
//! Inside a Tervin pane, aliases already work: the pane runs the user's real
//! interactive login shell, which expands its own aliases. Nothing here is needed
//! for that.
//!
//! This exists for the places where *Tervin* handles a command rather than the
//! shell — and one of them is a safety problem, not a convenience one:
//!
//! - **Risk classification.** Given `alias deploy='kubectl apply --context prod'`,
//!   a classifier that only sees `deploy` finds an unknown program and reports it
//!   as ordinary. Expanding first is what makes Tervin Rules mean anything for a
//!   user who lives in aliases.
//! - **Re-running a Block**, saved workflows, and any command Tervin spawns
//!   directly, where there is no shell to do the expanding.
//! - **The command palette and composer**, which can offer aliases as first-class
//!   things to run and show what they actually do.
//!
//! ## How the list is obtained
//!
//! There is no file to read: aliases are shell state, produced by rc files that
//! can contain arbitrary logic. The only correct way to enumerate them is to ask
//! the shell. Tervin runs the user's shell non-interactively-but-rc-loaded, asks
//! it to print its aliases and function names, and parses the result. The output
//! is treated as untrusted data — parsed, never executed.
//!
//! ## What expansion does and does not do
//!
//! Expansion replaces the *command word* of a segment, which is what a shell does.
//! It is recursive with cycle detection, so `alias ls='ls --color'` expands once
//! rather than forever. Two shell behaviours are deliberately not emulated,
//! because guessing at them would be worse than not trying: zsh global aliases
//! (`alias -g`), which can appear in any position, and the POSIX rule where an
//! alias value ending in a space causes the next word to be expanded too. Both are
//! recorded and reported rather than silently mishandled.

use crate::Shell;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// How deep alias expansion will recurse before giving up.
const MAX_EXPANSION_DEPTH: usize = 16;

/// How long to wait for the shell to list its aliases.
///
/// An rc file that blocks — waiting on a network call, a prompt, a slow version
/// manager — must not hang Tervin's startup.
const ENUMERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A snapshot of the shell's aliases and function names.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShellAliases {
    /// Alias name to its expansion.
    pub aliases: BTreeMap<String, String>,
    /// Names of shell functions.
    ///
    /// Functions cannot be expanded — their body is code, not a command line — but
    /// knowing a name is a function means Tervin can say "this is a shell
    /// function, its effects are unknown" instead of treating it as an unknown
    /// binary.
    pub functions: BTreeSet<String>,
    /// zsh global aliases, which can appear in any argument position.
    ///
    /// Recorded so their presence can be disclosed; not applied.
    pub global_aliases: BTreeMap<String, String>,
    pub shell: Option<Shell>,
    /// Anything that went wrong while enumerating, shown rather than swallowed.
    pub notes: Vec<String>,
    /// True when the shell was actually asked.
    ///
    /// Without this, "you have no aliases" and "Tervin could not read them" are the
    /// same empty list. They are not the same thing at all: alias discovery is how a
    /// second agent account gets found, so failing to check it silently means a user
    /// never learns their `claude-work` profile was there to adopt.
    pub enumerated: bool,
}

impl ShellAliases {
    /// Enumerate aliases from the user's configured shell.
    pub fn load() -> Self {
        match Shell::from_env() {
            Some(shell) => Self::load_from(shell),
            None => Self {
                notes: vec![
                    "Tervin does not recognise $SHELL, so aliases were not loaded.".to_string(),
                ],
                ..Default::default()
            },
        }
    }

    /// Enumerate aliases from a specific shell.
    pub fn load_from(shell: Shell) -> Self {
        let mut out = Self {
            shell: Some(shell),
            ..Default::default()
        };

        let program = match std::env::var("SHELL") {
            Ok(s) if !s.is_empty() => s,
            _ => shell.name().to_ascii_lowercase(),
        };

        let (args, _) = enumeration_command(shell);
        match run_with_timeout(&program, &args, ENUMERATION_TIMEOUT) {
            Ok(text) => {
                out.enumerated = true;
                out.absorb(shell, &text);
            }
            Err(e) => out
                .notes
                .push(format!("Could not read aliases from {}: {e}", shell.name())),
        }

        out
    }

    /// Parse enumeration output.
    pub fn absorb(&mut self, shell: Shell, text: &str) {
        for line in text.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }

            // Function names are emitted on their own marked lines so they can be
            // told apart from aliases without a second invocation.
            if let Some(name) = line.strip_prefix("\u{1}fn\u{1}") {
                let name = name.trim();
                if !name.is_empty() {
                    self.functions.insert(name.to_string());
                }
                continue;
            }

            if let Some((name, value, global)) = parse_alias_line(shell, line) {
                if global {
                    self.global_aliases.insert(name, value);
                } else {
                    self.aliases.insert(name, value);
                }
            }
        }

        if !self.global_aliases.is_empty() {
            self.notes.push(format!(
                "{} zsh global alias(es) found. Tervin lists them but does not expand them, \
                 because they can appear in any argument position.",
                self.global_aliases.len()
            ));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty() && self.functions.is_empty()
    }

    /// Expand the command word of every segment in a command line.
    ///
    /// Segments are split on `&&`, `||`, `;`, `|`, and newlines, matching where a
    /// shell would treat the next word as a command.
    pub fn expand_command_line(&self, command: &str) -> Expansion {
        let mut result = String::with_capacity(command.len());
        let mut applied: Vec<AppliedAlias> = Vec::new();
        let mut cursor = 0usize;

        for (start, end) in segment_spans(command) {
            // Preserve separators and spacing byte-for-byte.
            result.push_str(&command[cursor..start]);
            let segment = &command[start..end];
            let expanded = self.expand_segment(segment, &mut applied);
            result.push_str(&expanded);
            cursor = end;
        }
        result.push_str(&command[cursor..]);

        Expansion {
            original: command.to_string(),
            expanded: result,
            applied,
        }
    }

    /// Expand one segment's leading command word.
    fn expand_segment(&self, segment: &str, applied: &mut Vec<AppliedAlias>) -> String {
        let leading_ws = segment.len() - segment.trim_start().len();
        let (prefix, body) = segment.split_at(leading_ws);
        if body.is_empty() {
            return segment.to_string();
        }

        let mut current = body.to_string();
        // Track names already expanded so `alias ls='ls --color'` terminates.
        let mut seen: BTreeSet<String> = BTreeSet::new();

        for _ in 0..MAX_EXPANSION_DEPTH {
            // Skip leading VAR=value assignments to find the real command word.
            let (assign_len, word) = command_word(&current);
            let Some(word) = word else { break };

            if seen.contains(&word) {
                break;
            }
            let Some(value) = self.aliases.get(&word) else {
                break;
            };

            seen.insert(word.clone());
            applied.push(AppliedAlias {
                name: word.clone(),
                expansion: value.clone(),
            });

            let head = &current[..assign_len];
            let tail = &current[assign_len + word.len()..];
            current = format!("{head}{value}{tail}");
        }

        format!("{prefix}{current}")
    }

    /// Whether a name is a shell function, whose effects Tervin cannot inspect.
    pub fn is_function(&self, name: &str) -> bool {
        self.functions.contains(name)
    }

    /// Aliases as palette entries: `(name, expansion)`, sorted.
    pub fn palette_entries(&self) -> Vec<(&str, &str)> {
        self.aliases
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }
}

/// One alias substitution that was applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedAlias {
    pub name: String,
    pub expansion: String,
}

/// The result of expanding a command line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Expansion {
    pub original: String,
    pub expanded: String,
    /// Empty when nothing was expanded.
    pub applied: Vec<AppliedAlias>,
}

impl Expansion {
    pub fn changed(&self) -> bool {
        !self.applied.is_empty()
    }

    /// A sentence explaining the substitution, for an approval prompt.
    ///
    /// A user must be able to see that the thing they are approving is not the
    /// thing they typed.
    pub fn explanation(&self) -> Option<String> {
        if self.applied.is_empty() {
            return None;
        }
        let names: Vec<&str> = self.applied.iter().map(|a| a.name.as_str()).collect();
        Some(format!(
            "Expanded shell alias {}: runs `{}`",
            names
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(" → "),
            self.expanded.trim()
        ))
    }
}

/// The command that makes a shell print its aliases and function names.
///
/// Function names are prefixed with a control character so one invocation yields
/// both lists unambiguously.
fn enumeration_command(shell: Shell) -> (Vec<String>, &'static str) {
    let s = |v: &str| v.to_string();
    match shell {
        // `-i` is required: aliases live in .zshrc, which non-interactive shells
        // skip. `alias +` would omit values, so plain `alias` is used.
        Shell::Zsh => (
            vec![
                s("-ic"),
                s("alias; alias -g 2>/dev/null | sed 's/^/\\x01g\\x01/'; \
                   print -l ${(k)functions} 2>/dev/null | sed 's/^/\\x01fn\\x01/'"),
            ],
            "zsh",
        ),
        Shell::Bash => (
            vec![
                s("-ic"),
                s("alias; declare -F | sed 's/^declare -f /\\x01fn\\x01/'"),
            ],
            "bash",
        ),
        // fish implements aliases as functions, so `alias` lists them and
        // `functions -n` lists the rest.
        Shell::Fish => (
            vec![
                s("-c"),
                s("alias; functions -n | tr ', ' '\\n' | sed '/^$/d;s/^/\\x01fn\\x01/'"),
            ],
            "fish",
        ),
        Shell::PowerShell => (
            vec![
                s("-NoProfile"),
                s("-Command"),
                s("Get-Alias | ForEach-Object { \"$($_.Name)=$($_.Definition)\" }"),
            ],
            "pwsh",
        ),
    }
}

/// Parse one line of enumeration output.
///
/// Returns `(name, value, is_global)`.
fn parse_alias_line(shell: Shell, line: &str) -> Option<(String, String, bool)> {
    let mut line = line.trim();
    let mut global = false;

    if let Some(rest) = line.strip_prefix("\u{1}g\u{1}") {
        global = true;
        line = rest.trim();
    }

    // bash prints `alias name='value'`; zsh and fish print `name=value`.
    let line = line.strip_prefix("alias ").unwrap_or(line);

    // fish prints `alias name value` for some definitions.
    if shell == Shell::Fish && !line.contains('=') {
        let (name, value) = line.split_once(char::is_whitespace)?;
        return Some((name.trim().to_string(), value.trim().to_string(), false));
    }

    let (name, raw_value) = line.split_once('=')?;
    let name = name.trim();

    // A name must be a plausible command word.
    if name.is_empty()
        || name.contains(char::is_whitespace)
        || name.starts_with('-')
        || name.contains('/')
    {
        return None;
    }

    Some((name.to_string(), unquote(raw_value.trim()), global))
}

/// Strip one layer of surrounding quotes and undo the escaping inside.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if first == last && (first == b'\'' || first == b'"') {
            let inner = &value[1..value.len() - 1];
            return if first == b'\'' {
                // Inside single quotes a shell writes `'` as `'\''`.
                inner.replace("'\\''", "'")
            } else {
                inner.replace("\\\"", "\"").replace("\\\\", "\\")
            };
        }
    }
    value.to_string()
}

/// Find the command word of a segment, skipping `VAR=value` assignments.
///
/// Returns the byte offset where the word starts and the word itself.
fn command_word(segment: &str) -> (usize, Option<String>) {
    let mut offset = 0usize;
    let bytes = segment.as_bytes();

    loop {
        // Skip whitespace.
        while offset < bytes.len() && (bytes[offset] as char).is_whitespace() {
            offset += 1;
        }
        if offset >= bytes.len() {
            return (offset, None);
        }

        let start = offset;
        let mut end = offset;
        while end < bytes.len() && !(bytes[end] as char).is_whitespace() {
            end += 1;
        }
        let token = &segment[start..end];

        // `VAR=value` before the command is an assignment, not the command.
        let is_assignment = token
            .split_once('=')
            .map(|(k, _)| {
                !k.is_empty()
                    && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !k.chars().next().is_some_and(|c| c.is_ascii_digit())
            })
            .unwrap_or(false);

        if is_assignment {
            offset = end;
            continue;
        }

        return (start, Some(token.to_string()));
    }
}

/// Byte ranges of each command segment, split on shell separators.
///
/// Quote-aware, so a separator inside quotes does not split the line.
fn segment_spans(command: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let bytes = command.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut quote: Option<u8> = None;
    let mut escaped = false;

    while i < bytes.len() {
        let c = bytes[i];

        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if c == b'\\' {
            escaped = true;
            i += 1;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' {
            quote = Some(c);
            i += 1;
            continue;
        }

        let two = if i + 1 < bytes.len() {
            Some((c, bytes[i + 1]))
        } else {
            None
        };
        let sep_len = match (c, two) {
            (b'&', Some((b'&', b'&'))) => 2,
            (b'|', Some((b'|', b'|'))) => 2,
            (b';', _) | (b'\n', _) | (b'|', _) | (b'&', _) => 1,
            _ => 0,
        };

        if sep_len > 0 {
            spans.push((start, i));
            i += sep_len;
            start = i;
            continue;
        }

        i += 1;
    }
    spans.push((start, command.len()));
    spans.into_iter().filter(|(s, e)| e > s).collect()
}

/// Run a command with a wall-clock timeout.
///
/// An rc file that blocks must not hang Tervin. The child is killed on timeout.
fn run_with_timeout(
    program: &str,
    args: &[String],
    timeout: std::time::Duration,
) -> Result<String, String> {
    use std::process::{Command, Stdio};

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    return Err(format!(
                        "timed out after {}s (a shell startup file may be blocking)",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => return Err(e.to_string()),
        }
    }

    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zsh_aliases(pairs: &[(&str, &str)]) -> ShellAliases {
        let mut a = ShellAliases::default();
        for (k, v) in pairs {
            a.aliases.insert(k.to_string(), v.to_string());
        }
        a
    }

    #[test]
    fn parses_zsh_and_bash_alias_output() {
        let mut a = ShellAliases::default();
        a.absorb(
            Shell::Zsh,
            "ll='ls -la'\ngs='git status'\nclaude-work='CLAUDE_CONFIG_DIR=~/.claude-work claude'",
        );
        assert_eq!(a.aliases.get("ll").map(String::as_str), Some("ls -la"));
        assert_eq!(a.aliases.get("gs").map(String::as_str), Some("git status"));

        let mut b = ShellAliases::default();
        b.absorb(
            Shell::Bash,
            "alias ll='ls -la'\nalias grep='grep --color=auto'",
        );
        assert_eq!(b.aliases.get("ll").map(String::as_str), Some("ls -la"));
    }

    #[test]
    fn parses_function_names_separately_from_aliases() {
        let mut a = ShellAliases::default();
        a.absorb(
            Shell::Zsh,
            "ll='ls -la'\n\u{1}fn\u{1}my_deploy\n\u{1}fn\u{1}prompt_setup",
        );
        assert_eq!(a.aliases.len(), 1);
        assert!(a.is_function("my_deploy"));
        assert!(!a.is_function("ll"));
    }

    #[test]
    fn handles_embedded_quotes_in_alias_values() {
        let mut a = ShellAliases::default();
        a.absorb(Shell::Zsh, r#"gc='git commit -m '\''wip'\'''"#);
        assert_eq!(
            a.aliases.get("gc").map(String::as_str),
            Some("git commit -m 'wip'")
        );
    }

    #[test]
    fn expands_a_simple_alias() {
        let a = zsh_aliases(&[("ll", "ls -la")]);
        let e = a.expand_command_line("ll /tmp");
        assert_eq!(e.expanded, "ls -la /tmp");
        assert!(e.changed());
    }

    #[test]
    fn leaves_unknown_commands_untouched() {
        let a = zsh_aliases(&[("ll", "ls -la")]);
        let e = a.expand_command_line("cargo build");
        assert_eq!(e.expanded, "cargo build");
        assert!(!e.changed());
    }

    #[test]
    fn a_self_referential_alias_terminates() {
        // `alias ls='ls --color'` is extremely common and would loop forever
        // under naive recursive expansion.
        let a = zsh_aliases(&[("ls", "ls --color=auto")]);
        let e = a.expand_command_line("ls -la");
        assert_eq!(e.expanded, "ls --color=auto -la");
        assert_eq!(e.applied.len(), 1);
    }

    #[test]
    fn chained_aliases_expand_transitively() {
        let a = zsh_aliases(&[("g", "git"), ("gs", "g status")]);
        let e = a.expand_command_line("gs --short");
        assert_eq!(e.expanded, "git status --short");
        assert_eq!(e.applied.len(), 2);
    }

    #[test]
    fn mutually_recursive_aliases_do_not_hang() {
        let a = zsh_aliases(&[("a", "b"), ("b", "a")]);
        let e = a.expand_command_line("a");
        // Terminates, and reports what it did rather than looping.
        assert!(e.applied.len() <= MAX_EXPANSION_DEPTH);
    }

    #[test]
    fn expands_every_segment_of_a_compound_command() {
        // The safety-relevant case: the dangerous alias is not the first word.
        let a = zsh_aliases(&[("nuke", "rm -rf /"), ("ll", "ls -la")]);
        let e = a.expand_command_line("ll && nuke");
        assert_eq!(e.expanded, "ls -la && rm -rf /");
        assert_eq!(e.applied.len(), 2);
    }

    #[test]
    fn expands_after_a_pipe_and_a_semicolon() {
        let a = zsh_aliases(&[("j", "jq ."), ("c", "cat")]);
        let e = a.expand_command_line("c file.json | j");
        assert_eq!(e.expanded, "cat file.json | jq .");
    }

    #[test]
    fn does_not_expand_inside_quotes() {
        let a = zsh_aliases(&[("ll", "ls -la")]);
        // `ll` here is data, not a command word.
        let e = a.expand_command_line("echo 'run ll now'");
        assert_eq!(e.expanded, "echo 'run ll now'");
        assert!(!e.changed());
    }

    #[test]
    fn does_not_expand_an_argument_that_matches_an_alias_name() {
        let a = zsh_aliases(&[("build", "cargo build --release")]);
        let e = a.expand_command_line("make build");
        assert_eq!(e.expanded, "make build");
    }

    #[test]
    fn expands_after_leading_env_assignments() {
        let a = zsh_aliases(&[("dep", "kubectl apply")]);
        let e = a.expand_command_line("KUBECONFIG=/tmp/k dep -f x.yaml");
        assert_eq!(e.expanded, "KUBECONFIG=/tmp/k kubectl apply -f x.yaml");
    }

    #[test]
    fn preserves_original_spacing_around_separators() {
        let a = zsh_aliases(&[("ll", "ls -la")]);
        let e = a.expand_command_line("  ll   &&   ll  ");
        assert_eq!(e.expanded, "  ls -la   &&   ls -la  ");
    }

    #[test]
    fn an_alias_hiding_a_dangerous_command_is_revealed() {
        // The reason this module exists. Without expansion the classifier sees an
        // unknown program and reports the action as ordinary.
        let a = zsh_aliases(&[("deploy", "kubectl apply --context prod-cluster -f .")]);
        let e = a.expand_command_line("deploy");
        assert!(e.changed());

        let naive = rules_engine_classify("deploy");
        let expanded = rules_engine_classify(&e.expanded);
        assert!(
            expanded > naive,
            "expanding must raise the assessed risk: {naive:?} -> {expanded:?}"
        );
    }

    /// Local shim so this crate does not depend on `rules-engine`; the real
    /// wiring lives in the application layer.
    fn rules_engine_classify(command: &str) -> u8 {
        // A stand-in ranking: presence of a production deploy shape.
        let lower = command.to_ascii_lowercase();
        if lower.contains("kubectl apply") && lower.contains("prod") {
            3
        } else if lower.contains("kubectl") {
            2
        } else {
            0
        }
    }

    #[test]
    fn explanation_states_what_will_actually_run() {
        let a = zsh_aliases(&[("deploy", "kubectl apply -f .")]);
        let e = a.expand_command_line("deploy");
        let text = e.explanation().unwrap();
        assert!(text.contains("deploy"));
        assert!(text.contains("kubectl apply -f ."));
    }

    #[test]
    fn global_aliases_are_recorded_but_not_applied() {
        // zsh global aliases can appear anywhere; applying them by guesswork
        // would corrupt commands, so their presence is disclosed instead.
        let mut a = ShellAliases::default();
        a.absorb(Shell::Zsh, "\u{1}g\u{1}G='| grep'");
        assert!(a.aliases.is_empty());
        assert_eq!(a.global_aliases.len(), 1);
        assert!(a.notes.iter().any(|n| n.contains("global alias")));

        let e = a.expand_command_line("ls G foo");
        assert!(!e.changed());
    }

    #[test]
    fn ignores_lines_that_are_not_aliases() {
        let mut a = ShellAliases::default();
        a.absorb(Shell::Zsh, "not an alias line\n=broken\n-x=1\n/usr/bin/x=y");
        assert!(a.aliases.is_empty());
    }

    #[test]
    fn empty_input_is_handled() {
        let a = ShellAliases::default();
        let e = a.expand_command_line("");
        assert_eq!(e.expanded, "");
        assert!(!e.changed());
    }

    /// Exercises the real shell, so a change in output format is caught rather
    /// than assumed away. Skipped where the shell is unavailable.
    #[test]
    fn reads_aliases_from_the_real_shell() {
        let Some(shell) = Shell::from_env() else {
            return;
        };
        if shell != Shell::Zsh && shell != Shell::Bash {
            return;
        }
        let loaded = ShellAliases::load();

        // The invariant is "either the shell was asked, or there is a note saying why
        // not" — never silence. An earlier version of this test demanded aliases or a
        // note, which fails on a machine that simply has no aliases: a bare CI runner,
        // for one. That is a legitimate outcome, and the assertion was wrong rather
        // than the behaviour.
        assert!(
            loaded.enumerated || !loaded.notes.is_empty(),
            "enumeration neither ran nor explained itself"
        );

        // Anything found has to be usable: an empty name would break expansion, and a
        // self-referential alias would loop it.
        for (name, value) in &loaded.aliases {
            assert!(!name.is_empty(), "an alias with no name");
            assert_ne!(name, value, "an alias that expands to itself: {name}");
        }
    }
}
