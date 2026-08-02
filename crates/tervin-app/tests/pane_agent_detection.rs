//! End to end: real agent bytes in, a searchable Thread out.
//!
//! Every layer in this path was unit-tested separately, and that is exactly the setup
//! that hides an integration bug — a scanner that finds the sequence, a parser that
//! reads it, and a builder that never forwards it would all pass their own tests.
//!
//! So this drives the whole chain with **bytes captured from a real `claude`
//! process**, taken from a PTY rather than written by hand:
//!
//! ```text
//! PTY bytes → OscScanner → signals::parse → BlockBuilder → PaneAgents → Store
//! ```
//!
//! and asserts the two things that actually matter to someone using Tervin: the
//! session shows up as a Thread, and what they typed into a pane is findable later.

use block_engine::{BlockBuilder, BlockEvent, Store};
use terminal_core::{OscScanner, PositionedSignal, PtyChunk};
use tervin_app::pane_agents::PaneAgents;
use tervin_core::{PaneId, SessionId};

/// The three notifications a real session emits, captured verbatim from a PTY. The
/// session id, cwd, project and transcript path are as Claude Code 2.1.220 wrote
/// them; only the home directory is rewritten so the test is portable.
const SESSION_START: &str = "\x1b]777;notify;warp://cli-agent;{\"v\":1,\"agent\":\"claude\",\"event\":\"session_start\",\"session_id\":\"c3a5583d-3934-47a5-9b21-0549acae939e\",\"cwd\":\"/proj\",\"project\":\"proj\",\"plugin_version\":\"2.1.0\"}\x07";

const PROMPT_SUBMIT: &str = "\x1b]777;notify;warp://cli-agent;{\"v\":1,\"agent\":\"claude\",\"event\":\"prompt_submit\",\"session_id\":\"c3a5583d-3934-47a5-9b21-0549acae939e\",\"cwd\":\"/proj\",\"project\":\"proj\",\"query\":\"why does the auth test time out on CI\"}\x07";

/// Also captured: the title sequence the TUI interleaves with its notifications.
/// Included because it has to be ignored, and a scanner that mis-frames it would
/// swallow the notification that follows.
const TITLE: &str = "\x1b]0;\u{2733} Claude Code\x07";

fn stop_with(transcript: &str) -> String {
    format!(
        "\x1b]777;notify;warp://cli-agent;{{\"v\":1,\"agent\":\"claude\",\"event\":\"stop\",\"session_id\":\"c3a5583d-3934-47a5-9b21-0549acae939e\",\"cwd\":\"/proj\",\"project\":\"proj\",\"query\":\"\",\"response\":\"\",\"transcript_path\":\"{transcript}\"}}\x07"
    )
}

/// Everything a pane needs, wired as the app wires it.
struct Pane {
    builder: BlockBuilder,
    scanner: OscScanner,
    pane_id: PaneId,
}

impl Pane {
    fn new() -> Self {
        let pane_id = PaneId::from_external("pane_1");
        Self {
            builder: {
                let mut builder = BlockBuilder::new(
                    pane_id.clone(),
                    SessionId::new(),
                    "/proj".to_string(),
                    std::env::temp_dir(),
                );
                builder.set_project(Some("proj".to_string()));
                builder
            },
            scanner: OscScanner::new(),
            pane_id,
        }
    }

    /// Feed bytes exactly as the PTY pump does, and return what the builder emitted.
    fn feed(&mut self, bytes: &[u8]) -> Vec<BlockEvent> {
        let signals = self
            .scanner
            .feed_indexed(bytes)
            .into_iter()
            .filter_map(|hit| {
                terminal_core::signals::parse(&hit.payload).map(|signal| PositionedSignal {
                    start: hit.start_offset,
                    end: hit.end_offset,
                    signal,
                })
            })
            .collect();
        self.builder.consume(&PtyChunk {
            pane_id: self.pane_id.clone(),
            bytes: bytes.to_vec(),
            signals,
            pending_marker: self.scanner.pending_marker(),
            mode_changes: self.scanner.mode_changes().to_vec(),
            alternate_screen: false,
        })
    }
}

/// Run a whole session through the chain and return the store it filled.
fn run(chunks: &[&[u8]]) -> (Store, PaneAgents, Vec<String>) {
    let store = Store::open_in_memory().unwrap();
    let agents = PaneAgents::new();
    let mut pane = Pane::new();
    let mut threads: Vec<String> = Vec::new();

    for chunk in chunks {
        for event in pane.feed(chunk) {
            if let BlockEvent::AgentActivity { pane_id, activity } = event {
                let observation = agents.observe(&activity, &pane_id, &store);
                if let Some(thread) = &observation.thread {
                    store.upsert_thread(thread).unwrap();
                    threads.push(thread.id.as_str().to_string());
                }
                for event in &observation.events {
                    store.append_event(event, None).unwrap();
                }
            }
        }
    }
    // Deduplicated: `Observation.thread` is returned whenever a Thread is created *or*
    // changed, and the first prompt legitimately renames it. What matters is how many
    // distinct Threads a session produced.
    threads.sort();
    threads.dedup();
    (store, agents, threads)
}

#[test]
fn a_session_run_in_a_pane_becomes_a_thread_and_its_prompt_is_searchable() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("session.jsonl");
    // The transcript as the agent leaves it after answering: the prompt it already
    // announced over OSC, plus the reply and the edit it made.
    std::fs::write(
        &transcript,
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"why does the auth test time out on CI"},"timestamp":"2026-08-02T15:29:00.000Z"}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude-opus-5","content":[{"type":"thinking","thinking":"probably the fixture"},{"type":"text","text":"The fixture waits on a socket that CI never opens."},{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/proj/tests/auth.rs","old_string":"a","new_string":"b"}}]},"timestamp":"2026-08-02T15:29:04.000Z"}"#,
            "\n",
        ),
    )
    .unwrap();

    let stop = stop_with(&transcript.display().to_string());
    let (store, agents, threads) = run(&[
        SESSION_START.as_bytes(),
        TITLE.as_bytes(),
        PROMPT_SUBMIT.as_bytes(),
        stop.as_bytes(),
    ]);

    // One Thread, not one per notification.
    assert_eq!(
        threads.len(),
        1,
        "expected exactly one Thread for the session, got {threads:?}"
    );
    let thread = store
        .thread_by_resume_id("c3a5583d-3934-47a5-9b21-0549acae939e")
        .unwrap()
        .expect("the session should be findable by its own id");

    // Named after what was asked, so the Threads list is readable.
    assert_eq!(thread.task_title, "why does the auth test time out on CI");
    // Pinned to the pane, which is what "Show the pane" needs and what marks the
    // Thread read-only in the UI.
    assert_eq!(thread.pane_id, Some(PaneId::from_external("pane_1")));
    assert_eq!(thread.agent.display_name, "Claude Code");

    // The point of the whole feature: a prompt typed into a bare terminal pane is now
    // in prompt history. This is the thing that had no answer anywhere before.
    let hits = store.search_prompts("auth test time out", 10).unwrap();
    assert_eq!(hits.len(), 1, "the pane prompt was not indexed");
    assert!(hits[0].text.contains("why does the auth test time out"));

    // The reply is searchable too, and came from the transcript rather than the
    // notification — which sends `response` empty.
    let replies = store
        .search_prompts("fixture waits on a socket", 10)
        .unwrap();
    assert_eq!(replies.len(), 1, "the agent's reply was not indexed");

    // Reasoning is excluded from search on purpose: it is long, model-specific, and
    // would bury what the person wrote under the model's thinking about it.
    assert!(
        store
            .search_prompts("probably the fixture", 10)
            .unwrap()
            .is_empty(),
        "reasoning leaked into prompt search"
    );

    // And the timeline holds the whole story, including the edit.
    let events = store.thread_events(&thread.id, 100).unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.payload.kind()).collect();
    assert!(kinds.contains(&"thread.started"), "{kinds:?}");
    assert!(kinds.contains(&"user.prompted"), "{kinds:?}");
    assert!(kinds.contains(&"agent.message"), "{kinds:?}");
    // Review and the Deck key off file.changed, so an edit made in a pane reaches
    // them the same way one Tervin drove would.
    assert!(kinds.contains(&"file.changed"), "{kinds:?}");
    assert!(kinds.contains(&"tool.requested"), "{kinds:?}");

    // Exactly one prompt row: the notification and the transcript both carry it, and
    // recording both would double every row in prompt history.
    assert_eq!(
        kinds.iter().filter(|k| **k == "user.prompted").count(),
        1,
        "the prompt was recorded twice"
    );

    assert_eq!(
        agents.thread_for("c3a5583d-3934-47a5-9b21-0549acae939e"),
        Some(thread.id)
    );
}

#[test]
fn a_notification_split_across_two_pty_reads_is_still_recognised() {
    // A PTY read can end anywhere, including inside the JSON body. The scanner is
    // stateful for exactly this, and a chain that reassembled it wrongly would drop
    // sessions intermittently — the worst kind of bug to chase.
    let cut = SESSION_START.len() / 2;
    let (head, tail) = SESSION_START.as_bytes().split_at(cut);

    let (store, _, threads) = run(&[head, tail]);
    assert_eq!(threads.len(), 1, "a split notification was lost");
    assert!(store
        .thread_by_resume_id("c3a5583d-3934-47a5-9b21-0549acae939e")
        .unwrap()
        .is_some());
}

#[test]
fn an_agent_in_a_pane_does_not_produce_blocks() {
    // An agent's TUI is one long-lived process. If its notifications moved Block
    // state, a session would be chopped into fragments while it was still running,
    // and the Blocks list would fill with rows that are not commands.
    let mut pane = Pane::new();
    let events = pane.feed(SESSION_START.as_bytes());

    assert!(
        events
            .iter()
            .all(|e| matches!(e, BlockEvent::AgentActivity { .. })),
        "agent activity moved Block state"
    );
}

#[test]
fn ordinary_shell_activity_still_produces_blocks_alongside_an_agent() {
    // The regression that matters most: adding OSC 777 handling must not disturb the
    // OSC 133 path that Blocks depend on.
    use base64::Engine as _;
    let mut pane = Pane::new();
    pane.feed(SESSION_START.as_bytes());

    let cmd = base64::engine::general_purpose::STANDARD.encode("cargo test");
    let events = pane
        .feed(format!("\x1b]7373;cmd={cmd}\x07\x1b]133;C\x07ok\r\n\x1b]133;D;0\x07").as_bytes());

    let finished = events.iter().find_map(|e| match e {
        BlockEvent::Finished(block) => Some(block),
        _ => None,
    });
    let block = finished.expect("a command in the same pane should still build a Block");
    assert_eq!(block.command, "cargo test");
    assert_eq!(block.exit_code, Some(0));
}
