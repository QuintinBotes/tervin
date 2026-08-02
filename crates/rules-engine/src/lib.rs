//! Tervin Rules: provider-neutral policy, approvals, and auditability.
//!
//! Tervin owns this layer rather than delegating it to an agent runtime, so the
//! same policy applies whichever agent — or none — is acting. Two pieces:
//!
//! - [`classify`] judges what an action would do, conservatively.
//! - [`policy`] decides what happens as a result, and records why.

pub mod classify;
pub mod policy;

pub use classify::{classify, split_segments, Segment};
pub use policy::{
    default_rules, ActionContext, ActionKind, ApprovalOutcome, ApprovalRequest, ApprovalScope,
    Decision, Effect, Pattern, PolicyRule, ResolveResult, RulesEngine,
};
