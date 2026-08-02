//! Reading `~/.ssh/config`.
//!
//! Tervin does not invent its own host list. People already have one, it is
//! already correct, and it is already what `ssh` itself obeys — so the SSH
//! manager reads that file rather than asking anyone to retype it.
//!
//! The parser implements the parts of the format real configs actually use:
//!
//! - `Host` blocks with glob patterns and `!` negation.
//! - `Include` with globs, resolved relative to the containing file's directory
//!   (or `~/.ssh` for a bare relative path), with recursion depth capped.
//! - **First-obtained-value-wins.** This is the rule most hand-rolled parsers get
//!   backwards. In `ssh_config` the *earliest* matching declaration wins, which is
//!   why a `Host *` block at the bottom acts as defaults rather than as an
//!   override. Getting this wrong makes Tervin connect somewhere the user's own
//!   `ssh` would not.
//! - Case-insensitive keywords, `=` or whitespace separators, quoted values.
//!
//! `Match` blocks are recognised and skipped rather than half-evaluated: they can
//! depend on `exec` output, the originating user, and the final hostname, and a
//! guess about them would be a guess about where a connection goes.
//!
//! Nothing here ever reads a private key or a passphrase. Tervin records only
//! *which* identity file a host names, so it can say the host uses one.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Cap on `Include` recursion, so a config that includes itself terminates.
const MAX_INCLUDE_DEPTH: usize = 8;

/// Cap on files pulled in by a single glob, so a stray `Include *` cannot make
/// startup read a whole directory tree.
const MAX_INCLUDED_FILES: usize = 256;

/// One host entry, resolved from every block that matches its alias.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshHost {
    /// The name as written in the config, and what a user types.
    pub alias: String,
    /// `HostName`, when the config renames the target.
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// Path only. The key itself is never read.
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub proxy_command: Option<String>,
    pub forward_agent: Option<bool>,
    /// `RequestTTY`, which decides whether a remote shell is interactive.
    pub request_tty: Option<String>,
    /// Where this host was declared, for "why is this here".
    pub source_file: Option<String>,
    /// True when the alias contains a glob and so is a defaults block rather than
    /// a connectable host.
    pub is_pattern: bool,
}

impl SshHost {
    /// What `ssh <alias>` would actually connect to, for display.
    pub fn target(&self) -> String {
        let host = self.hostname.clone().unwrap_or_else(|| self.alias.clone());
        match (&self.user, self.port) {
            (Some(u), Some(p)) if p != 22 => format!("{u}@{host}:{p}"),
            (Some(u), _) => format!("{u}@{host}"),
            (None, Some(p)) if p != 22 => format!("{host}:{p}"),
            _ => host,
        }
    }

    /// The argument list Tervin would launch.
    ///
    /// Built from the alias alone: `ssh` reads the same config and applies every
    /// option itself. Re-passing `-p` and `-l` here would duplicate the config's
    /// own logic and diverge from it the moment a `Match` block applies.
    pub fn ssh_args(&self) -> Vec<String> {
        vec![self.alias.clone()]
    }
}

/// The parsed config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SshConfig {
    /// Connectable hosts, in declaration order.
    pub hosts: Vec<SshHost>,
    /// Wildcard blocks such as `Host *`, kept for display but not connectable.
    pub patterns: Vec<SshHost>,
    /// Files that could not be read, reported rather than silently skipped.
    pub warnings: Vec<String>,
}

impl SshConfig {
    /// Read the user's config, returning an empty set when there is none.
    pub fn load() -> Self {
        match dirs::home_dir() {
            Some(home) => Self::load_from(&home.join(".ssh").join("config")),
            None => Self::default(),
        }
    }

    /// Read a specific config file.
    pub fn load_from(path: &Path) -> Self {
        let mut blocks: Vec<Block> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        parse_file(path, 0, &mut blocks, &mut warnings);
        Self::from_blocks(blocks, warnings)
    }

    /// Parse config text directly, used by tests.
    pub fn parse_str(text: &str) -> Self {
        let mut blocks = Vec::new();
        let mut warnings = Vec::new();
        parse_text(text, None, 0, &mut blocks, &mut warnings);
        Self::from_blocks(blocks, warnings)
    }

    fn from_blocks(blocks: Vec<Block>, warnings: Vec<String>) -> Self {
        // Every literal alias mentioned anywhere is a candidate host.
        let mut aliases: Vec<String> = Vec::new();
        for block in &blocks {
            for pattern in &block.patterns {
                if !pattern.negated && !is_glob(&pattern.text) && !aliases.contains(&pattern.text) {
                    aliases.push(pattern.text.clone());
                }
            }
        }

        let hosts = aliases
            .iter()
            .map(|alias| resolve(alias, &blocks))
            .collect();

        // Wildcard blocks are shown so a user can see where defaults come from.
        let mut patterns = Vec::new();
        for block in &blocks {
            for pattern in &block.patterns {
                if pattern.negated || !is_glob(&pattern.text) {
                    continue;
                }
                let mut host = host_from_keywords(&pattern.text, &block.keywords);
                host.is_pattern = true;
                host.source_file = block.source.clone();
                patterns.push(host);
            }
        }

        Self {
            hosts,
            patterns,
            warnings,
        }
    }

    pub fn get(&self, alias: &str) -> Option<&SshHost> {
        self.hosts.iter().find(|h| h.alias == alias)
    }
}

/// One `Host` block: the patterns it names and the keywords it sets.
#[derive(Debug, Clone)]
struct Block {
    patterns: Vec<Pattern>,
    /// Lower-cased keyword to value, in declaration order.
    keywords: Vec<(String, String)>,
    source: Option<String>,
}

#[derive(Debug, Clone)]
struct Pattern {
    text: String,
    negated: bool,
}

/// Resolve one alias against every block, first-value-wins.
fn resolve(alias: &str, blocks: &[Block]) -> SshHost {
    let mut merged: Vec<(String, String)> = Vec::new();
    let mut source: Option<String> = None;

    for block in blocks {
        if !block_matches(alias, &block.patterns) {
            continue;
        }
        if source.is_none() {
            // Attribute the host to the first block that named it literally.
            if block.patterns.iter().any(|p| !p.negated && p.text == alias) {
                source = block.source.clone();
            }
        }
        for (key, value) in &block.keywords {
            // First wins: a later block never overrides an earlier one.
            if !merged.iter().any(|(k, _)| k == key) {
                merged.push((key.clone(), value.clone()));
            }
        }
    }

    let mut host = host_from_keywords(alias, &merged);
    host.source_file = source;
    host
}

/// Whether an alias matches a block's pattern list.
///
/// A negated pattern excludes the alias outright, which is why negations are
/// checked before positives rather than folded into the same pass.
fn block_matches(alias: &str, patterns: &[Pattern]) -> bool {
    if patterns
        .iter()
        .any(|p| p.negated && glob_match(&p.text, alias))
    {
        return false;
    }
    patterns
        .iter()
        .any(|p| !p.negated && glob_match(&p.text, alias))
}

fn host_from_keywords(alias: &str, keywords: &[(String, String)]) -> SshHost {
    let get = |name: &str| {
        keywords
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };

    SshHost {
        alias: alias.to_string(),
        hostname: get("hostname"),
        user: get("user"),
        port: get("port").and_then(|p| p.parse().ok()),
        identity_file: get("identityfile"),
        proxy_jump: get("proxyjump"),
        proxy_command: get("proxycommand"),
        forward_agent: get("forwardagent")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "on")),
        request_tty: get("requesttty"),
        source_file: None,
        is_pattern: false,
    }
}

fn parse_file(path: &Path, depth: usize, blocks: &mut Vec<Block>, warnings: &mut Vec<String>) {
    if depth > MAX_INCLUDE_DEPTH {
        warnings.push(format!(
            "Stopped at {}: includes nested more than {MAX_INCLUDE_DEPTH} deep.",
            path.display()
        ));
        return;
    }
    match std::fs::read_to_string(path) {
        Ok(text) => parse_text(&text, Some(path), depth, blocks, warnings),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warnings.push(format!("Could not read {}: {e}", path.display())),
    }
}

fn parse_text(
    text: &str,
    source: Option<&Path>,
    depth: usize,
    blocks: &mut Vec<Block>,
    warnings: &mut Vec<String>,
) {
    let source_label = source.map(|p| p.display().to_string());
    let mut current: Option<Block> = None;
    // Set inside a `Match` block, so its keywords are not attributed to the
    // preceding `Host`.
    let mut in_match = false;

    for raw in text.lines() {
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }

        let Some((keyword, value)) = split_keyword(line) else {
            continue;
        };
        let lower = keyword.to_ascii_lowercase();

        match lower.as_str() {
            "host" => {
                in_match = false;
                if let Some(block) = current.take() {
                    blocks.push(block);
                }
                current = Some(Block {
                    patterns: parse_patterns(&value),
                    keywords: Vec::new(),
                    source: source_label.clone(),
                });
            }

            // Recognised and skipped: evaluating it would require running `exec`
            // and resolving the final hostname, so a guess here is a guess about
            // where a connection goes.
            "match" => {
                in_match = true;
                if let Some(block) = current.take() {
                    blocks.push(block);
                }
            }

            "include" => {
                if in_match {
                    continue;
                }
                // An include inside a Host block still applies globally, matching
                // ssh's own behaviour of splicing the file in at this point.
                if let Some(block) = current.take() {
                    blocks.push(block);
                }
                for path in expand_include(&value, source, warnings) {
                    parse_file(&path, depth + 1, blocks, warnings);
                }
            }

            _ => {
                if in_match {
                    continue;
                }
                if let Some(block) = current.as_mut() {
                    block.keywords.push((lower, value));
                }
                // Keywords before any Host block are global defaults in ssh.
                // Represented as an implicit `Host *` so the same resolution
                // path handles them.
                else {
                    current = Some(Block {
                        patterns: vec![Pattern {
                            text: "*".to_string(),
                            negated: false,
                        }],
                        keywords: vec![(lower, value)],
                        source: source_label.clone(),
                    });
                }
            }
        }
    }

    if let Some(block) = current.take() {
        blocks.push(block);
    }
}

/// Resolve an `Include` value into concrete paths.
fn expand_include(value: &str, source: Option<&Path>, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut out = Vec::new();

    for token in split_values(value) {
        let expanded = expand_tilde(&token);
        let path = Path::new(&expanded);

        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            // Relative includes resolve against the containing file's directory,
            // falling back to ~/.ssh as ssh does for the user config.
            let base = source
                .and_then(|s| s.parent().map(|p| p.to_path_buf()))
                .or_else(|| dirs::home_dir().map(|h| h.join(".ssh")))
                .unwrap_or_else(|| PathBuf::from("."));
            base.join(path)
        };

        if absolute.to_string_lossy().contains(['*', '?', '[']) {
            out.extend(glob_paths(&absolute, warnings));
        } else {
            out.push(absolute);
        }
    }

    out
}

/// Expand a glob over one directory level, which is what real configs use
/// (`Include config.d/*`).
fn glob_paths(pattern: &Path, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let Some(parent) = pattern.parent() else {
        return Vec::new();
    };
    let Some(name) = pattern.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };

    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };

    let mut out: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|f| glob_match(name, f))
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();

    // Deterministic order, so two runs produce the same first-wins result.
    out.sort();

    if out.len() > MAX_INCLUDED_FILES {
        warnings.push(format!(
            "{} matched {} files; only the first {MAX_INCLUDED_FILES} were read.",
            pattern.display(),
            out.len()
        ));
        out.truncate(MAX_INCLUDED_FILES);
    }
    out
}

fn strip_comment(line: &str) -> &str {
    let line = line.trim();
    match line.find('#') {
        Some(0) => "",
        Some(i) => line[..i].trim_end(),
        None => line,
    }
}

/// Split `Keyword value` or `Keyword=value`.
fn split_keyword(line: &str) -> Option<(String, String)> {
    let bytes = line.as_bytes();
    let end = bytes
        .iter()
        .position(|&c| c == b' ' || c == b'\t' || c == b'=')?;
    let keyword = line[..end].to_string();
    let rest = line[end..].trim_start_matches([' ', '\t', '=']).trim();
    if keyword.is_empty() {
        return None;
    }
    Some((keyword, unquote(rest)))
}

fn parse_patterns(value: &str) -> Vec<Pattern> {
    split_values(value)
        .into_iter()
        .map(|token| match token.strip_prefix('!') {
            Some(rest) => Pattern {
                text: rest.to_string(),
                negated: true,
            },
            None => Pattern {
                text: token,
                negated: false,
            },
        })
        .collect()
}

/// Split a value into whitespace-separated tokens, honouring double quotes.
fn split_values(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;

    for ch in value.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn expand_tilde(value: &str) -> String {
    match value.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|h| h.join(rest).display().to_string())
            .unwrap_or_else(|| value.to_string()),
        None => value.to_string(),
    }
}

fn is_glob(text: &str) -> bool {
    text.contains(['*', '?'])
}

/// Match an ssh_config pattern: `*` any run, `?` one character.
fn glob_match(pattern: &str, text: &str) -> bool {
    // Iterative backtracking rather than recursion, so a pathological pattern
    // cannot blow the stack.
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();

    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if star != usize::MAX {
            // Backtrack: let the last `*` absorb one more character.
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }

    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Keys used by `BTreeMap`-based callers that want the raw keyword set.
pub type KeywordMap = BTreeMap<String, String>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_basic_host_block() {
        let config = SshConfig::parse_str(
            "Host staging\n  HostName staging.example.com\n  User deploy\n  Port 2222\n",
        );
        let host = config.get("staging").expect("staging not found");
        assert_eq!(host.hostname.as_deref(), Some("staging.example.com"));
        assert_eq!(host.user.as_deref(), Some("deploy"));
        assert_eq!(host.port, Some(2222));
        assert_eq!(host.target(), "deploy@staging.example.com:2222");
    }

    #[test]
    fn the_first_matching_value_wins() {
        // The rule most hand-rolled parsers get backwards. A trailing `Host *`
        // supplies defaults; it must not override what came before it.
        let config = SshConfig::parse_str(
            "Host web\n  User first\n\nHost web\n  User second\n\nHost *\n  User fallback\n",
        );
        assert_eq!(config.get("web").unwrap().user.as_deref(), Some("first"));
    }

    #[test]
    fn wildcard_defaults_apply_to_unset_keys_only() {
        let config = SshConfig::parse_str(
            "Host prod\n  HostName prod.example.com\n\nHost *\n  User admin\n  Port 2200\n",
        );
        let host = config.get("prod").unwrap();
        assert_eq!(host.hostname.as_deref(), Some("prod.example.com"));
        assert_eq!(host.user.as_deref(), Some("admin"));
        assert_eq!(host.port, Some(2200));
    }

    #[test]
    fn negated_patterns_exclude_a_host() {
        let config = SshConfig::parse_str(
            "Host * !secure\n  ForwardAgent yes\n\nHost secure\n  HostName secure.example.com\n\nHost normal\n  HostName normal.example.com\n",
        );
        assert_eq!(config.get("normal").unwrap().forward_agent, Some(true));
        assert_eq!(
            config.get("secure").unwrap().forward_agent,
            None,
            "a negated pattern must exclude the host entirely"
        );
    }

    #[test]
    fn glob_patterns_match_like_ssh() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*.example.com", "a.example.com"));
        assert!(glob_match("web-?", "web-1"));
        assert!(!glob_match("web-?", "web-12"));
        assert!(glob_match("a*b*c", "axxbyyc"));
        assert!(!glob_match("a*b*c", "axxbyy"));
        // Backtracking must terminate rather than hang.
        assert!(!glob_match("*a*a*a*a*b", &"a".repeat(40)));
    }

    #[test]
    fn accepts_equals_separated_and_quoted_values() {
        let config = SshConfig::parse_str(
            "Host odd\n  HostName=box.example.com\n  ProxyCommand=\"nc -X connect %h %p\"\n",
        );
        let host = config.get("odd").unwrap();
        assert_eq!(host.hostname.as_deref(), Some("box.example.com"));
        assert_eq!(host.proxy_command.as_deref(), Some("nc -X connect %h %p"));
    }

    #[test]
    fn keywords_are_case_insensitive() {
        let config = SshConfig::parse_str("HOST box\n  hostname box.local\n  USER root\n");
        let host = config.get("box").unwrap();
        assert_eq!(host.hostname.as_deref(), Some("box.local"));
        assert_eq!(host.user.as_deref(), Some("root"));
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let config = SshConfig::parse_str(
            "# a comment\n\nHost box   # trailing\n  HostName box.local\n  # indented comment\n",
        );
        assert_eq!(
            config.get("box").unwrap().hostname.as_deref(),
            Some("box.local")
        );
    }

    #[test]
    fn one_block_can_declare_several_hosts() {
        let config = SshConfig::parse_str("Host a b c\n  User shared\n");
        for alias in ["a", "b", "c"] {
            assert_eq!(
                config.get(alias).unwrap().user.as_deref(),
                Some("shared"),
                "{alias} did not inherit the shared block"
            );
        }
    }

    #[test]
    fn wildcard_blocks_are_not_offered_as_connectable_hosts() {
        let config = SshConfig::parse_str("Host *.internal\n  User svc\n\nHost real\n  User me\n");
        assert!(config.get("*.internal").is_none());
        assert_eq!(config.hosts.len(), 1);
        assert_eq!(config.hosts[0].alias, "real");
        // Still visible, so a user can see where a default came from.
        assert!(config.patterns.iter().any(|p| p.alias == "*.internal"));
    }

    #[test]
    fn match_blocks_are_skipped_not_misattributed() {
        // A Match block's keywords must not leak onto the preceding Host.
        let config = SshConfig::parse_str(
            "Host box\n  User me\n\nMatch host box exec \"true\"\n  User someone-else\n  Port 9999\n",
        );
        let host = config.get("box").unwrap();
        assert_eq!(host.user.as_deref(), Some("me"));
        assert_eq!(host.port, None, "Match keywords must not be applied");
    }

    #[test]
    fn keywords_before_any_host_block_are_global_defaults() {
        let config = SshConfig::parse_str("ForwardAgent yes\n\nHost box\n  HostName box.local\n");
        assert_eq!(config.get("box").unwrap().forward_agent, Some(true));
    }

    #[test]
    fn resolves_includes_relative_to_the_containing_file() {
        let dir = tempfile::tempdir().unwrap();
        let inner_dir = dir.path().join("config.d");
        std::fs::create_dir_all(&inner_dir).unwrap();
        std::fs::write(
            inner_dir.join("10-work"),
            "Host work\n  HostName work.example.com\n  User q\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("config"),
            "Include config.d/*\n\nHost home\n  HostName home.local\n",
        )
        .unwrap();

        let config = SshConfig::load_from(&dir.path().join("config"));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
        assert_eq!(
            config.get("work").unwrap().hostname.as_deref(),
            Some("work.example.com")
        );
        assert_eq!(
            config.get("home").unwrap().hostname.as_deref(),
            Some("home.local")
        );
    }

    #[test]
    fn included_files_keep_first_wins_ordering() {
        // An include at the top must win over a later local block, because that
        // is the order ssh reads them in.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("extra"), "Host box\n  User from-include\n").unwrap();
        std::fs::write(
            dir.path().join("config"),
            "Include extra\n\nHost box\n  User from-main\n",
        )
        .unwrap();

        let config = SshConfig::load_from(&dir.path().join("config"));
        assert_eq!(
            config.get("box").unwrap().user.as_deref(),
            Some("from-include")
        );
    }

    #[test]
    fn a_self_including_config_terminates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        std::fs::write(&path, "Include config\n\nHost box\n  User me\n").unwrap();

        let config = SshConfig::load_from(&path);
        assert_eq!(config.get("box").unwrap().user.as_deref(), Some("me"));
        assert!(
            config.warnings.iter().any(|w| w.contains("nested")),
            "recursion should be reported, got {:?}",
            config.warnings
        );
    }

    #[test]
    fn a_missing_config_is_not_an_error() {
        let config = SshConfig::load_from(Path::new("/nonexistent/ssh/config"));
        assert!(config.hosts.is_empty());
        assert!(
            config.warnings.is_empty(),
            "absence is normal, not a warning"
        );
    }

    #[test]
    fn launch_args_delegate_option_handling_to_ssh() {
        // Re-passing -p/-l would duplicate the config's own logic and diverge
        // the moment a Match block applies.
        let config = SshConfig::parse_str("Host box\n  User me\n  Port 2222\n");
        assert_eq!(config.get("box").unwrap().ssh_args(), vec!["box"]);
    }

    #[test]
    fn identity_files_are_named_never_read() {
        let config = SshConfig::parse_str("Host box\n  IdentityFile ~/.ssh/id_ed25519\n");
        let host = config.get("box").unwrap();
        assert_eq!(host.identity_file.as_deref(), Some("~/.ssh/id_ed25519"));
    }

    /// Parse the machine's real config, if there is one.
    ///
    /// Fixtures only prove the parser handles what was imagined. This proves it
    /// handles what actually exists. Skipped where there is no config.
    #[test]
    fn parses_the_real_config_on_this_machine() {
        let Some(home) = dirs::home_dir() else { return };
        let path = home.join(".ssh").join("config");
        if !path.exists() {
            return;
        }

        let config = SshConfig::load_from(&path);
        assert!(
            config.warnings.is_empty(),
            "real config produced warnings: {:?}",
            config.warnings
        );
        // Every parsed host must be connectable: a literal alias and a target.
        for host in &config.hosts {
            assert!(!host.alias.is_empty());
            assert!(!host.is_pattern, "{} was offered as a host", host.alias);
            assert!(!host.target().is_empty());
            assert_eq!(host.ssh_args(), vec![host.alias.clone()]);
        }
    }

    #[test]
    fn parses_a_realistic_config_end_to_end() {
        let config = SshConfig::parse_str(
            r#"
# Personal
Host github.com
  User git
  IdentityFile ~/.ssh/id_ed25519

Host bastion
  HostName bastion.corp.example
  User quintin
  ForwardAgent yes

Host db-*
  ProxyJump bastion
  User dbadmin

Host db-primary
  HostName 10.0.1.20

Host *
  ServerAliveInterval 60
  Port 22
"#,
        );

        assert!(config.get("github.com").is_some());
        let db = config.get("db-primary").unwrap();
        assert_eq!(db.hostname.as_deref(), Some("10.0.1.20"));
        // Inherited from the `db-*` pattern.
        assert_eq!(db.proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(db.user.as_deref(), Some("dbadmin"));
        assert_eq!(db.port, Some(22));
        assert_eq!(db.target(), "dbadmin@10.0.1.20");
    }
}
