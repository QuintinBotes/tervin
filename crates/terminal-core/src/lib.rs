//! Terminal core: PTY sessions and the shell-integration tap.
//!
//! Terminal correctness comes before everything else in Tervin, so this crate is
//! deliberately narrow. It opens PTYs, moves bytes in both directions without
//! altering them, and observes the stream for shell-integration signals. It does
//! not interpret the screen, and it does not decide what a Block is — that
//! belongs to `block-engine`, which consumes the signals emitted here.

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
