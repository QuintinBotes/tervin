//! Shared Tervin domain types.
//!
//! Everything in this crate is provider-neutral by construction. No type here
//! names a model vendor, and no field assumes a particular agent runtime — that
//! is the load-bearing constraint behind product principle 3 (agent agnostic by
//! design). Adapters translate *into* these types; they never leak outward.

// `panic = "abort"` in the release profile means a panic on any thread ends the
// whole window, so a production panic costs the session rather than one feature.
// Each one that remains carries an `#[allow]` whose `reason` is the argument for
// why it cannot fire; a new one has to make that argument or fail the build. What
// this list covers, and the one route it cannot, is written down in tervin-app's
// `tests/production_panics.rs`.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::allow_attributes_without_reason
    )
)]

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
