//! Tervin Blocks: grouping, metadata, persistence, and search.
//!
//! A Block is one submitted command and everything Tervin learned about it. The
//! crate has three layers that stay separate on purpose:
//!
//! - [`builder`] turns a live byte stream plus shell signals into Blocks.
//! - [`parse`] recovers best-effort structure — paths, ports, diagnostics — from
//!   output, without ever replacing the raw text.
//! - [`store`] persists Blocks locally and makes them searchable.

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

pub mod builder;
pub mod model;
pub mod parse;
pub mod query;
pub mod saved;
pub mod store;

pub use builder::{BlockBuilder, BlockEvent};
pub use model::{
    Block, BlockOutput, BlockStatus, CommandHit, GitContext, ParsedDiagnostic, ParsedOutput,
    PathHit, RecentDir, TestSummary,
};
pub use query::{BlockFilter, BlockSummary, SortOrder};
pub use saved::{Parameter, SavedCommand};
pub use store::{AuditRecord, PromptHit, Store, StoreError};

#[cfg(test)]
mod tests {
    use super::*;
    use tervin_core::{PaneId, SessionId};

    fn sample(command: &str, status: BlockStatus, output: &str) -> Block {
        let mut b = Block::new(
            PaneId::new(),
            SessionId::new(),
            command,
            "/Users/dev/proj",
            "local",
        );
        b.status = status;
        b.exit_code = Some(match status {
            BlockStatus::Succeeded => 0,
            BlockStatus::Failed => 1,
            _ => 0,
        });
        b.output.inline = output.as_bytes().to_vec();
        b.output.total_bytes = output.len() as u64;
        b.parsed = parse::extract(output, "/Users/dev/proj");
        b
    }

    #[test]
    fn round_trips_a_block_through_the_store() {
        let store = Store::open_in_memory().unwrap();
        let mut block = sample("cargo build", BlockStatus::Failed, "error[E0308]: nope\n");
        block.tags = vec!["build".to_string()];
        block.note = Some("investigating".to_string());
        store.upsert_block(&block).unwrap();

        let loaded = store.get_block(&block.id).unwrap().unwrap();
        assert_eq!(loaded.command, "cargo build");
        assert_eq!(loaded.status, BlockStatus::Failed);
        assert_eq!(loaded.tags, vec!["build".to_string()]);
        assert_eq!(loaded.note.as_deref(), Some("investigating"));
        assert_eq!(loaded.parsed.error_count, 1);
    }

    #[test]
    fn finds_blocks_by_output_text() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_block(&sample(
                "npm run dev",
                BlockStatus::Succeeded,
                "ready on 5173",
            ))
            .unwrap();
        store
            .upsert_block(&sample("cargo test", BlockStatus::Succeeded, "all green"))
            .unwrap();

        let hits = store
            .query_blocks(&BlockFilter {
                text: Some("ready".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "npm run dev");
    }

    #[test]
    fn search_input_with_fts_operators_does_not_error() {
        // Users type mid-thought. Quotes and operators must not raise a parse
        // error and break the search box.
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_block(&sample("echo hi", BlockStatus::Succeeded, "hi there"))
            .unwrap();

        for probe in ["\"unclosed", "AND", "a OR", "* *", "NEAR(", ""] {
            let res = store.query_blocks(&BlockFilter {
                text: Some(probe.to_string()),
                ..Default::default()
            });
            assert!(res.is_ok(), "query {probe:?} errored: {:?}", res.err());
        }
    }

    #[test]
    fn search_is_incremental_on_prefixes() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_block(&sample(
                "kubectl get pods",
                BlockStatus::Succeeded,
                "running",
            ))
            .unwrap();
        let hits = store
            .query_blocks(&BlockFilter {
                text: Some("kubec".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            hits.len(),
            1,
            "prefix search should match as the user types"
        );
    }

    #[test]
    fn filters_by_status_and_bookmark() {
        let store = Store::open_in_memory().unwrap();
        let ok = sample("true", BlockStatus::Succeeded, "");
        let mut bad = sample("false", BlockStatus::Failed, "");
        bad.bookmarked = true;
        store.upsert_block(&ok).unwrap();
        store.upsert_block(&bad).unwrap();

        let failures = store.query_blocks(&BlockFilter::failures()).unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].command, "false");

        let marked = store
            .query_blocks(&BlockFilter {
                bookmarked_only: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(marked.len(), 1);
    }

    #[test]
    fn tag_filter_does_not_match_a_longer_tag() {
        // Filtering by `api` must not pull in `api-tests`.
        let store = Store::open_in_memory().unwrap();
        let mut a = sample("one", BlockStatus::Succeeded, "");
        a.tags = vec!["api".to_string()];
        let mut b = sample("two", BlockStatus::Succeeded, "");
        b.tags = vec!["api-tests".to_string()];
        store.upsert_block(&a).unwrap();
        store.upsert_block(&b).unwrap();

        let hits = store
            .query_blocks(&BlockFilter {
                tags: vec!["api".to_string()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "one");
    }

    #[test]
    fn cwd_filter_matches_subdirectories() {
        let store = Store::open_in_memory().unwrap();
        let mut nested = sample("ls", BlockStatus::Succeeded, "");
        nested.cwd = "/Users/dev/proj/src/api".to_string();
        store.upsert_block(&nested).unwrap();

        let hits = store
            .query_blocks(&BlockFilter {
                cwd_prefix: Some("/Users/dev/proj".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn summaries_are_bounded_even_for_huge_output() {
        // A history list must not drag a quarter-megabyte per row across IPC.
        let store = Store::open_in_memory().unwrap();
        let big = "x".repeat(200_000);
        store
            .upsert_block(&sample("cat big.log", BlockStatus::Succeeded, &big))
            .unwrap();

        let hits = store.query_blocks(&BlockFilter::recent(10)).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(
            hits[0].preview.len() <= 2100,
            "preview was {} chars",
            hits[0].preview.len()
        );
        // The true size is still reported, so the UI can say what it is hiding.
        assert_eq!(hits[0].output_total, 200_000);
    }

    #[test]
    fn updating_tags_updates_the_search_index() {
        let store = Store::open_in_memory().unwrap();
        let block = sample("deploy", BlockStatus::Succeeded, "");
        store.upsert_block(&block).unwrap();
        store.set_tags(&block.id, &["release".to_string()]).unwrap();

        let hits = store
            .query_blocks(&BlockFilter {
                text: Some("release".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1, "tag change should be searchable");
        assert_eq!(store.all_tags().unwrap(), vec!["release".to_string()]);
    }

    #[test]
    fn full_output_reads_back_from_the_spill_file() {
        let store = Store::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("tervin-spill-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let spill = dir.join("out.raw");
        let whole = "COMPLETE OUTPUT ON DISK".repeat(100);
        std::fs::write(&spill, &whole).unwrap();

        let mut block = sample("big", BlockStatus::Succeeded, "only the head");
        block.output.spill_path = Some(spill.clone());
        block.output.total_bytes = whole.len() as u64;
        store.upsert_block(&block).unwrap();

        // "The raw terminal output is always available" — including the part
        // that never fit in the row.
        let full = store.read_full_output(&block.id).unwrap();
        assert_eq!(full.len(), whole.len());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn appends_and_reads_a_thread_timeline_in_order() {
        use tervin_core::{AgentIdentity, EventPayload, TervinEvent, ThreadId, Tier};
        let store = Store::open_in_memory().unwrap();
        let thread = ThreadId::new();
        let agent = AgentIdentity::new("claude-code", "Claude Code", Tier::Structured);

        for text in ["first", "second", "third"] {
            let ev = TervinEvent::new(
                agent.clone(),
                text,
                EventPayload::AgentMessage {
                    text: text.to_string(),
                    is_reasoning: false,
                    parent_tool_use_id: None,
                },
            )
            .with_thread(thread.clone());
            store.append_event(&ev, None).unwrap();
        }

        let events = store.thread_events(&thread, 50).unwrap();
        assert_eq!(events.len(), 3);
        // Insertion order, not timestamp order: events inside one millisecond
        // must not shuffle.
        assert_eq!(events[0].summary, "first");
        assert_eq!(events[2].summary, "third");
    }

    #[test]
    fn audit_log_is_append_only_and_ordered_newest_first() {
        let store = Store::open_in_memory().unwrap();
        store
            .append_audit(
                None,
                "tervin",
                "rm -rf build",
                "requested",
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        store
            .append_audit(
                None,
                "user",
                "rm -rf build",
                "decided",
                Some("denied"),
                Some("tervin"),
                Some("once"),
                None,
                None,
            )
            .unwrap();

        let records = store.recent_audit(10).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].phase, "decided");
        assert_eq!(records[0].decision.as_deref(), Some("denied"));
    }

    #[test]
    fn workspaces_round_trip() {
        let store = Store::open_in_memory().unwrap();
        store
            .save_workspace("api", "API service", r#"{"panes":2}"#)
            .unwrap();
        assert_eq!(
            store.load_workspace("api").unwrap().as_deref(),
            Some(r#"{"panes":2}"#)
        );
        assert_eq!(store.list_workspaces().unwrap().len(), 1);
    }
}
