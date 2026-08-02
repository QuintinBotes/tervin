//! Opaque, typed identifiers.
//!
//! These are newtypes rather than bare strings so a `BlockId` can never be
//! passed where a `ThreadId` is expected. Every id serialises transparently, so
//! the wire format the UI sees is just a string.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Mint a fresh identifier. The human-readable prefix makes logs and
            /// audit records legible without a lookup.
            pub fn new() -> Self {
                Self(format!("{}_{}", $prefix, uuid::Uuid::new_v4().simple()))
            }

            /// Adopt an identifier issued by an external system (for example an
            /// agent runtime's own session id) without reformatting it.
            pub fn from_external(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }
    };
}

define_id!(
    /// A structured command/output unit — one Tervin Block.
    BlockId, "blk"
);
define_id!(
    /// A provider-independent coding-agent task — one Tervin Thread.
    ThreadId, "thr"
);
define_id!(
    /// One entry in a Thread's append-only event stream.
    EventId, "evt"
);
define_id!(
    /// A terminal pane inside the terminal canvas.
    PaneId, "pane"
);
define_id!(
    /// A terminal canvas tab, which owns a pane tree.
    TabId, "tab"
);
define_id!(
    /// A persistent project/session/pane arrangement.
    WorkspaceId, "wsp"
);
define_id!(
    /// A project root, keyed by canonical path.
    ProjectId, "prj"
);
define_id!(
    /// A shell, SSH, tmux, or agent-hosted session.
    SessionId, "ses"
);
define_id!(
    /// An approval request awaiting a decision.
    RequestId, "req"
);
define_id!(
    /// A grouped compiler error, test failure, lint warning, or stack trace.
    DiagnosticId, "dgn"
);
define_id!(
    /// An exported or agent-produced file.
    ArtifactId, "art"
);
