//! Saved commands with typed holes in them.
//!
//! Half the commands anyone runs are a shape with one thing changed: deploy to *this*
//! environment, tail *that* service, reset *this* branch. Shell history gives you the last
//! one you happened to type, which is the wrong instance of the shape more often than not
//! — so people keep a scratch file of commands and copy out of it.
//!
//! A saved command is that scratch file, with the varying parts named:
//!
//! ```text
//! kubectl logs -f {{service}} --namespace {{env:staging}}
//! ```
//!
//! ## Why the parser is strict about what a hole is
//!
//! A command line is full of braces. `${HOME}`, `awk '{print $1}'`, a JSON body, a brace
//! expansion like `mv x.{txt,md}` — all of those are ordinary text that must survive
//! untouched. Treating any `{...}` as a hole would quietly corrupt the command the user
//! saved, and they would only find out when it ran.
//!
//! So a hole is exactly `{{name}}` or `{{name:default}}`, with the name restricted to
//! word characters and dashes. Everything else is text. That is narrow enough to be
//! unambiguous and wide enough for the cases people actually want.

use serde::{Deserialize, Serialize};

/// A named command with holes to fill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedCommand {
    pub id: String,
    /// What it is called in the picker.
    pub name: String,
    /// The command, holes included.
    pub template: String,
    /// One line on what it does, shown next to the name.
    pub description: Option<String>,
    /// Times it has been used, for ranking.
    pub uses: u32,
}

/// One hole in a template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    /// Prefilled when present, so the common case is one keystroke.
    pub default: Option<String>,
}

/// Longest a template may be.
///
/// Generous for a command, small enough that a pasted file cannot become one.
pub const MAX_TEMPLATE: usize = 8 * 1024;

/// The holes in a template, in the order they appear, each named once.
///
/// A name repeated in the template is one parameter filled in both places — which is the
/// point of naming them rather than numbering them.
pub fn parameters(template: &str) -> Vec<Parameter> {
    let mut out: Vec<Parameter> = Vec::new();
    for (name, default) in scan(template) {
        match out.iter_mut().find(|p| p.name == name) {
            // A later default fills in for an earlier bare mention, so
            // `{{env}} … {{env:staging}}` still offers the default.
            Some(existing) => {
                if existing.default.is_none() {
                    existing.default = default;
                }
            }
            None => out.push(Parameter { name, default }),
        }
    }
    out
}

/// Fill a template. Holes with no value keep their default, or are left as they are.
///
/// A missing value is deliberately *not* substituted with an empty string: an empty
/// argument silently changes what a command does — `rm -rf {{path}}` with `path` unset
/// would become `rm -rf`, and a visible `{{path}}` is a command that fails loudly instead.
pub fn render(template: &str, values: &[(String, String)]) -> String {
    let lookup = |name: &str| -> Option<&str> {
        values
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    };

    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some((name, default, end)) = hole_at(template, i) {
            match lookup(&name).map(str::to_string).or(default) {
                Some(value) => out.push_str(&value),
                // Left visible on purpose. See above.
                None => out.push_str(&template[i..end]),
            }
            i = end;
            continue;
        }
        // Push one whole character, so a multi-byte character is never split.
        let ch = template[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Every hole, as `(name, default)`.
fn scan(template: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < template.len() {
        match hole_at(template, i) {
            Some((name, default, end)) => {
                out.push((name, default));
                i = end;
            }
            None => {
                let ch = template[i..].chars().next().unwrap_or('\u{FFFD}');
                i += ch.len_utf8();
            }
        }
    }
    out
}

/// A hole starting at `at`, returning its name, default, and the offset just past it.
///
/// Returns `None` for anything that is not exactly `{{name}}` or `{{name:default}}` —
/// which is what keeps `${HOME}`, `awk '{print $1}'` and `mv x.{txt,md}` intact.
fn hole_at(template: &str, at: usize) -> Option<(String, Option<String>, usize)> {
    let rest = template.get(at..)?;
    let inner = rest.strip_prefix("{{")?;
    let close = inner.find("}}")?;
    let body = &inner[..close];
    // A nested `{` means this is not a hole — most likely a shell expansion that happens
    // to sit next to a brace.
    if body.contains('{') || body.contains('}') {
        return None;
    }

    let (name, default) = match body.split_once(':') {
        Some((name, default)) => (name.trim(), Some(default.trim().to_string())),
        None => (body.trim(), None),
    };
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }

    Some((
        name.to_string(),
        // An empty default is no default: `{{env:}}` reads as "I have not decided yet".
        default.filter(|d| !d.is_empty()),
        at + 2 + close + 2,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(template: &str) -> Vec<String> {
        parameters(template).into_iter().map(|p| p.name).collect()
    }

    #[test]
    fn finds_holes_in_order() {
        assert_eq!(
            names("kubectl logs -f {{service}} --namespace {{env}}"),
            vec!["service".to_string(), "env".to_string()]
        );
    }

    #[test]
    fn reads_a_default() {
        let params = parameters("deploy {{env:staging}}");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "env");
        assert_eq!(params[0].default.as_deref(), Some("staging"));
    }

    #[test]
    fn a_repeated_name_is_one_parameter() {
        // The point of naming rather than numbering: fill it once, it appears everywhere.
        assert_eq!(
            names("git checkout {{branch}} && git pull origin {{branch}}"),
            vec!["branch".to_string()]
        );
        assert_eq!(
            render(
                "git checkout {{branch}} && git pull origin {{branch}}",
                &[("branch".to_string(), "main".to_string())]
            ),
            "git checkout main && git pull origin main"
        );
    }

    #[test]
    fn a_later_default_fills_in_for_an_earlier_bare_mention() {
        let params = parameters("echo {{env}} then deploy {{env:staging}}");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].default.as_deref(), Some("staging"));
    }

    /// The tests that matter most. A command line is full of braces, and treating any of
    /// them as a hole would quietly corrupt what the user saved.
    #[test]
    fn ordinary_shell_syntax_is_not_mistaken_for_a_hole() {
        for template in [
            "echo ${HOME}",
            "echo $HOME",
            "awk '{print $1}' file",
            "awk '{{print $1}}' file",
            "mv report.{txt,md} /tmp",
            r#"curl -d '{"key": "value"}' http://x"#,
            "find . -exec rm {} \\;",
            "echo {}",
            "echo {{}}",
            "echo {{ }}",
            // A name with a space or punctuation is not a name.
            "echo {{not a name}}",
            "echo {{no.dots}}",
            "echo {{$var}}",
        ] {
            assert!(
                parameters(template).is_empty(),
                "{template:?} was read as having a parameter"
            );
            // And rendering leaves it byte for byte.
            assert_eq!(render(template, &[]), template, "{template:?} was altered");
        }
    }

    #[test]
    fn a_hole_beside_shell_syntax_still_works() {
        // Both in one line, which is what a real deploy command looks like.
        let template = r#"curl -d '{"env":"{{env}}"}' ${API_URL}/deploy"#;
        assert_eq!(names(template), vec!["env".to_string()]);
        assert_eq!(
            render(template, &[("env".to_string(), "prod".to_string())]),
            r#"curl -d '{"env":"prod"}' ${API_URL}/deploy"#
        );
    }

    #[test]
    fn a_missing_value_is_left_visible_rather_than_emptied() {
        // The one that could cause real damage. `rm -rf {{path}}` with nothing filled in
        // must not become `rm -rf`, which deletes the current directory's contents on some
        // shells and errors on others — either way, not what was meant.
        assert_eq!(render("rm -rf {{path}}", &[]), "rm -rf {{path}}");
        // An explicitly empty value is treated the same, because a blank text field is
        // "not filled in" rather than "the empty string".
        assert_eq!(
            render("rm -rf {{path}}", &[("path".to_string(), String::new())]),
            "rm -rf {{path}}"
        );
    }

    #[test]
    fn a_default_is_used_when_nothing_is_filled_in() {
        assert_eq!(render("deploy {{env:staging}}", &[]), "deploy staging");
        assert_eq!(
            render(
                "deploy {{env:staging}}",
                &[("env".to_string(), "prod".to_string())]
            ),
            "deploy prod"
        );
    }

    #[test]
    fn an_empty_default_is_no_default() {
        // `{{env:}}` reads as "I have not decided yet", so it must not render as blank.
        assert_eq!(parameters("deploy {{env:}}")[0].default, None);
        assert_eq!(render("deploy {{env:}}", &[]), "deploy {{env:}}");
    }

    #[test]
    fn a_value_is_substituted_literally_and_not_re_scanned() {
        // A value containing what looks like a hole must not be expanded again — that is
        // how a template injection turns into a different command.
        assert_eq!(
            render(
                "echo {{a}}",
                &[
                    ("a".to_string(), "{{b}}".to_string()),
                    ("b".to_string(), "surprise".to_string())
                ]
            ),
            "echo {{b}}"
        );
    }

    #[test]
    fn a_value_is_not_shell_quoted_here() {
        // Deliberate: the rendered command is typed into the pane for the user to read and
        // send, so quoting it would mangle a value that is meant to be several arguments —
        // `{{flags}}` as `-v --color=always`. The user sees the line before it runs.
        assert_eq!(
            render(
                "ls {{flags}}",
                &[("flags".to_string(), "-la --color".to_string())]
            ),
            "ls -la --color"
        );
    }

    #[test]
    fn an_unterminated_hole_is_text() {
        for template in ["echo {{unclosed", "echo {{a}", "echo {"] {
            assert!(parameters(template).is_empty(), "{template:?}");
            assert_eq!(render(template, &[]), template);
        }
    }

    #[test]
    fn multi_byte_text_survives_rendering() {
        // The renderer walks bytes, so a naive step would split a character.
        let template = "echo 'héllo wörld — {{name}}' 日本語";
        assert_eq!(
            render(template, &[("name".to_string(), "ok".to_string())]),
            "echo 'héllo wörld — ok' 日本語"
        );
    }

    #[test]
    fn a_name_may_contain_dashes_and_underscores() {
        assert_eq!(
            names("deploy {{target-env}} {{build_id}}"),
            vec!["target-env".to_string(), "build_id".to_string()]
        );
    }

    #[test]
    fn whitespace_around_a_name_is_ignored() {
        let params = parameters("deploy {{ env : staging }}");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "env");
        assert_eq!(params[0].default.as_deref(), Some("staging"));
    }

    #[test]
    fn a_default_may_contain_anything_except_braces() {
        let params = parameters("ssh {{host:user@example.com:22}}");
        assert_eq!(params[0].name, "host");
        // Only the first colon separates; the rest is the default.
        assert_eq!(params[0].default.as_deref(), Some("user@example.com:22"));
    }

    #[test]
    fn a_template_with_no_holes_renders_unchanged() {
        assert_eq!(parameters("cargo test --workspace"), Vec::new());
        assert_eq!(
            render("cargo test --workspace", &[]),
            "cargo test --workspace"
        );
    }
}
