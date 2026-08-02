//! Shared Tervin domain types.
//!
//! Everything in this crate is provider-neutral by construction. No type here
//! names a model vendor, and no field assumes a particular agent runtime — that
//! is the load-bearing constraint behind product principle 3 (agent agnostic by
//! design). Adapters translate *into* these types; they never leak outward.

pub mod capability;
pub mod events;
pub mod ids;
pub mod paths;
pub mod risk;
pub mod thread;

pub use capability::{Capabilities, CapabilityLevel, Tier};
pub use events::{EventPayload, Link, RawRef, TervinEvent};
pub use ids::{
    ArtifactId, BlockId, DiagnosticId, EventId, PaneId, ProjectId, RequestId, SessionId, TabId,
    ThreadId, WorkspaceId,
};
pub use risk::{RiskAssessment, RiskCategory, RiskLevel};
pub use thread::{AgentIdentity, ThreadState};

/// Wall-clock instant used across every module, so timeline ordering is
/// comparable between terminal, git, and agent sources.
pub type Timestamp = chrono::DateTime<chrono::Utc>;

pub fn now() -> Timestamp {
    chrono::Utc::now()
}
