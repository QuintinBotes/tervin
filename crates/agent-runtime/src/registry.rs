//! Runtime discovery and the adapter registry.
//!
//! Tervin lists every agent it can find, not only the ones it has a deep adapter
//! for. An agent with no adapter still runs — as a Tier 3 managed command in a
//! pane, with full terminal fidelity and no structured events — and the Bridge
//! panel says exactly that. Hiding a runtime because Tervin cannot introspect it
//! would make Tervin the thing deciding which agents a user is allowed to use.
//!
//! ## Where Tier 3 lives
//!
//! Tier 3 is deliberately *not* implemented as an [`AgentRuntime`]. A generic
//! agent is a command in a terminal pane, and panes are owned by the application
//! layer, which already renders them, captures Blocks from them, and tracks their
//! Git delta. Wrapping that in an adapter would mean a second, parallel owner of
//! the same PTY. What this module provides instead is honest discovery and
//! capability reporting, so a Tier 3 agent appears in the picker and the UI knows
//! precisely which controls to withhold.

use crate::acp::{known_acp_agents, AcpRuntime};
use crate::claude::ClaudeCodeRuntime;
use crate::local::{known_local_endpoints, LocalModelRuntime};
use crate::runtime::{AgentRuntime, Discovery, PermissionArbiter};
use std::sync::Arc;
use tervin_core::{Capabilities, Tier};

/// A known agent that has no structured adapter.
#[derive(Debug, Clone, Copy)]
pub struct GenericAgent {
    pub runtime_id: &'static str,
    pub display_name: &'static str,
    pub binary: &'static str,
    /// Why there is no deeper integration yet, shown in the Bridge panel.
    ///
    /// Written to be useful rather than apologetic: it names the route to a better
    /// integration where one exists, so a user can take it today instead of waiting
    /// for Tervin to ship an adapter.
    pub note: &'static str,
}

/// Agents Tervin recognises and can host generically.
pub const GENERIC_AGENTS: [GenericAgent; 5] = [
    GenericAgent {
        runtime_id: "codex",
        display_name: "Codex",
        binary: "codex",
        note: "Runs as a managed pane with its own interface. Codex has a \
               machine-readable mode (`codex exec --json`) that a structured adapter \
               could read, but it is one-shot and has no permission callback, so it \
               would give Tervin a timeline without a gate. Not implemented yet, and \
               not claimed.",
    },
    GenericAgent {
        runtime_id: "gemini",
        display_name: "Gemini CLI (own UI)",
        binary: "gemini",
        note: "Runs as a managed pane with its own interface. For structured events, \
               plans, and a real permission gate, use the Gemini CLI entry that speaks \
               the Agent Client Protocol instead.",
    },
    GenericAgent {
        runtime_id: "aider",
        display_name: "Aider",
        binary: "aider",
        note: "Runs as a managed pane. Aider commits its own work, so Tervin sees \
               changes through Git rather than being told about them — the Review \
               surface still works, attributed to the commit rather than to a tool \
               call.",
    },
    GenericAgent {
        runtime_id: "opencode",
        display_name: "OpenCode",
        binary: "opencode",
        note: "Runs as a managed pane with its own interface. If your build speaks the \
               Agent Client Protocol, add it under Settings › Agents › Add an ACP \
               agent for a full structured integration with a real permission gate.",
    },
    GenericAgent {
        runtime_id: "cursor-agent",
        display_name: "Cursor Agent",
        binary: "cursor-agent",
        note: "Runs as a managed pane with its own interface. If your build speaks the \
               Agent Client Protocol, add it under Settings › Agents › Add an ACP \
               agent for a full structured integration with a real permission gate.",
    },
];

/// Adapters and discovery.
pub struct RuntimeRegistry {
    runtimes: Vec<Arc<dyn AgentRuntime>>,
}

impl RuntimeRegistry {
    /// Build the registry, wiring Tervin Rules in as the permission arbiter.
    ///
    /// One adapter is registered per known ACP agent. They share the ACP
    /// implementation entirely — the spec is data, not code — which is the whole
    /// argument for integrating with a protocol rather than with vendors.
    pub fn new(arbiter: Option<Arc<dyn PermissionArbiter>>) -> Self {
        let mut runtimes: Vec<Arc<dyn AgentRuntime>> = Vec::new();

        runtimes.push(Arc::new(match arbiter.clone() {
            Some(a) => ClaudeCodeRuntime::new().with_arbiter(a),
            None => ClaudeCodeRuntime::new(),
        }));

        for spec in known_acp_agents() {
            runtimes.push(Arc::new(match arbiter.clone() {
                Some(a) => AcpRuntime::new(spec).with_arbiter(a),
                None => AcpRuntime::new(spec),
            }));
        }

        // Model endpoints take no arbiter: they cannot act, so there is nothing to
        // gate. Passing one would imply a decision that never happens.
        for endpoint in known_local_endpoints() {
            runtimes.push(Arc::new(LocalModelRuntime::new(endpoint)));
        }

        Self { runtimes }
    }

    /// Register a user-configured model endpoint.
    ///
    /// Any server speaking the OpenAI dialect, local or remote — the same argument as
    /// [`Self::add_acp_agent`], applied to models rather than agents.
    pub fn add_local_model(
        &mut self,
        runtime_id: impl Into<String>,
        display_name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
    ) {
        let runtime_id = runtime_id.into();
        self.runtimes.retain(|r| r.runtime_id() != runtime_id);
        self.runtimes.push(Arc::new(
            LocalModelRuntime::custom(runtime_id, display_name, base_url).with_api_key(api_key),
        ));
    }

    /// Register a user-configured ACP agent.
    ///
    /// This is how an agent Tervin has never heard of becomes a first-class,
    /// structured integration without a release.
    pub fn add_acp_agent(
        &mut self,
        spec: crate::acp::AcpAgentSpec,
        arbiter: Option<Arc<dyn PermissionArbiter>>,
    ) {
        // Replacing rather than appending, so re-registering an id does not leave a
        // stale adapter shadowing the new one.
        self.runtimes.retain(|r| r.runtime_id() != spec.runtime_id);
        self.runtimes.push(Arc::new(match arbiter {
            Some(a) => AcpRuntime::new(spec).with_arbiter(a),
            None => AcpRuntime::new(spec),
        }));
    }

    pub fn get(&self, runtime_id: &str) -> Option<Arc<dyn AgentRuntime>> {
        self.runtimes
            .iter()
            .find(|r| r.runtime_id() == runtime_id)
            .cloned()
    }

    pub fn adapters(&self) -> &[Arc<dyn AgentRuntime>] {
        &self.runtimes
    }

    /// Clone the adapter handles so discovery can run without holding a lock on
    /// the registry across an await.
    pub fn snapshot(&self) -> Vec<Arc<dyn AgentRuntime>> {
        self.runtimes.clone()
    }

    /// Everything Tervin can find on this machine, adapters first.
    pub async fn discover_all(&self) -> Vec<Discovery> {
        let mut out = Vec::new();
        for runtime in &self.runtimes {
            out.push(runtime.discover().await);
        }
        for agent in GENERIC_AGENTS {
            out.push(discover_generic(agent).await);
        }
        out
    }
}

/// Discover a generic agent by looking for its binary.
pub async fn discover_generic(agent: GenericAgent) -> Discovery {
    let path = crate::which(agent.binary);
    let available = path.is_some();

    let mut notes = vec![agent.note.to_string()];
    if !available {
        notes.insert(0, format!("`{}` was not found on PATH.", agent.binary));
    }

    Discovery {
        runtime_id: agent.runtime_id.to_string(),
        display_name: agent.display_name.to_string(),
        available,
        // Version probing is skipped: every CLI spells `--version` differently
        // and guessing wrong would print a misleading number.
        version: None,
        path,
        notes,
        capabilities: Capabilities::generic_terminal(),
    }
}

/// The tier Tervin can offer for a runtime id.
pub fn tier_for(runtime_id: &str) -> Tier {
    if runtime_id == "claude-code"
        || known_acp_agents()
            .iter()
            .any(|a| a.runtime_id == runtime_id)
    {
        return Tier::Structured;
    }
    if known_local_endpoints()
        .iter()
        .any(|e| e.runtime_id == runtime_id)
    {
        return Tier::Conversational;
    }
    // Anything registered as an ACP agent is structured by construction, so the
    // suffix is honoured for user-configured agents too.
    if runtime_id.ends_with("-acp") {
        return Tier::Structured;
    }
    Tier::GenericTerminal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discovery_lists_adapters_and_generic_agents() {
        let registry = RuntimeRegistry::new(None);
        let found = registry.discover_all().await;

        assert!(found.iter().any(|d| d.runtime_id == "claude-code"));
        for agent in GENERIC_AGENTS {
            assert!(
                found.iter().any(|d| d.runtime_id == agent.runtime_id),
                "{} missing from discovery",
                agent.runtime_id
            );
        }
    }

    #[tokio::test]
    async fn unavailable_runtimes_are_listed_with_an_explanation() {
        // A runtime that is not installed still appears, so the user learns it is
        // supported rather than finding an empty list.
        let found = discover_generic(GENERIC_AGENTS[0]).await;
        assert!(!found.notes.is_empty());
        if !found.available {
            assert!(found.notes[0].contains("not found on PATH"));
        }
    }

    #[test]
    fn generic_agents_declare_no_structured_capabilities() {
        // The UI must withhold controls that cannot work, with a reason.
        let caps = Capabilities::generic_terminal();
        assert_eq!(caps.tier, Tier::GenericTerminal);
        assert!(!caps.plan_mode.is_usable());
        assert!(!caps.tool_events.is_usable());
        assert!(!caps.cost_reporting.is_usable());
        assert!(
            caps.plan_mode.note().is_some(),
            "a refusal must be explained"
        );
        assert!(
            !caps.native_permission_bridge.is_usable(),
            "Tervin cannot gate an unmanaged command and must not imply it can"
        );
    }

    #[test]
    fn structured_tier_covers_claude_code_and_every_acp_agent() {
        assert_eq!(tier_for("claude-code"), Tier::Structured);
        assert_eq!(tier_for("gemini-acp"), Tier::Structured);
        // A user-configured ACP agent is structured too: the protocol is what
        // makes it so, not a hard-coded list.
        assert_eq!(tier_for("some-new-agent-acp"), Tier::Structured);
        assert_eq!(tier_for("aider"), Tier::GenericTerminal);
        assert_eq!(tier_for("something-new"), Tier::GenericTerminal);
    }

    #[tokio::test]
    async fn every_known_acp_agent_gets_an_adapter() {
        let registry = RuntimeRegistry::new(None);
        for spec in known_acp_agents() {
            assert!(
                registry.get(&spec.runtime_id).is_some(),
                "{} has no adapter",
                spec.runtime_id
            );
        }
    }

    #[test]
    fn a_user_configured_acp_agent_can_be_registered_and_replaced() {
        let mut registry = RuntimeRegistry::new(None);
        let before = registry.adapters().len();

        let spec = crate::acp::AcpAgentSpec {
            runtime_id: "my-agent-acp".into(),
            display_name: "My agent".into(),
            binary: "my-agent".into(),
            args: vec!["--acp".into()],
            note: "n".into(),
            install_hint: String::new(),
        };
        registry.add_acp_agent(spec.clone(), None);
        assert_eq!(registry.adapters().len(), before + 1);
        assert!(registry.get("my-agent-acp").is_some());

        // Re-registering must not leave a stale adapter shadowing the new one.
        registry.add_acp_agent(spec, None);
        assert_eq!(registry.adapters().len(), before + 1);
    }

    #[tokio::test]
    async fn an_acp_adapter_declares_a_real_permission_gate() {
        // The reason ACP is worth adopting, asserted at the registry level so it
        // cannot regress silently.
        let registry = RuntimeRegistry::new(None);
        let acp = registry.get("gemini-acp").expect("no gemini-acp adapter");
        assert!(
            acp.capabilities().native_permission_bridge.is_usable(),
            "an ACP adapter must offer a usable permission bridge"
        );
    }
}
