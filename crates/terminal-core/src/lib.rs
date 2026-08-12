//! Terminal core: PTY sessions and the shell-integration tap.
//!
//! Terminal correctness comes before everything else in Tervin, so this crate is
//! deliberately narrow. It opens PTYs, moves bytes in both directions without
//! altering them, and observes the stream for shell-integration signals. It does
//! not interpret the screen, and it does not decide what a Block is — that
//! belongs to `block-engine`, which consumes the signals emitted here.

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

pub mod osc;
pub mod pty;
pub mod registry;
pub mod signals;

pub use osc::{
    ColorScheme, ModeChange, OscHit, OscScanner, PendingMarker, PrivateMode, TerminalQuery,
};
pub use pty::{PositionedSignal, PtyChunk, PtyConfig, PtyError, PtyEvent, PtySession};
pub use registry::TerminalRegistry;
pub use signals::{AgentActivity, AgentEvent, CommandMeta, ShellSignal, AGENT_NOTIFY_TARGET};
