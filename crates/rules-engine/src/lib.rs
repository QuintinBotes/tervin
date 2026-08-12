//! Tervin Rules: provider-neutral policy, approvals, and auditability.
//!
//! Tervin owns this layer rather than delegating it to an agent runtime, so the
//! same policy applies whichever agent — or none — is acting. Two pieces:
//!
//! - [`classify`] judges what an action would do, conservatively.
//! - [`policy`] decides what happens as a result, and records why.

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

pub mod classify;
pub mod policy;

pub use classify::{classify, split_segments, Segment};
pub use policy::{
    default_rules, ActionContext, ActionKind, ApprovalOutcome, ApprovalRequest, ApprovalScope,
    Decision, Effect, Pattern, PolicyRule, ResolveResult, RulesEngine,
};
