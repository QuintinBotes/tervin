//! Agent runtimes: the interface, discovery, profiles, and adapters.
//!
//! Tervin is agent-agnostic by construction. Nothing above this crate knows which
//! agent is running: adapters translate a runtime's dialect into Tervin's event
//! vocabulary, and declare honestly what they cannot do.
//!
//! - [`runtime`] — the `AgentRuntime` interface and its supporting types.
//! - [`claude`] — the Claude Code adapter (Tier 1, structured).
//! - [`acp`] — the Agent Client Protocol adapter, which covers every agent that
//!   speaks ACP rather than one vendor. It is also the only adapter with a genuine
//!   pre-execution permission gate.
//! - [`handoff`] — the Context Bundle, which moves work between agents by turning
//!   the provider-neutral event stream into a briefing another agent can read.
//! - [`local`] — OpenAI-compatible model endpoints (LM Studio, Ollama, vLLM,
//!   llama.cpp). Conversational, not agentic: they answer and cannot act.
//! - [`mcp`] — MCP servers Tervin supplies to ACP agents, which have no config of
//!   their own to read.
//! - [`profile`] — multiple installs or accounts of the same runtime.
//! - [`registry`] — discovery across adapters and generic agents.

pub mod acp;
pub mod claude;
pub mod codex;
pub mod handoff;
pub mod local;
pub mod mcp;
pub mod profile;
pub mod registry;
pub mod runtime;

pub use acp::{known_acp_agents, AcpAgentSpec, AcpRuntime};
pub use claude::ClaudeCodeRuntime;
pub use codex::{CodexNormalizer, CodexRuntime};
pub use handoff::{CommandRecord, ContextBundle};
pub use local::{known_local_endpoints, LocalEndpoint, LocalModelRuntime};
pub use mcp::{McpConfig, McpServer};
pub use profile::{AgentProfile, ImportCandidate, ProfileConfig};
pub use registry::{RuntimeRegistry, GENERIC_AGENTS};
pub use runtime::{
    AgentRuntime, AgentSession, ArbiterDecision, Attachment, Discovery, LaunchConfig,
    LaunchedSession, PermissionArbiter, PermissionState, RuntimeError, SessionMetadata,
};

/// Resolve a binary on `PATH`.
///
/// Shared by discovery and profiles so both answer "where does this come from"
/// the same way.
pub(crate) fn which(binary: &str) -> Option<String> {
    if binary.contains('/') {
        return std::path::Path::new(binary)
            .exists()
            .then(|| binary.to_string());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
        .map(|p| p.display().to_string())
}
