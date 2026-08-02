//! Commands an agent ran, as Blocks.
//!
//! A command an agent runs is the same kind of thing as a command you ran: it has a
//! command line, output, an outcome, and diagnostics worth jumping to. Until now it was
//! only a row on a Thread's timeline — so it could not be searched with the rest of your
//! history, bookmarked, or found again next week, and `Block::thread_id` sat unused.
//!
//! This turns `command.started` / `command.output` / `command.completed` into Blocks
//! attributed to the Thread that produced them.
//!
//! ## Two things it deliberately does not claim
//!
//! **The exit code, unless a runtime actually reported one.** An ACP terminal reports a
//! real status. Claude Code reports success or failure and nothing more, and the 0/1/130
//! on its events is Tervin's inference — so a Block from it carries `exit_code: None` and
//! a status of Succeeded or Failed. A Block claiming "exit 1" that no runtime ever said
//! would be worse than one that admits it does not know, because an exit code is the one
//! field people read as fact.
//!
//! **Complete output.** Adapters pass a bounded excerpt — 4000 characters — because the
//! timeline needs to stay cheap to render. So these Blocks are marked truncated, which is
//! what stops a partial log being read as the whole story.
//!
//! Both are visible in the UI rather than explained in a doc comment nobody reads.

use block_engine::{Block, BlockOutput, BlockStatus, Store};
use parking_lot::Mutex;
use std::collections::HashMap;
use tervin_core::{EventPayload, PaneId, SessionId, ThreadId};

/// Bytes of excerpt kept for one Block, across however many output events arrive.
///
/// Generous next to a single excerpt and far below the inline cap, so a chatty command
/// cannot make a Thread's Blocks the largest rows in the database.
const MAX_OUTPUT: usize = 64 * 1024;

/// Commands in flight, one per Thread.
///
/// One at a time per Thread is the right model: a runtime reports its tool calls in
/// sequence, and a second `command.started` before a completion means the first will
/// never be completed — so it is closed as Unknown rather than left open forever.
#[derive(Default)]
pub struct AgentBlocks {
    open: Mutex<HashMap<ThreadId, InFlight>>,
}

struct InFlight {
    block: Block,
    output: String,
    /// True once output was dropped, so the Block can say the log is partial for the
    /// right reason rather than always.
    over_cap: bool,
}

/// What the caller should do with one event.
#[derive(Default)]
pub struct BlockUpdate {
    /// A Block that just started, to show as running.
    pub started: Option<Block>,
    /// A Block that finished, to persist and show.
    pub finished: Option<Block>,
}

impl AgentBlocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one Thread event, returning any Block activity it produced.
    ///
    /// `cwd` comes from the event rather than from the Thread, because an agent can run a
    /// command somewhere other than where the Thread started.
    pub fn observe(&self, thread_id: &ThreadId, event: &tervin_core::TervinEvent) -> BlockUpdate {
        let mut update = BlockUpdate::default();
        let mut open = self.open.lock();

        match &event.payload {
            EventPayload::CommandStarted { command, .. } => {
                // A previous command with no completion: close it as Unknown rather than
                // leaving a row that says "running" for the rest of the session.
                if let Some(stale) = open.remove(thread_id) {
                    update.finished = Some(finish(stale, None, BlockStatus::Unknown, 0));
                }

                let mut block = Block::new(
                    // Agent commands have no pane. A synthetic id keyed to the Thread
                    // keeps the column non-null and honest about where it came from,
                    // rather than borrowing a pane the command never ran in.
                    PaneId::from_external(format!("thread:{}", thread_id.as_str())),
                    SessionId::new(),
                    command.clone(),
                    event.cwd.clone().unwrap_or_default(),
                    "local".to_string(),
                );
                block.thread_id = Some(thread_id.clone());
                block.project = event.project.clone();
                // No shell: the agent invoked the command, and naming one would imply
                // rc files and aliases applied when they may not have.
                block.shell = None;
                block.status = BlockStatus::Running;

                update.started = Some(block.clone());
                open.insert(
                    thread_id.clone(),
                    InFlight {
                        block,
                        output: String::new(),
                        over_cap: false,
                    },
                );
            }

            EventPayload::CommandOutput { excerpt, .. } => {
                if let Some(flight) = open.get_mut(thread_id) {
                    let room = MAX_OUTPUT.saturating_sub(flight.output.len());
                    if room == 0 {
                        flight.over_cap = true;
                    } else if excerpt.len() > room {
                        // Cut on a character boundary: this is displayed, and a split
                        // multi-byte character renders as a replacement glyph.
                        let mut end = room;
                        while end > 0 && !excerpt.is_char_boundary(end) {
                            end -= 1;
                        }
                        flight.output.push_str(&excerpt[..end]);
                        flight.over_cap = true;
                    } else {
                        flight.output.push_str(excerpt);
                    }
                }
            }

            EventPayload::CommandCompleted {
                exit_code,
                duration_ms,
                exit_code_reported,
                ..
            } => {
                if let Some(flight) = open.remove(thread_id) {
                    // Reported: the number is real and the status follows shell
                    // convention. Not reported: the number is Tervin's inference, so the
                    // Block keeps the outcome and drops the number.
                    let (code, status) = if *exit_code_reported {
                        (
                            Some(*exit_code),
                            BlockStatus::from_exit_code(Some(*exit_code)),
                        )
                    } else if *exit_code == 130 {
                        (None, BlockStatus::Interrupted)
                    } else if *exit_code == 0 {
                        (None, BlockStatus::Succeeded)
                    } else {
                        (None, BlockStatus::Failed)
                    };
                    update.finished = Some(finish(flight, code, status, *duration_ms));
                }
            }

            // A Thread that ends mid-command leaves a Block that would otherwise say
            // "running" forever.
            EventPayload::ThreadCompleted { .. }
            | EventPayload::ThreadFailed { .. }
            | EventPayload::ThreadState {
                state:
                    tervin_core::ThreadState::Completed
                    | tervin_core::ThreadState::Failed
                    | tervin_core::ThreadState::Interrupted
                    | tervin_core::ThreadState::Disconnected,
            } => {
                if let Some(flight) = open.remove(thread_id) {
                    update.finished = Some(finish(flight, None, BlockStatus::Unknown, 0));
                }
            }

            _ => {}
        }

        update
    }

    /// Close anything still open for a Thread, for a session that ended abruptly.
    pub fn close_thread(&self, thread_id: &ThreadId) -> Option<Block> {
        let flight = self.open.lock().remove(thread_id)?;
        Some(finish(flight, None, BlockStatus::Unknown, 0))
    }

    /// How many commands are open. Exposed so a leak is observable rather than inferred.
    pub fn in_flight(&self) -> usize {
        self.open.lock().len()
    }
}

/// Finish a Block: attach output, parse it, and stamp the outcome.
fn finish(
    flight: InFlight,
    exit_code: Option<i32>,
    status: BlockStatus,
    duration_ms: u64,
) -> Block {
    let InFlight {
        mut block,
        output,
        over_cap,
    } = flight;

    let bytes = output.into_bytes();
    block.output = BlockOutput {
        total_bytes: bytes.len() as u64,
        // Always truncated, and for two reasons worth keeping apart: adapters pass a
        // bounded excerpt rather than the full stream, and a very chatty command can also
        // exceed the cap here. Either way the log is partial, and a Block that did not say
        // so would be read as complete.
        truncated: true,
        inline: bytes,
        spill_path: None,
    };
    let _ = over_cap;

    block.parsed = block_engine::parse::extract(&block.output.inline_text(), &block.cwd);
    block.exit_code = exit_code;
    block.status = status;
    block.duration_ms = Some(duration_ms).filter(|d| *d > 0);
    block.ended_at = Some(tervin_core::now());
    block
}

/// Persist and announce Block activity from an agent.
pub fn apply(store: &Store, app: &tauri::AppHandle, update: BlockUpdate) {
    use tauri::Emitter as _;

    if let Some(block) = update.started {
        let _ = app.emit("block://started", &block);
    }
    if let Some(block) = update.finished {
        if let Err(e) = store.upsert_block(&block) {
            tracing::warn!("could not persist an agent Block: {e}");
        }
        let _ = app.emit("block://finished", &block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tervin_core::{AgentIdentity, TervinEvent, Tier};

    fn identity() -> AgentIdentity {
        AgentIdentity::new("claude-code", "Claude Code", Tier::Structured)
    }

    fn event(payload: EventPayload) -> TervinEvent {
        let mut event = TervinEvent::new(identity(), "summary".to_string(), payload);
        event.cwd = Some("/proj".to_string());
        event.project = Some("proj".to_string());
        event
    }

    fn started(command: &str) -> TervinEvent {
        event(EventPayload::CommandStarted {
            command: command.to_string(),
            block_id: None,
        })
    }

    fn output(text: &str) -> TervinEvent {
        event(EventPayload::CommandOutput {
            stream: tervin_core::events::OutputStream::Stdout,
            excerpt: text.to_string(),
            block_id: None,
        })
    }

    fn completed(exit_code: i32, reported: bool) -> TervinEvent {
        event(EventPayload::CommandCompleted {
            command: "cargo test".to_string(),
            exit_code,
            duration_ms: 1500,
            exit_code_reported: reported,
            block_id: None,
        })
    }

    fn thread() -> ThreadId {
        ThreadId::new()
    }

    #[test]
    fn a_command_becomes_a_block_attributed_to_its_thread() {
        let blocks = AgentBlocks::new();
        let id = thread();

        let start = blocks.observe(&id, &started("cargo test --workspace"));
        let block = start.started.expect("a Block should have started");
        assert_eq!(block.command, "cargo test --workspace");
        // The field that existed for this and was never set.
        assert_eq!(block.thread_id.as_ref(), Some(&id));
        assert_eq!(block.status, BlockStatus::Running);
        assert_eq!(block.cwd, "/proj");
        assert_eq!(block.project.as_deref(), Some("proj"));
        // No shell: naming one would imply rc files and aliases applied.
        assert_eq!(block.shell, None);
        // The pane is synthetic and says where it came from, rather than borrowing a
        // pane the command never ran in.
        assert!(block.pane_id.as_str().starts_with("thread:"));

        blocks.observe(&id, &output("running 470 tests\n"));
        let done = blocks.observe(&id, &completed(0, true));
        let finished = done.finished.expect("a Block should have finished");
        assert_eq!(
            finished.id, block.id,
            "the finished Block is a different one"
        );
        assert!(finished.output.inline_text().contains("470 tests"));
        assert_eq!(finished.duration_ms, Some(1500));
    }

    #[test]
    fn a_reported_exit_code_is_kept() {
        let blocks = AgentBlocks::new();
        let id = thread();
        blocks.observe(&id, &started("false"));
        let block = blocks.observe(&id, &completed(3, true)).finished.unwrap();

        // An ACP terminal reports a real status, so the number is fact and is shown.
        assert_eq!(block.exit_code, Some(3));
        assert_eq!(block.status, BlockStatus::Failed);
    }

    #[test]
    fn a_derived_exit_code_is_not_presented_as_fact() {
        let blocks = AgentBlocks::new();
        let id = thread();

        // Claude Code reports failure and never a status; the 1 is Tervin's inference.
        blocks.observe(&id, &started("cargo test"));
        let failed = blocks.observe(&id, &completed(1, false)).finished.unwrap();
        assert_eq!(
            failed.exit_code, None,
            "a Block claimed an exit code no runtime reported"
        );
        // The outcome is still known, and is what the row shows.
        assert_eq!(failed.status, BlockStatus::Failed);

        blocks.observe(&id, &started("cargo test"));
        let ok = blocks.observe(&id, &completed(0, false)).finished.unwrap();
        assert_eq!(ok.exit_code, None);
        assert_eq!(ok.status, BlockStatus::Succeeded);

        blocks.observe(&id, &started("sleep 100"));
        let stopped = blocks
            .observe(&id, &completed(130, false))
            .finished
            .unwrap();
        assert_eq!(stopped.exit_code, None);
        assert_eq!(stopped.status, BlockStatus::Interrupted);
    }

    #[test]
    fn output_is_marked_partial_because_an_adapter_only_passes_an_excerpt() {
        let blocks = AgentBlocks::new();
        let id = thread();
        blocks.observe(&id, &started("cargo test"));
        blocks.observe(&id, &output("a short line"));
        let block = blocks.observe(&id, &completed(0, true)).finished.unwrap();

        // The adapters truncate to 4000 characters before the event is even built, so no
        // agent Block holds a complete log. Saying so is what stops a partial log being
        // read as the whole story.
        assert!(
            block.output.truncated,
            "a partial log was presented as complete"
        );
    }

    #[test]
    fn diagnostics_are_parsed_out_of_the_output() {
        let blocks = AgentBlocks::new();
        let id = thread();
        blocks.observe(&id, &started("cargo build"));
        blocks.observe(
            &id,
            &output("error[E0433]: cannot find crate\n --> src/main.rs:4:5\n"),
        );
        let block = blocks.observe(&id, &completed(1, true)).finished.unwrap();

        // The reason a Block is worth more than a timeline row: the error is jumpable.
        assert!(block.parsed.error_count > 0, "no diagnostics were parsed");
    }

    #[test]
    fn a_second_start_closes_the_first_as_unknown() {
        // A runtime that reports a new command without completing the previous one would
        // otherwise leave a row saying "running" for the rest of the session.
        let blocks = AgentBlocks::new();
        let id = thread();
        blocks.observe(&id, &started("first"));

        let update = blocks.observe(&id, &started("second"));
        let closed = update
            .finished
            .expect("the first Block should have been closed");
        assert_eq!(closed.command, "first");
        assert_eq!(closed.status, BlockStatus::Unknown);
        assert_eq!(closed.exit_code, None);
        assert_eq!(blocks.in_flight(), 1);
    }

    #[test]
    fn a_thread_ending_mid_command_closes_it() {
        let blocks = AgentBlocks::new();
        let id = thread();
        blocks.observe(&id, &started("sleep 100"));

        let update = blocks.observe(
            &id,
            &event(EventPayload::ThreadFailed {
                reason: "the agent exited".to_string(),
                recoverable: Some(false),
            }),
        );
        assert_eq!(
            update.finished.map(|b| b.status),
            Some(BlockStatus::Unknown),
            "a command open when the Thread died was left running"
        );
        assert_eq!(blocks.in_flight(), 0);
    }

    #[test]
    fn two_threads_do_not_share_a_command() {
        let blocks = AgentBlocks::new();
        let (a, b) = (thread(), thread());

        blocks.observe(&a, &started("in-a"));
        blocks.observe(&b, &started("in-b"));
        blocks.observe(&a, &output("output for a"));

        let done_a = blocks.observe(&a, &completed(0, true)).finished.unwrap();
        assert_eq!(done_a.command, "in-a");
        assert!(done_a.output.inline_text().contains("output for a"));

        let done_b = blocks.observe(&b, &completed(0, true)).finished.unwrap();
        assert_eq!(done_b.command, "in-b");
        // The output belonged to the other Thread's command.
        assert!(done_b.output.inline_text().is_empty());
    }

    #[test]
    fn output_with_no_open_command_is_ignored_rather_than_starting_one() {
        // An adapter can report output for a call Tervin never saw start. Inventing a
        // Block for it would produce a row with no command line.
        let blocks = AgentBlocks::new();
        let id = thread();
        let update = blocks.observe(&id, &output("orphaned"));
        assert!(update.started.is_none() && update.finished.is_none());
        assert_eq!(blocks.in_flight(), 0);
    }

    #[test]
    fn a_completion_with_no_start_produces_nothing() {
        let blocks = AgentBlocks::new();
        assert!(blocks
            .observe(&thread(), &completed(0, true))
            .finished
            .is_none());
    }

    #[test]
    fn a_flood_of_output_is_capped_on_a_character_boundary() {
        let blocks = AgentBlocks::new();
        let id = thread();
        blocks.observe(&id, &started("yes"));
        // Multi-byte, so a byte-wise cut would split a character.
        for _ in 0..40 {
            blocks.observe(&id, &output(&"é".repeat(4000)));
        }
        let block = blocks.observe(&id, &completed(0, true)).finished.unwrap();

        assert!(block.output.inline.len() <= MAX_OUTPUT);
        // Valid UTF-8 throughout, with no replacement glyph from a split character.
        let text = block.output.inline_text();
        assert!(!text.contains('\u{FFFD}'), "output was cut mid-character");
    }

    #[test]
    fn a_zero_duration_is_reported_as_unknown_rather_than_as_instant() {
        // Claude Code omits the duration for some calls, and the adapter passes 0. A
        // Block saying "0ms" would look like a measurement.
        let blocks = AgentBlocks::new();
        let id = thread();
        blocks.observe(&id, &started("cargo test"));
        let block = blocks
            .observe(
                &id,
                &event(EventPayload::CommandCompleted {
                    command: "cargo test".to_string(),
                    exit_code: 0,
                    duration_ms: 0,
                    exit_code_reported: true,
                    block_id: None,
                }),
            )
            .finished
            .unwrap();
        assert_eq!(block.duration_ms, None);
    }

    #[test]
    fn closing_a_thread_returns_whatever_was_open() {
        let blocks = AgentBlocks::new();
        let id = thread();
        blocks.observe(&id, &started("still going"));

        let closed = blocks
            .close_thread(&id)
            .expect("the open command should be returned");
        assert_eq!(closed.status, BlockStatus::Unknown);
        assert_eq!(blocks.in_flight(), 0);
        // And closing again is harmless.
        assert!(blocks.close_thread(&id).is_none());
    }

    #[test]
    fn a_command_with_no_cwd_does_not_invent_one() {
        let blocks = AgentBlocks::new();
        let id = thread();
        let mut start = started("ls");
        start.cwd = None;

        let block = blocks.observe(&id, &start).started.unwrap();
        // Empty rather than a guess. A wrong directory makes every parsed path wrong too.
        assert_eq!(block.cwd, "");
    }
}
