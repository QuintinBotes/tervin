//! The capability model behind capability-aware UI.
//!
//! Tervin does not fake feature parity between agents. A control is rendered
//! only when the hosting runtime genuinely supports it, and when a runtime
//! partially supports something the UI must be able to explain the limit. That
//! is why `CapabilityLevel` carries a note instead of being a bool.

use serde::{Deserialize, Serialize};

/// How much structure Tervin can get out of a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Documented API, SDK, or structured event output.
    Structured,
    /// Interactive CLI with limited machine-readable output; the terminal
    /// remains authoritative and Tervin extracts only what is reliable.
    EnhancedCli,
    /// An arbitrary managed command. Full terminal fidelity, no adapter.
    GenericTerminal,
    /// A model endpoint that answers but cannot act: no tools, no plans, nothing to
    /// approve.
    ///
    /// A separate tier rather than a structured runtime with most capabilities
    /// switched off, because the tier is shown as a badge and "Tier 1" next to
    /// something that cannot run a command or edit a file would be read as a
    /// promise. What it can do — answer about the workspace, and carry context
    /// between agents — it does completely.
    Conversational,
}

impl Tier {
    pub fn number(&self) -> u8 {
        match self {
            Self::Structured => 1,
            Self::EnhancedCli => 2,
            Self::GenericTerminal => 3,
            // Not a rung on the same ladder: it is a different kind of thing, and
            // numbering it 4 would imply it is worse than a generic terminal.
            Self::Conversational => 0,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Structured => "Tier 1 · Structured",
            Self::EnhancedCli => "Tier 2 · Enhanced CLI",
            Self::GenericTerminal => "Tier 3 · Generic agent terminal",
            Self::Conversational => "Conversational · answers, cannot act",
        }
    }
}

/// Support level for one capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "level", rename_all = "snake_case")]
pub enum CapabilityLevel {
    Supported,
    /// Works, but with a caveat the UI must show verbatim.
    Partial {
        note: String,
    },
    /// Definitively absent. The control is hidden or disabled with this reason.
    Unsupported {
        reason: String,
    },
    /// Not yet probed, or unknowable for this runtime.
    Unknown,
}

impl CapabilityLevel {
    pub fn partial(note: impl Into<String>) -> Self {
        Self::Partial { note: note.into() }
    }

    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self::Unsupported {
            reason: reason.into(),
        }
    }

    /// Whether the corresponding control should be interactive.
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Supported | Self::Partial { .. })
    }

    /// The caveat or reason, when there is one to show.
    pub fn note(&self) -> Option<&str> {
        match self {
            Self::Partial { note } => Some(note),
            Self::Unsupported { reason } => Some(reason),
            _ => None,
        }
    }
}

/// What a runtime can actually do, one field per user-visible affordance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub tier: Tier,
    pub plan_mode: CapabilityLevel,
    pub resume: CapabilityLevel,
    pub tool_events: CapabilityLevel,
    pub file_edits: CapabilityLevel,
    /// Whether Tervin can gate the runtime's actions *before* they execute.
    /// Anything less than `Supported` means approvals are provider-native and
    /// the UI must say so.
    pub native_permission_bridge: CapabilityLevel,
    pub mcp: CapabilityLevel,
    pub hooks: CapabilityLevel,
    pub subagents: CapabilityLevel,
    pub image_input: CapabilityLevel,
    pub cost_reporting: CapabilityLevel,
    pub model_selection: CapabilityLevel,
    pub remote_execution: CapabilityLevel,
    /// Whether the runtime accepts further input on a live session, as opposed
    /// to being a one-shot invocation.
    pub multi_turn: CapabilityLevel,
    pub interrupt: CapabilityLevel,
}

impl Capabilities {
    /// A conservative baseline: nothing is claimed until an adapter proves it.
    pub fn unknown(tier: Tier) -> Self {
        Self {
            tier,
            plan_mode: CapabilityLevel::Unknown,
            resume: CapabilityLevel::Unknown,
            tool_events: CapabilityLevel::Unknown,
            file_edits: CapabilityLevel::Unknown,
            native_permission_bridge: CapabilityLevel::Unknown,
            mcp: CapabilityLevel::Unknown,
            hooks: CapabilityLevel::Unknown,
            subagents: CapabilityLevel::Unknown,
            image_input: CapabilityLevel::Unknown,
            cost_reporting: CapabilityLevel::Unknown,
            model_selection: CapabilityLevel::Unknown,
            remote_execution: CapabilityLevel::Unknown,
            multi_turn: CapabilityLevel::Unknown,
            interrupt: CapabilityLevel::Unknown,
        }
    }

    /// The Tier 3 baseline. A managed command in a pane gives full terminal
    /// fidelity and nothing else; every structured affordance is honestly absent.
    pub fn generic_terminal() -> Self {
        let no_adapter =
            || CapabilityLevel::unsupported("No adapter for this command; output is unstructured.");
        Self {
            tier: Tier::GenericTerminal,
            plan_mode: no_adapter(),
            resume: no_adapter(),
            tool_events: no_adapter(),
            file_edits: CapabilityLevel::partial(
                "File changes are observed through Git, not reported by the agent.",
            ),
            native_permission_bridge: CapabilityLevel::unsupported(
                "Tervin cannot intercept actions of an unmanaged command. Approvals, if any, are the agent's own.",
            ),
            mcp: CapabilityLevel::Unknown,
            hooks: no_adapter(),
            subagents: no_adapter(),
            image_input: no_adapter(),
            cost_reporting: no_adapter(),
            model_selection: no_adapter(),
            remote_execution: CapabilityLevel::partial("Inherits the pane's session, local or remote."),
            multi_turn: CapabilityLevel::partial("Type directly into the pane."),
            interrupt: CapabilityLevel::partial("Signals the process; the agent may not exit cleanly."),
        }
    }

    /// Named capabilities as `(label, level)` pairs for the capability panel,
    /// ordered as the panel presents them.
    pub fn entries(&self) -> Vec<(&'static str, &CapabilityLevel)> {
        vec![
            ("Plan mode", &self.plan_mode),
            ("Resume", &self.resume),
            ("Tool events", &self.tool_events),
            ("File edits", &self.file_edits),
            ("Permission bridge", &self.native_permission_bridge),
            ("MCP", &self.mcp),
            ("Hooks", &self.hooks),
            ("Subagents", &self.subagents),
            ("Image input", &self.image_input),
            ("Cost reporting", &self.cost_reporting),
            ("Model selection", &self.model_selection),
            ("Remote execution", &self.remote_execution),
            ("Multi-turn session", &self.multi_turn),
            ("Interrupt", &self.interrupt),
        ]
    }
}
