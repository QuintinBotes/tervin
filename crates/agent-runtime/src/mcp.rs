//! MCP servers Tervin supplies to agents.
//!
//! ## Why this exists only for ACP
//!
//! Under ACP the *client* supplies MCP servers: `session/new` takes them as a
//! parameter, and an agent has no user config of its own to read. So without this,
//! an ACP agent running in Tervin has no MCP at all — which would make Tervin the
//! worst host for exactly the agents it integrates with most deeply.
//!
//! Claude Code is the opposite case: it reads its own configuration and, on this
//! machine, already had sixty servers connected. Tervin deliberately does **not**
//! forward these to it. Silently adding servers to a runtime that has its own would
//! change the tools available to an agent without the user asking, and "why is this
//! server here" would have no answer anywhere in either product.
//!
//! ## The file format is not Tervin's own
//!
//! `mcp.json` in Tervin's config directory uses the same `mcpServers` object every client
//! uses, so an existing configuration can be pasted in unchanged. Inventing a
//! Tervin-shaped format would mean every user hand-translating something they
//! already have.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One MCP server, as every client spells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServer {
    /// Executable to run. Stdio transport only, which is what ACP defines.
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Set to skip this server without deleting its configuration.
    #[serde(default)]
    pub disabled: bool,
}

/// The on-disk set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default, rename = "mcpServers")]
    pub servers: BTreeMap<String, McpServer>,
}

impl McpConfig {
    /// `mcp.json` in Tervin's config directory.
    ///
    /// The location differs by platform — `~/Library/Application Support/tervin` on
    /// macOS, `~/.config/tervin` on Linux — so it is resolved rather than written down.
    pub fn path() -> PathBuf {
        tervin_core::paths::config_dir().join("mcp.json")
    }

    /// Load the configuration, reporting a parse failure rather than hiding it.
    ///
    /// A malformed file returns an empty set *and* a message. Silently treating a
    /// typo as "no servers" would look identical to working configuration that
    /// simply did nothing.
    pub fn load() -> (Self, Option<String>) {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (Self::default(), None);
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(config) => (config, None),
            Err(e) => (
                Self::default(),
                Some(format!(
                    "{} could not be parsed, so no MCP servers were passed to agents: {e}",
                    path.display()
                )),
            ),
        }
    }

    /// Servers that should actually be started.
    pub fn enabled(&self) -> impl Iterator<Item = (&String, &McpServer)> {
        self.servers.iter().filter(|(_, s)| !s.disabled)
    }

    /// The `mcpServers` parameter for `session/new`.
    ///
    /// ACP wants an array with the name inline; the config file uses an object map,
    /// as every MCP client does. This is the only place that difference lives.
    pub fn to_acp(&self) -> Value {
        Value::Array(
            self.enabled()
                .map(|(name, server)| {
                    json!({
                        "name": name,
                        "command": server.command,
                        "args": server.args,
                        // ACP spells environment as a list of name/value pairs.
                        "env": server
                            .env
                            .iter()
                            .map(|(k, v)| json!({ "name": k, "value": v }))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect(),
        )
    }

    /// What was declared, for the Bridge panel.
    ///
    /// The status says "declared" and says why: ACP has no way to report whether a
    /// server actually connected, so anything more definite would be invented.
    pub fn declared_states(&self) -> Vec<crate::runtime::McpServerState> {
        self.enabled()
            .map(|(name, _)| crate::runtime::McpServerState {
                name: name.clone(),
                status: "declared — the protocol does not report connection state".to_string(),
            })
            .collect()
    }

    /// A starter file, written on first run so there is something to edit.
    pub fn example() -> &'static str {
        r#"{
  "//": "MCP servers Tervin passes to agents that speak the Agent Client Protocol.",
  "//1": "This is the same `mcpServers` format every MCP client uses, so an existing",
  "//2": "configuration can be pasted in unchanged.",
  "//3": "",
  "//4": "These are NOT passed to Claude Code, which reads its own configuration —",
  "//5": "adding servers it did not ask for would change an agent's tools silently.",
  "mcpServers": {}
}
"#
    }

    /// Write the starter file, readable only by its owner.
    ///
    /// Owner-only because of what the user is invited to put in it: every server
    /// entry takes an `env` block, and that is where a server's API token goes. The
    /// file is created empty, but the permissions have to be right before the user
    /// edits it rather than after.
    pub fn write_example(path: &std::path::Path) -> std::io::Result<()> {
        tervin_core::paths::write_private(path, Self::example())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> McpConfig {
        serde_json::from_str(text).expect("could not parse")
    }

    #[test]
    fn the_standard_client_format_parses_unchanged() {
        // Pasted from a real MCP client configuration, field for field.
        let parsed = config(
            r#"{
              "mcpServers": {
                "filesystem": {
                  "command": "npx",
                  "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                  "env": { "LOG_LEVEL": "debug" }
                }
              }
            }"#,
        );
        let server = parsed.servers.get("filesystem").expect("no server");
        assert_eq!(server.command, "npx");
        assert_eq!(server.args.len(), 3);
        assert_eq!(
            server.env.get("LOG_LEVEL").map(String::as_str),
            Some("debug")
        );
        assert!(!server.disabled);
    }

    #[test]
    fn a_file_with_comment_keys_still_parses() {
        // The starter file explains itself with `//` keys, which is the conventional
        // trick in a format with no comments. It must not break loading.
        let parsed: McpConfig =
            serde_json::from_str(McpConfig::example()).expect("the starter file must parse");
        assert!(parsed.servers.is_empty());
    }

    #[test]
    fn a_disabled_server_is_kept_but_not_started() {
        // Deleting configuration to turn something off loses it.
        let parsed =
            config(r#"{"mcpServers":{"a":{"command":"x"},"b":{"command":"y","disabled":true}}}"#);
        assert_eq!(parsed.servers.len(), 2);
        let names: Vec<&String> = parsed.enabled().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["a"]);
    }

    #[test]
    fn the_acp_shape_moves_the_name_inline_and_lists_the_environment() {
        let parsed =
            config(r#"{"mcpServers":{"fs":{"command":"npx","args":["-y","s"],"env":{"K":"V"}}}}"#);
        let acp = parsed.to_acp();
        let entry = &acp[0];
        assert_eq!(entry["name"], json!("fs"));
        assert_eq!(entry["command"], json!("npx"));
        assert_eq!(entry["args"], json!(["-y", "s"]));
        // ACP spells environment as name/value pairs, not an object.
        assert_eq!(entry["env"], json!([{ "name": "K", "value": "V" }]));
    }

    #[test]
    fn an_empty_configuration_produces_an_empty_array_not_null() {
        // The schema requires the parameter; `null` would be a protocol error.
        assert_eq!(McpConfig::default().to_acp(), json!([]));
    }

    #[test]
    fn a_declared_server_reports_only_its_name() {
        // `declared_states` is what the Bridge panel renders, and an MCP entry
        // routinely holds an API token in `env` — that block exists for exactly that.
        // A panel row built from anything but the name would put a live credential on
        // screen and carry it into whatever that panel's state is serialised into.
        //
        // `to_acp` deliberately *does* carry `env` values, and that is not an
        // inconsistency: passing the server to the agent is the entire point of
        // configuring one, and it goes to the process the user asked for rather than
        // into a file, a panel, or an export. That half is asserted by
        // `the_acp_shape_moves_the_name_inline_and_lists_the_environment`.
        let parsed = config(
            r#"{"mcpServers":{"paid":{
                 "command":"secret-server-binary",
                 "args":["--token","sk-arg-never-rendered"],
                 "env":{"API_KEY":"sk-env-never-rendered"}
               }}}"#,
        );

        let states = parsed.declared_states();
        assert_eq!(states.len(), 1);
        let json = serde_json::to_string(&states).expect("a panel state must serialise");
        assert!(
            json.contains("paid"),
            "the name is the whole point of the row: {json}"
        );
        for leaked in [
            "secret-server-binary",
            "sk-arg-never-rendered",
            "API_KEY",
            "sk-env-never-rendered",
        ] {
            assert!(
                !json.contains(leaked),
                "`{leaked}` reached the Bridge panel: {json}"
            );
        }
    }

    #[test]
    fn declared_servers_never_claim_to_be_connected() {
        let parsed = config(r#"{"mcpServers":{"fs":{"command":"x"}}}"#);
        let states = parsed.declared_states();
        assert_eq!(states.len(), 1);
        assert!(
            states[0].status.contains("does not report"),
            "status must not imply a connection Tervin cannot see: {}",
            states[0].status
        );
    }

    #[test]
    fn a_malformed_file_is_reported_rather_than_read_as_empty() {
        // Silently treating a typo as "no servers" looks identical to configuration
        // that worked and did nothing.
        let err = serde_json::from_str::<McpConfig>("{ not json }").err();
        assert!(err.is_some());
    }

    #[test]
    fn servers_keep_a_stable_order() {
        // A map with arbitrary iteration order would reorder the Bridge panel on
        // every refresh.
        let parsed = config(
            r#"{"mcpServers":{"zeta":{"command":"z"},"alpha":{"command":"a"},"mid":{"command":"m"}}}"#,
        );
        let names: Vec<&String> = parsed.enabled().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["alpha", "mid", "zeta"]);
    }
}
