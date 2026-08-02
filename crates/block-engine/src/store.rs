//! The local workspace database.
//!
//! One SQLite file holds Blocks, the agent event stream, Threads, the audit log,
//! and saved workspaces. It is local-first with no remote counterpart: nothing in
//! this module can talk to a network.
//!
//! List queries return [`BlockSummary`] rather than whole Blocks. A Block can
//! carry a quarter-megabyte of inline output, and a history list that loaded that
//! per row would stall on scroll — so the row carries a bounded preview and the
//! full output is fetched only when something expands.

use crate::model::{Block, BlockOutput, BlockStatus, GitContext, ParsedOutput};
use crate::query::{BlockFilter, BlockSummary, SortOrder};
use chrono::{DateTime, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use std::path::{Path, PathBuf};
use tervin_core::{BlockId, PaneId, SessionId, ThreadId, Timestamp};

/// Characters of output text kept on a summary row for preview.
const PREVIEW_CHARS: usize = 2000;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialisation error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("block {0} not found")]
    NotFound(BlockId),
}

type Result<T> = std::result::Result<T, StoreError>;

/// The workspace database.
pub struct Store {
    conn: parking_lot::Mutex<Connection>,
}

impl Store {
    /// Open (and migrate) the database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::configure(&conn)?;
        Self::migrate(&conn)?;
        Ok(Self {
            conn: parking_lot::Mutex::new(conn),
        })
    }

    /// An ephemeral database, used by tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure(&conn)?;
        Self::migrate(&conn)?;
        Ok(Self {
            conn: parking_lot::Mutex::new(conn),
        })
    }

    fn configure(conn: &Connection) -> Result<()> {
        // WAL keeps reads from blocking while a Block is being written, which is
        // what stops history queries stalling during heavy output.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(())
    }

    fn migrate(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS blocks (
                id               TEXT PRIMARY KEY,
                pane_id          TEXT NOT NULL,
                session_id       TEXT NOT NULL,
                thread_id        TEXT,
                command          TEXT NOT NULL,
                cwd              TEXT NOT NULL,
                host             TEXT NOT NULL,
                shell            TEXT,
                project          TEXT,
                started_at       TEXT NOT NULL,
                ended_at         TEXT,
                duration_ms      INTEGER,
                exit_code        INTEGER,
                status           TEXT NOT NULL,
                output_inline    BLOB NOT NULL,
                output_spill     TEXT,
                output_total     INTEGER NOT NULL,
                output_truncated INTEGER NOT NULL,
                git_repo         TEXT,
                git_branch       TEXT,
                git_dirty        INTEGER,
                git_head         TEXT,
                parsed           TEXT NOT NULL,
                tags             TEXT NOT NULL,
                note             TEXT,
                bookmarked       INTEGER NOT NULL DEFAULT 0,
                artifacts        TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS blocks_started  ON blocks(started_at DESC);
            CREATE INDEX IF NOT EXISTS blocks_project  ON blocks(project);
            CREATE INDEX IF NOT EXISTS blocks_thread   ON blocks(thread_id);
            CREATE INDEX IF NOT EXISTS blocks_status   ON blocks(status);
            CREATE INDEX IF NOT EXISTS blocks_pane     ON blocks(pane_id);
            CREATE INDEX IF NOT EXISTS blocks_bookmark ON blocks(bookmarked);

            CREATE VIRTUAL TABLE IF NOT EXISTS blocks_fts USING fts5(
                block_id UNINDEXED,
                command,
                output,
                tags,
                note,
                tokenize = 'unicode61'
            );

            CREATE TABLE IF NOT EXISTS events (
                id          TEXT PRIMARY KEY,
                thread_id   TEXT,
                ts          TEXT NOT NULL,
                kind        TEXT NOT NULL,
                runtime_id  TEXT NOT NULL,
                summary     TEXT NOT NULL,
                project     TEXT,
                cwd         TEXT,
                payload     TEXT NOT NULL,
                links       TEXT NOT NULL,
                raw_pointer TEXT
            );
            -- Indexed on thread_id only: `rowid` cannot appear in an index, and
            -- entries under one key are already stored in rowid order, so
            -- `ORDER BY rowid` needs no extra sort.
            CREATE INDEX IF NOT EXISTS events_thread ON events(thread_id);
            CREATE INDEX IF NOT EXISTS events_kind   ON events(kind);
            -- Retention prunes by age, so the timestamp needs an index of its own.
            CREATE INDEX IF NOT EXISTS events_ts     ON events(ts);

            -- Full-text search over what was said, not over the whole event stream.
            --
            -- Only prompts and agent replies are indexed. A `tool.requested` summary or
            -- a state transition would flood every result with rows nobody is looking
            -- for, and the point of this index is a specific question: "what did I ask
            -- an agent about this, and what did it say?" A shell keeps command history;
            -- nothing keeps that.
            CREATE VIRTUAL TABLE IF NOT EXISTS prompts_fts USING fts5(
                event_id UNINDEXED,
                thread_id UNINDEXED,
                kind UNINDEXED,
                text,
                tokenize = "unicode61 remove_diacritics 2"
            );

            CREATE TABLE IF NOT EXISTS raw_payloads (
                pointer   TEXT PRIMARY KEY,
                kind      TEXT NOT NULL,
                body      TEXT NOT NULL,
                redacted  INTEGER NOT NULL,
                byte_len  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS threads (
                id         TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                state      TEXT NOT NULL,
                json       TEXT NOT NULL,
                -- Denormalised out of `json` so a session can be matched back to
                -- its Thread with an index rather than by scanning every row.
                resume_id  TEXT
            );

            CREATE TABLE IF NOT EXISTS audit (
                id        TEXT PRIMARY KEY,
                ts        TEXT NOT NULL,
                thread_id TEXT,
                actor     TEXT NOT NULL,
                action    TEXT NOT NULL,
                phase     TEXT NOT NULL,
                decision  TEXT,
                authority TEXT,
                scope     TEXT,
                risk      TEXT,
                detail    TEXT
            );
            CREATE INDEX IF NOT EXISTS audit_ts ON audit(ts DESC);

            CREATE TABLE IF NOT EXISTS workspaces (
                id         TEXT PRIMARY KEY,
                name       TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                json       TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS kv (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;

        Self::add_missing_columns(conn)?;
        Ok(())
    }

    /// Add columns that were introduced after a database was first created.
    ///
    /// `CREATE TABLE IF NOT EXISTS` above does nothing to a table that already
    /// exists, so a new column never reaches an installed database — the schema
    /// would be right on a fresh machine and wrong on every upgrade. Adding it here
    /// keeps both cases in step without a version counter to get out of sync.
    fn add_missing_columns(conn: &Connection) -> Result<()> {
        // (table, column, definition)
        const ADDED: &[(&str, &str, &str)] = &[("threads", "resume_id", "TEXT")];

        for (table, column, definition) in ADDED {
            let exists: bool = conn
                .prepare(&format!("SELECT * FROM pragma_table_info('{table}')"))?
                .query_map([], |r| r.get::<_, String>("name"))?
                .filter_map(std::result::Result::ok)
                .any(|name| name == *column);
            if !exists {
                conn.execute_batch(&format!(
                    "ALTER TABLE {table} ADD COLUMN {column} {definition};"
                ))?;
            }
        }

        // Only now that the column is guaranteed to exist. Creating this index in the
        // batch above would fail on an upgrade — the table is not recreated, so the
        // column is not there yet, and `Store::open` would error before the ALTER ran.
        conn.execute_batch("CREATE INDEX IF NOT EXISTS threads_resume ON threads(resume_id);")?;

        // Backfill from the JSON the column was denormalised out of, so an upgrade
        // can match existing Threads rather than starting duplicates for sessions
        // that are already recorded.
        conn.execute_batch(
            "UPDATE threads
                SET resume_id = json_extract(json, '$.resume_id')
              WHERE resume_id IS NULL
                AND json_extract(json, '$.resume_id') IS NOT NULL;",
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------- blocks

    /// Insert or replace a Block, keeping the search index in step.
    pub fn upsert_block(&self, block: &Block) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        tx.execute(
            r#"
            INSERT INTO blocks (
                id, pane_id, session_id, thread_id, command, cwd, host, shell, project,
                started_at, ended_at, duration_ms, exit_code, status,
                output_inline, output_spill, output_total, output_truncated,
                git_repo, git_branch, git_dirty, git_head,
                parsed, tags, note, bookmarked, artifacts
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                ?10, ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18,
                ?19, ?20, ?21, ?22,
                ?23, ?24, ?25, ?26, ?27
            )
            ON CONFLICT(id) DO UPDATE SET
                thread_id = excluded.thread_id,
                command = excluded.command,
                ended_at = excluded.ended_at,
                duration_ms = excluded.duration_ms,
                exit_code = excluded.exit_code,
                status = excluded.status,
                output_inline = excluded.output_inline,
                output_spill = excluded.output_spill,
                output_total = excluded.output_total,
                output_truncated = excluded.output_truncated,
                git_repo = excluded.git_repo,
                git_branch = excluded.git_branch,
                git_dirty = excluded.git_dirty,
                git_head = excluded.git_head,
                parsed = excluded.parsed,
                tags = excluded.tags,
                note = excluded.note,
                bookmarked = excluded.bookmarked,
                artifacts = excluded.artifacts
            "#,
            params![
                block.id.as_str(),
                block.pane_id.as_str(),
                block.session_id.as_str(),
                block.thread_id.as_ref().map(|t| t.as_str()),
                block.command,
                block.cwd,
                block.host,
                block.shell,
                block.project,
                rfc3339(&block.started_at),
                block.ended_at.map(|t| rfc3339(&t)),
                block.duration_ms.map(|d| d as i64),
                block.exit_code,
                status_str(block.status),
                block.output.inline,
                block
                    .output
                    .spill_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string()),
                block.output.total_bytes as i64,
                block.output.truncated as i32,
                block.git.repo_root,
                block.git.branch,
                block.git.dirty.map(|d| d as i32),
                block.git.head_sha,
                serde_json::to_string(&block.parsed)?,
                serde_json::to_string(&block.tags)?,
                block.note,
                block.bookmarked as i32,
                serde_json::to_string(&block.artifacts)?,
            ],
        )?;

        // FTS5 has no upsert, so replace the row outright.
        tx.execute(
            "DELETE FROM blocks_fts WHERE block_id = ?1",
            params![block.id.as_str()],
        )?;
        tx.execute(
            "INSERT INTO blocks_fts (block_id, command, output, tags, note) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                block.id.as_str(),
                block.command,
                crate::parse::strip_ansi(&block.output.inline_text()),
                block.tags.join(" "),
                block.note.clone().unwrap_or_default(),
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    pub fn get_block(&self, id: &BlockId) -> Result<Option<Block>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT * FROM blocks WHERE id = ?1",
            params![id.as_str()],
            row_to_block,
        )
        .optional()
        .map_err(StoreError::from)
    }

    /// Full raw output, reading the spill file when the Block has one.
    ///
    /// This is what backs "the raw terminal output is always available".
    pub fn read_full_output(&self, id: &BlockId) -> Result<Vec<u8>> {
        let block = self
            .get_block(id)?
            .ok_or_else(|| StoreError::NotFound(id.clone()))?;
        match &block.output.spill_path {
            Some(path) if path.exists() => Ok(std::fs::read(path)?),
            _ => Ok(block.output.inline),
        }
    }

    /// Query Blocks, returning bounded summaries.
    pub fn query_blocks(&self, filter: &BlockFilter) -> Result<Vec<BlockSummary>> {
        let conn = self.conn.lock();

        let mut wheres: Vec<String> = Vec::new();
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        // Full-text terms go through FTS5; everything else is plain SQL so the
        // indexes apply.
        if let Some(text) = filter.text.as_ref().filter(|t| !t.trim().is_empty()) {
            wheres.push(
                "b.id IN (SELECT block_id FROM blocks_fts WHERE blocks_fts MATCH ?)".to_string(),
            );
            binds.push(Box::new(fts_query(text)));
        }
        if let Some(p) = &filter.project {
            wheres.push("b.project = ?".to_string());
            binds.push(Box::new(p.clone()));
        }
        if let Some(c) = &filter.cwd_prefix {
            wheres.push("b.cwd LIKE ? ESCAPE '\\'".to_string());
            binds.push(Box::new(format!("{}%", escape_like(c))));
        }
        if let Some(h) = &filter.host {
            wheres.push("b.host = ?".to_string());
            binds.push(Box::new(h.clone()));
        }
        if let Some(t) = &filter.thread_id {
            wheres.push("b.thread_id = ?".to_string());
            binds.push(Box::new(t.as_str().to_string()));
        }
        if let Some(p) = &filter.pane_id {
            wheres.push("b.pane_id = ?".to_string());
            binds.push(Box::new(p.as_str().to_string()));
        }
        if !filter.statuses.is_empty() {
            let holes = vec!["?"; filter.statuses.len()].join(", ");
            wheres.push(format!("b.status IN ({holes})"));
            for s in &filter.statuses {
                binds.push(Box::new(status_str(*s).to_string()));
            }
        }
        if filter.bookmarked_only {
            wheres.push("b.bookmarked = 1".to_string());
        }
        for tag in &filter.tags {
            // Tags are a JSON array; match the quoted element to avoid `api`
            // matching `api-tests`.
            wheres.push("b.tags LIKE ? ESCAPE '\\'".to_string());
            binds.push(Box::new(format!("%\"{}\"%", escape_like(tag))));
        }
        if let Some(since) = &filter.since {
            wheres.push("b.started_at >= ?".to_string());
            binds.push(Box::new(rfc3339(since)));
        }
        if let Some(until) = &filter.until {
            wheres.push("b.started_at <= ?".to_string());
            binds.push(Box::new(rfc3339(until)));
        }
        if let Some(cmd) = &filter.command_contains {
            wheres.push("b.command LIKE ? ESCAPE '\\'".to_string());
            binds.push(Box::new(format!("%{}%", escape_like(cmd))));
        }

        let where_clause = if wheres.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", wheres.join(" AND "))
        };

        let order = match filter.sort {
            SortOrder::NewestFirst => "b.started_at DESC, b.rowid DESC",
            SortOrder::OldestFirst => "b.started_at ASC, b.rowid ASC",
            SortOrder::LongestFirst => "b.duration_ms DESC NULLS LAST",
        };

        // Slice the preview in SQL so a 256 KB blob never crosses into Rust for a
        // row the user is only scrolling past.
        let sql = format!(
            r#"
            SELECT b.id, b.pane_id, b.thread_id, b.command, b.cwd, b.host, b.project,
                   b.started_at, b.duration_ms, b.exit_code, b.status,
                   b.bookmarked, b.tags, b.note, b.output_total, b.output_truncated,
                   b.git_branch, b.parsed,
                   substr(CAST(b.output_inline AS TEXT), 1, {PREVIEW_CHARS}) AS preview
            FROM blocks b
            {where_clause}
            ORDER BY {order}
            LIMIT ? OFFSET ?
            "#
        );

        binds.push(Box::new(filter.limit as i64));
        binds.push(Box::new(filter.offset as i64));

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params_from_iter(binds.iter().map(|b| b.as_ref())),
            row_to_summary,
        )?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn count_blocks(&self) -> Result<u64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM blocks", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    pub fn set_bookmark(&self, id: &BlockId, bookmarked: bool) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE blocks SET bookmarked = ?2 WHERE id = ?1",
            params![id.as_str(), bookmarked as i32],
        )?;
        Ok(())
    }

    pub fn set_tags(&self, id: &BlockId, tags: &[String]) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE blocks SET tags = ?2 WHERE id = ?1",
            params![id.as_str(), serde_json::to_string(tags)?],
        )?;
        tx.execute(
            "UPDATE blocks_fts SET tags = ?2 WHERE block_id = ?1",
            params![id.as_str(), tags.join(" ")],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_note(&self, id: &BlockId, note: Option<&str>) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE blocks SET note = ?2 WHERE id = ?1",
            params![id.as_str(), note],
        )?;
        tx.execute(
            "UPDATE blocks_fts SET note = ?2 WHERE block_id = ?1",
            params![id.as_str(), note.unwrap_or("")],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Distinct tags in use, for filter chips and autocomplete.
    pub fn all_tags(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT DISTINCT tags FROM blocks WHERE tags != '[]'")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut set = std::collections::BTreeSet::new();
        for r in rows {
            if let Ok(list) = serde_json::from_str::<Vec<String>>(&r?) {
                set.extend(list);
            }
        }
        Ok(set.into_iter().collect())
    }

    // ---------------------------------------------------------------- events

    /// Append an event, storing its raw payload separately.
    pub fn append_event(
        &self,
        event: &tervin_core::TervinEvent,
        raw_body: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        if let (Some(raw), Some(body)) = (&event.raw, raw_body) {
            tx.execute(
                "INSERT OR REPLACE INTO raw_payloads (pointer, kind, body, redacted, byte_len) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![raw.pointer, raw.kind, body, raw.redacted as i32, raw.byte_len as i64],
            )?;
        }

        tx.execute(
            r#"INSERT OR REPLACE INTO events
               (id, thread_id, ts, kind, runtime_id, summary, project, cwd, payload, links, raw_pointer)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
            params![
                event.id.as_str(),
                event.thread_id.as_ref().map(|t| t.as_str()),
                rfc3339(&event.ts),
                event.kind(),
                event.agent.runtime_id,
                event.summary,
                event.project,
                event.cwd,
                serde_json::to_string(&event.payload)?,
                serde_json::to_string(&event.links)?,
                event.raw.as_ref().map(|r| r.pointer.clone()),
            ],
        )?;

        // Index what was actually said, so it can be found later.
        if let Some(text) = searchable_text(&event.payload) {
            tx.execute(
                "INSERT INTO prompts_fts (event_id, thread_id, kind, text) VALUES (?1, ?2, ?3, ?4)",
                params![
                    event.id.as_str(),
                    event.thread_id.as_ref().map(|t| t.as_str()),
                    event.kind(),
                    text,
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Search prompts and agent replies.
    ///
    /// The question this answers is one nothing else can: a shell keeps command
    /// history, and no agent keeps a searchable record of what you asked it. Reasoning
    /// passages are excluded — see [`searchable_text`].
    pub fn search_prompts(&self, query: &str, limit: usize) -> Result<Vec<PromptHit>> {
        // Tested on the *input*: `fts_query` deliberately returns a query that matches
        // nothing for empty input, which is right for Blocks and wrong here.
        if query.trim().is_empty() {
            // An empty query means "the most recent", not "everything ever".
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                r#"SELECT f.event_id, f.thread_id, f.kind, f.text, e.ts, e.runtime_id, e.project
                   FROM prompts_fts f JOIN events e ON e.id = f.event_id
                   ORDER BY e.ts DESC LIMIT ?1"#,
            )?;
            let rows = stmt.query_map(params![limit as i64], prompt_hit)?;
            return Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        }

        let sanitised = fts_query(query);
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT f.event_id, f.thread_id, f.kind, f.text, e.ts, e.runtime_id, e.project
               FROM prompts_fts f JOIN events e ON e.id = f.event_id
               WHERE prompts_fts MATCH ?1
               ORDER BY e.ts DESC LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![sanitised, limit as i64], prompt_hit)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Delete events, raw payloads, and prompt text older than `days`.
    ///
    /// Returns how many events went. Blocks are deliberately **not** pruned: a command
    /// and its output are small and are the thing people search for years later, while
    /// an agent transcript is large and stops being useful quickly. Treating them the
    /// same would either throw away the valuable half or keep the expensive one forever.
    pub fn prune_events(&self, days: u32) -> Result<usize> {
        let cutoff = tervin_core::now() - chrono::Duration::days(days as i64);
        let cutoff = rfc3339(&cutoff);

        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        // Raw payloads and the search index first: both are reached through `events`,
        // so deleting that first would orphan them permanently.
        tx.execute(
            "DELETE FROM raw_payloads WHERE pointer IN              (SELECT raw_pointer FROM events WHERE ts < ?1 AND raw_pointer IS NOT NULL)",
            params![cutoff],
        )?;
        tx.execute(
            "DELETE FROM prompts_fts WHERE event_id IN (SELECT id FROM events WHERE ts < ?1)",
            params![cutoff],
        )?;
        let removed = tx.execute("DELETE FROM events WHERE ts < ?1", params![cutoff])?;

        // A Thread with no remaining events is a title and nothing else.
        tx.execute(
            "DELETE FROM threads WHERE id NOT IN              (SELECT DISTINCT thread_id FROM events WHERE thread_id IS NOT NULL)",
            [],
        )?;

        tx.commit()?;
        Ok(removed)
    }

    /// A Thread's timeline in insertion order.
    pub fn thread_events(
        &self,
        thread_id: &ThreadId,
        limit: usize,
    ) -> Result<Vec<tervin_core::TervinEvent>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT id, thread_id, ts, runtime_id, summary, project, cwd, payload, links, raw_pointer
               FROM events WHERE thread_id = ?1 ORDER BY rowid ASC LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![thread_id.as_str(), limit as i64], |row| {
            Ok(row_to_event(row))
        })?;
        let mut out = Vec::new();
        for r in rows {
            if let Some(ev) = r? {
                out.push(ev);
            }
        }
        Ok(out)
    }

    /// The stored raw payload behind an event, for the "show raw" affordance.
    pub fn raw_payload(&self, pointer: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT body FROM raw_payloads WHERE pointer = ?1",
            params![pointer],
            |r| r.get(0),
        )
        .optional()
        .map_err(StoreError::from)
    }

    // --------------------------------------------------------------- threads

    pub fn upsert_thread(&self, thread: &tervin_core::thread::Thread) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO threads (id, updated_at, state, json, resume_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                thread.id.as_str(),
                rfc3339(&thread.updated_at),
                serde_json::to_string(&thread.state)?,
                serde_json::to_string(thread)?,
                thread.resume_id.as_deref(),
            ],
        )?;
        Ok(())
    }

    /// Find a Thread by the handle its runtime uses to resume it.
    ///
    /// This is what stops a restart of Tervin, or `claude --resume`, from creating a
    /// second Thread for a conversation that already has one.
    pub fn thread_by_resume_id(
        &self,
        resume_id: &str,
    ) -> Result<Option<tervin_core::thread::Thread>> {
        if resume_id.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock();
        let json: Option<String> = conn
            .query_row(
                // Newest wins: an id could in principle have been reused.
                "SELECT json FROM threads WHERE resume_id = ?1 ORDER BY updated_at DESC LIMIT 1",
                params![resume_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    pub fn list_threads(&self, limit: usize) -> Result<Vec<tervin_core::thread::Thread>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT json FROM threads ORDER BY updated_at DESC LIMIT ?1")?;
        let rows = stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(t) = serde_json::from_str(&r?) {
                out.push(t);
            }
        }
        Ok(out)
    }

    pub fn get_thread(&self, id: &ThreadId) -> Result<Option<tervin_core::thread::Thread>> {
        let conn = self.conn.lock();
        let json: Option<String> = conn
            .query_row(
                "SELECT json FROM threads WHERE id = ?1",
                params![id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    // ----------------------------------------------------------------- audit

    /// Append an audit record. Append-only by contract: nothing updates or
    /// deletes from this table.
    #[allow(clippy::too_many_arguments)]
    pub fn append_audit(
        &self,
        thread_id: Option<&ThreadId>,
        actor: &str,
        action: &str,
        phase: &str,
        decision: Option<&str>,
        authority: Option<&str>,
        scope: Option<&str>,
        risk_json: Option<&str>,
        detail: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"INSERT INTO audit (id, ts, thread_id, actor, action, phase, decision, authority, scope, risk, detail)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
            params![
                uuid::Uuid::new_v4().to_string(),
                rfc3339(&tervin_core::now()),
                thread_id.map(|t| t.as_str()),
                actor,
                action,
                phase,
                decision,
                authority,
                scope,
                risk_json,
                detail,
            ],
        )?;
        Ok(())
    }

    /// Recent audit records, newest first.
    pub fn recent_audit(&self, limit: usize) -> Result<Vec<AuditRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT id, ts, thread_id, actor, action, phase, decision, authority, scope, risk, detail
               FROM audit ORDER BY ts DESC, rowid DESC LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(AuditRecord {
                id: r.get(0)?,
                ts: r.get(1)?,
                thread_id: r.get(2)?,
                actor: r.get(3)?,
                action: r.get(4)?,
                phase: r.get(5)?,
                decision: r.get(6)?,
                authority: r.get(7)?,
                scope: r.get(8)?,
                risk: r.get(9)?,
                detail: r.get(10)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // ------------------------------------------------------------ workspaces

    pub fn save_workspace(&self, id: &str, name: &str, json: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO workspaces (id, name, updated_at, json) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, rfc3339(&tervin_core::now()), json],
        )?;
        Ok(())
    }

    pub fn load_workspace(&self, id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT json FROM workspaces WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(StoreError::from)
    }

    pub fn list_workspaces(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, name FROM workspaces ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO kv (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        conn.query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| {
            r.get(0)
        })
        .optional()
        .map_err(StoreError::from)
    }
}

/// One row of the audit log.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuditRecord {
    pub id: String,
    pub ts: String,
    pub thread_id: Option<String>,
    pub actor: String,
    pub action: String,
    pub phase: String,
    pub decision: Option<String>,
    pub authority: Option<String>,
    pub scope: Option<String>,
    pub risk: Option<String>,
    pub detail: Option<String>,
}

// ------------------------------------------------------------------ helpers

fn rfc3339(ts: &Timestamp) -> String {
    ts.to_rfc3339()
}

fn parse_ts(s: &str) -> Timestamp {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn status_str(s: BlockStatus) -> &'static str {
    match s {
        BlockStatus::Running => "running",
        BlockStatus::Succeeded => "succeeded",
        BlockStatus::Failed => "failed",
        BlockStatus::Interrupted => "interrupted",
        BlockStatus::Unknown => "unknown",
    }
}

fn status_from(s: &str) -> BlockStatus {
    match s {
        "running" => BlockStatus::Running,
        "succeeded" => BlockStatus::Succeeded,
        "failed" => BlockStatus::Failed,
        "interrupted" => BlockStatus::Interrupted,
        _ => BlockStatus::Unknown,
    }
}

/// Escape `LIKE` wildcards so a user searching for `_` or `%` gets a literal match.
fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Turn user input into a safe FTS5 query.
///
/// FTS5's query syntax would otherwise let stray quotes or operators raise a
/// parse error mid-typing. Each term is quoted and given a prefix wildcard so
/// search feels incremental.
/// One prompt or reply found by search.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PromptHit {
    pub event_id: String,
    pub thread_id: Option<String>,
    /// `user.prompted` or `agent.message`.
    pub kind: String,
    pub text: String,
    pub ts: String,
    pub runtime_id: String,
    pub project: Option<String>,
}

fn prompt_hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<PromptHit> {
    Ok(PromptHit {
        event_id: row.get(0)?,
        thread_id: row.get(1)?,
        kind: row.get(2)?,
        text: row.get(3)?,
        ts: row.get(4)?,
        runtime_id: row.get(5)?,
        project: row.get(6)?,
    })
}

/// The text worth indexing from an event, if any.
///
/// Prompts and non-reasoning replies only. Reasoning is excluded because it is long,
/// model-specific, and would swamp a search for something the user actually wrote —
/// the same reason a Context Bundle leaves it out.
fn searchable_text(payload: &tervin_core::EventPayload) -> Option<String> {
    match payload {
        tervin_core::EventPayload::UserPrompted { text } => Some(text.clone()),
        tervin_core::EventPayload::AgentMessage {
            text,
            is_reasoning: false,
            ..
        } => Some(text.clone()),
        _ => None,
    }
}

fn fts_query(input: &str) -> String {
    let terms: Vec<String> = input
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| {
            let cleaned = t.replace('"', " ");
            format!("\"{}\"*", cleaned.trim())
        })
        .filter(|t| t != "\"\"*")
        .collect();
    if terms.is_empty() {
        // Matches nothing rather than erroring on an empty query.
        return "\"\"".to_string();
    }
    terms.join(" AND ")
}

fn row_to_block(row: &Row<'_>) -> rusqlite::Result<Block> {
    let parsed: String = row.get("parsed")?;
    let tags: String = row.get("tags")?;
    let artifacts: String = row.get("artifacts")?;
    let spill: Option<String> = row.get("output_spill")?;

    Ok(Block {
        id: BlockId::from_external(row.get::<_, String>("id")?),
        pane_id: PaneId::from_external(row.get::<_, String>("pane_id")?),
        session_id: SessionId::from_external(row.get::<_, String>("session_id")?),
        thread_id: row
            .get::<_, Option<String>>("thread_id")?
            .map(ThreadId::from_external),
        command: row.get("command")?,
        cwd: row.get("cwd")?,
        host: row.get("host")?,
        shell: row.get("shell")?,
        project: row.get("project")?,
        started_at: parse_ts(&row.get::<_, String>("started_at")?),
        ended_at: row
            .get::<_, Option<String>>("ended_at")?
            .map(|s| parse_ts(&s)),
        duration_ms: row.get::<_, Option<i64>>("duration_ms")?.map(|d| d as u64),
        exit_code: row.get("exit_code")?,
        status: status_from(&row.get::<_, String>("status")?),
        output: BlockOutput {
            inline: row.get("output_inline")?,
            spill_path: spill.map(PathBuf::from),
            total_bytes: row.get::<_, i64>("output_total")? as u64,
            truncated: row.get::<_, i32>("output_truncated")? != 0,
        },
        git: GitContext {
            repo_root: row.get("git_repo")?,
            branch: row.get("git_branch")?,
            dirty: row.get::<_, Option<i32>>("git_dirty")?.map(|d| d != 0),
            head_sha: row.get("git_head")?,
        },
        parsed: serde_json::from_str(&parsed).unwrap_or_default(),
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        note: row.get("note")?,
        bookmarked: row.get::<_, i32>("bookmarked")? != 0,
        artifacts: serde_json::from_str(&artifacts).unwrap_or_default(),
    })
}

fn row_to_summary(row: &Row<'_>) -> rusqlite::Result<BlockSummary> {
    let parsed: ParsedOutput = serde_json::from_str(&row.get::<_, String>(17)?).unwrap_or_default();
    let tags: Vec<String> = serde_json::from_str(&row.get::<_, String>(12)?).unwrap_or_default();
    // The preview comes back as raw bytes cast to TEXT, so strip escapes here.
    let preview_raw: String = row.get::<_, Option<String>>(18)?.unwrap_or_default();

    Ok(BlockSummary {
        id: BlockId::from_external(row.get::<_, String>(0)?),
        pane_id: PaneId::from_external(row.get::<_, String>(1)?),
        thread_id: row
            .get::<_, Option<String>>(2)?
            .map(ThreadId::from_external),
        command: row.get(3)?,
        cwd: row.get(4)?,
        host: row.get(5)?,
        project: row.get(6)?,
        started_at: parse_ts(&row.get::<_, String>(7)?),
        duration_ms: row.get::<_, Option<i64>>(8)?.map(|d| d as u64),
        exit_code: row.get(9)?,
        status: status_from(&row.get::<_, String>(10)?),
        bookmarked: row.get::<_, i32>(11)? != 0,
        tags,
        note: row.get(13)?,
        output_total: row.get::<_, i64>(14)? as u64,
        output_truncated: row.get::<_, i32>(15)? != 0,
        git_branch: row.get(16)?,
        error_count: parsed.error_count,
        warning_count: parsed.warning_count,
        tests: parsed.tests,
        ports: parsed.ports,
        preview: crate::parse::strip_ansi(&preview_raw),
    })
}

fn row_to_event(row: &Row<'_>) -> Option<tervin_core::TervinEvent> {
    use tervin_core::{events::RawRef, AgentIdentity, EventId, Tier};

    let payload: tervin_core::EventPayload =
        serde_json::from_str(&row.get::<_, String>(7).ok()?).ok()?;
    let links = serde_json::from_str(&row.get::<_, String>(8).ok()?).unwrap_or_default();
    let runtime_id: String = row.get(3).ok()?;

    Some(tervin_core::TervinEvent {
        id: EventId::from_external(row.get::<_, String>(0).ok()?),
        thread_id: row
            .get::<_, Option<String>>(1)
            .ok()?
            .map(ThreadId::from_external),
        ts: parse_ts(&row.get::<_, String>(2).ok()?),
        // The stored row keeps the runtime key; display name and tier are
        // re-resolved from the live registry when rendering.
        agent: AgentIdentity::new(runtime_id.clone(), runtime_id, Tier::Structured),
        project: row.get(5).ok()?,
        cwd: row.get(6).ok()?,
        summary: row.get(4).ok()?,
        raw: row.get::<_, Option<String>>(9).ok()?.map(|pointer| RawRef {
            kind: String::new(),
            pointer,
            byte_len: 0,
            redacted: false,
        }),
        links,
        payload,
    })
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use tervin_core::{AgentIdentity, EventPayload, TervinEvent, Tier};

    fn agent() -> AgentIdentity {
        AgentIdentity::new("claude-code", "Claude Code", Tier::Structured)
    }

    /// An event at a chosen age, so retention can be tested without waiting.
    fn aged(payload: EventPayload, days_ago: i64) -> TervinEvent {
        let mut event = TervinEvent::new(agent(), "summary", payload);
        event.ts = tervin_core::now() - chrono::Duration::days(days_ago);
        event.thread_id = Some(tervin_core::ThreadId::new());
        event
    }

    fn prompt(text: &str, days_ago: i64) -> TervinEvent {
        aged(
            EventPayload::UserPrompted {
                text: text.to_string(),
            },
            days_ago,
        )
    }

    #[test]
    fn prompts_are_searchable_by_their_text() {
        // The gap this closes: a shell keeps command history, and no agent keeps a
        // searchable record of what you asked it.
        let store = Store::open_in_memory().unwrap();
        store
            .append_event(&prompt("fix the flaky auth test in the gateway", 0), None)
            .unwrap();
        store
            .append_event(&prompt("add a retention policy to the store", 0), None)
            .unwrap();

        let hits = store.search_prompts("flaky", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("gateway"));
        assert_eq!(hits[0].kind, "user.prompted");

        // Both, when the query matches both.
        assert_eq!(store.search_prompts("the", 20).unwrap().len(), 2);
    }

    #[test]
    fn an_empty_query_returns_the_most_recent_rather_than_everything() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..5 {
            store
                .append_event(&prompt(&format!("prompt number {i}"), i), None)
                .unwrap();
        }
        let hits = store.search_prompts("", 3).unwrap();
        assert_eq!(hits.len(), 3);
        // Newest first: day 0 is the most recent.
        assert!(hits[0].text.contains("number 0"), "{:?}", hits[0].text);
    }

    #[test]
    fn agent_replies_are_indexed_but_reasoning_is_not() {
        // Reasoning is long, model-specific, and would swamp a search for something
        // the user actually wrote.
        let store = Store::open_in_memory().unwrap();
        store
            .append_event(
                &aged(
                    EventPayload::AgentMessage {
                        text: "The deadlock is in permissions().".into(),
                        is_reasoning: false,
                        parent_tool_use_id: None,
                    },
                    0,
                ),
                None,
            )
            .unwrap();
        store
            .append_event(
                &aged(
                    EventPayload::AgentMessage {
                        text: "Maybe the mutex, maybe not, let me think about deadlock".into(),
                        is_reasoning: true,
                        parent_tool_use_id: None,
                    },
                    0,
                ),
                None,
            )
            .unwrap();

        let hits = store.search_prompts("deadlock", 20).unwrap();
        assert_eq!(hits.len(), 1, "reasoning must not be searchable");
        assert_eq!(hits[0].kind, "agent.message");
    }

    #[test]
    fn a_search_operator_typed_mid_query_does_not_error() {
        // Someone typing `foo(` should get no results, not a database error.
        let store = Store::open_in_memory().unwrap();
        store.append_event(&prompt("some text", 0), None).unwrap();
        for query in ["foo(", "a AND", "\"unclosed", "*", "NEAR("] {
            assert!(
                store.search_prompts(query, 10).is_ok(),
                "query {query:?} errored"
            );
        }
    }

    #[test]
    fn retention_prunes_old_agent_history_and_keeps_recent() {
        let store = Store::open_in_memory().unwrap();
        store
            .append_event(&prompt("ancient question", 90), None)
            .unwrap();
        store
            .append_event(&prompt("recent question", 3), None)
            .unwrap();

        let removed = store.prune_events(30).unwrap();
        assert_eq!(removed, 1);

        let hits = store.search_prompts("question", 20).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("recent"));
    }

    #[test]
    fn pruning_also_removes_the_search_index_and_raw_payloads() {
        // Otherwise the index keeps returning rows whose events are gone, and raw
        // payloads become unreachable garbage that grows forever.
        let store = Store::open_in_memory().unwrap();
        let mut old = prompt("ancient", 90);
        old.raw = Some(tervin_core::events::RawRef {
            kind: "claude-code/stream-json".into(),
            pointer: "ptr-1".into(),
            byte_len: 4,
            redacted: false,
        });
        store.append_event(&old, Some("{\"a\":1}")).unwrap();

        store.prune_events(30).unwrap();
        assert!(store.search_prompts("ancient", 20).unwrap().is_empty());
        // The payload went with it.
        assert!(store.raw_payload("ptr-1").unwrap().is_none());
    }

    #[test]
    fn blocks_survive_pruning_because_they_are_the_valuable_half() {
        // A command and its output are small and stay useful for years; an agent
        // transcript is large and stops being useful quickly. Treating them the same
        // would throw away the wrong one.
        let store = Store::open_in_memory().unwrap();
        let mut block = crate::model::Block::new(
            tervin_core::PaneId::new(),
            tervin_core::SessionId::new(),
            "cargo test",
            "/tmp",
            "local",
        );
        block.started_at = tervin_core::now() - chrono::Duration::days(300);
        store.upsert_block(&block).unwrap();

        store.prune_events(30).unwrap();
        assert!(
            store.get_block(&block.id).unwrap().is_some(),
            "a 300-day-old Block must survive"
        );
    }

    #[test]
    fn pruning_removes_threads_left_with_no_events() {
        let store = Store::open_in_memory().unwrap();
        let event = prompt("ancient", 90);
        let thread_id = event.thread_id.clone().unwrap();
        let mut thread =
            tervin_core::thread::Thread::new(agent(), "/tmp".to_string(), "old task".to_string());
        thread.id = thread_id.clone();
        store.upsert_thread(&thread).unwrap();
        store.append_event(&event, None).unwrap();

        store.prune_events(30).unwrap();
        assert!(
            store.get_thread(&thread_id).unwrap().is_none(),
            "a Thread with no remaining events is a title and nothing else"
        );
    }

    #[test]
    fn pruning_with_nothing_old_enough_removes_nothing() {
        let store = Store::open_in_memory().unwrap();
        store.append_event(&prompt("recent", 1), None).unwrap();
        assert_eq!(store.prune_events(30).unwrap(), 0);
        assert_eq!(store.search_prompts("recent", 10).unwrap().len(), 1);
    }
}

/// Upgrading a database that already exists.
///
/// `CREATE TABLE IF NOT EXISTS` is silent about a table it does not create, so a
/// column added later reaches a fresh install and no installed one. These build a
/// database with the pre-`resume_id` schema and then open it with the current code,
/// which is the only way to catch that.
#[cfg(test)]
mod migration_tests {
    use super::*;

    /// The `threads` table exactly as it shipped before `resume_id`.
    const OLD_SCHEMA: &str = r#"
        CREATE TABLE threads (
            id         TEXT PRIMARY KEY,
            updated_at TEXT NOT NULL,
            state      TEXT NOT NULL,
            json       TEXT NOT NULL
        );
    "#;

    fn thread_json(id: &str, resume: Option<&str>) -> String {
        let mut thread = tervin_core::thread::Thread::new(
            tervin_core::AgentIdentity::new(
                "claude-code",
                "Claude Code",
                tervin_core::Tier::EnhancedCli,
            ),
            "/proj".to_string(),
            "an old thread".to_string(),
        );
        thread.id = ThreadId::from_external(id);
        thread.resume_id = resume.map(String::from);
        serde_json::to_string(&thread).unwrap()
    }

    #[test]
    fn an_existing_database_gains_the_column_and_keeps_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.db");

        // A database as an earlier build left it, holding a Thread with a resume id
        // recorded only inside the JSON.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(OLD_SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO threads (id, updated_at, state, json) VALUES (?1, ?2, ?3, ?4)",
                params![
                    "thr_old",
                    "2026-07-01T00:00:00Z",
                    "\"idle\"",
                    thread_json("thr_old", Some("sess-abc")),
                ],
            )
            .unwrap();
        }

        // Opening it must not fail, and must not lose the row.
        let store = Store::open(&path).unwrap();
        assert_eq!(store.list_threads(10).unwrap().len(), 1);

        // Backfilled from the JSON, so a session already on disk is adopted rather
        // than duplicated on the next notification.
        let found = store.thread_by_resume_id("sess-abc").unwrap();
        assert_eq!(
            found.map(|t| t.id.as_str().to_string()),
            Some("thr_old".to_string()),
            "the resume id was not backfilled out of the existing JSON"
        );
    }

    #[test]
    fn opening_twice_is_harmless() {
        // `ALTER TABLE ADD COLUMN` fails on a column that already exists, so the
        // second open is the one that would break.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.db");
        drop(Store::open(&path).unwrap());
        assert!(
            Store::open(&path).is_ok(),
            "reopening a current database failed"
        );
    }

    #[test]
    fn a_thread_with_no_resume_id_is_not_matched_by_an_empty_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("workspace.db")).unwrap();

        let mut thread = tervin_core::thread::Thread::new(
            tervin_core::AgentIdentity::new(
                "claude-code",
                "Claude Code",
                tervin_core::Tier::Structured,
            ),
            "/proj".to_string(),
            "launched by Tervin".to_string(),
        );
        thread.resume_id = None;
        store.upsert_thread(&thread).unwrap();

        // Every Tervin-launched Thread has a null resume_id. If an empty lookup
        // matched them, the first observed pane session would adopt an unrelated
        // Thread and append its events to someone else's conversation.
        assert!(store.thread_by_resume_id("").unwrap().is_none());
    }
}
