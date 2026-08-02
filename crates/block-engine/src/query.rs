//! Filters and list projections for Blocks.

use crate::model::{BlockStatus, TestSummary};
use serde::{Deserialize, Serialize};
use tervin_core::{BlockId, PaneId, ThreadId, Timestamp};

/// Ordering for a Block list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    #[default]
    NewestFirst,
    OldestFirst,
    LongestFirst,
}

/// Every way the spec allows Blocks to be filtered, in one request object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockFilter {
    /// Free text, matched against command, output, tags, and notes.
    pub text: Option<String>,
    pub project: Option<String>,
    /// Match this directory and everything under it.
    pub cwd_prefix: Option<String>,
    pub host: Option<String>,
    pub thread_id: Option<ThreadId>,
    pub pane_id: Option<PaneId>,
    pub statuses: Vec<BlockStatus>,
    pub tags: Vec<String>,
    pub command_contains: Option<String>,
    pub bookmarked_only: bool,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    pub sort: SortOrder,
    pub limit: usize,
    pub offset: usize,
}

impl Default for BlockFilter {
    fn default() -> Self {
        Self {
            text: None,
            project: None,
            cwd_prefix: None,
            host: None,
            thread_id: None,
            pane_id: None,
            statuses: Vec::new(),
            tags: Vec::new(),
            command_contains: None,
            bookmarked_only: false,
            since: None,
            until: None,
            sort: SortOrder::NewestFirst,
            // A page, not a firehose: the list virtualises and fetches more.
            limit: 100,
            offset: 0,
        }
    }
}

impl BlockFilter {
    pub fn recent(limit: usize) -> Self {
        Self {
            limit,
            ..Default::default()
        }
    }

    pub fn for_thread(thread_id: ThreadId) -> Self {
        Self {
            thread_id: Some(thread_id),
            sort: SortOrder::OldestFirst,
            ..Default::default()
        }
    }

    pub fn failures() -> Self {
        Self {
            statuses: vec![BlockStatus::Failed],
            ..Default::default()
        }
    }
}

/// A Block projected for list rendering.
///
/// Carries a bounded preview instead of full output, so scrolling a long history
/// never pulls megabytes across the IPC boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockSummary {
    pub id: BlockId,
    pub pane_id: PaneId,
    pub thread_id: Option<ThreadId>,
    pub command: String,
    pub cwd: String,
    pub host: String,
    pub project: Option<String>,
    pub started_at: Timestamp,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub status: BlockStatus,
    pub bookmarked: bool,
    pub tags: Vec<String>,
    pub note: Option<String>,
    pub output_total: u64,
    pub output_truncated: bool,
    pub git_branch: Option<String>,
    pub error_count: u32,
    pub warning_count: u32,
    pub tests: Option<TestSummary>,
    pub ports: Vec<u16>,
    /// Leading plain text of the output, for the collapsed row.
    pub preview: String,
}

impl BlockSummary {
    /// Whether the row should carry an attention marker.
    pub fn is_notable(&self) -> bool {
        self.bookmarked || self.status == BlockStatus::Failed || self.error_count > 0
    }
}
