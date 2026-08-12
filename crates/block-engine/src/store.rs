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

use crate::model::{
    Block, BlockOutput, BlockStatus, CommandHit, GitContext, ParsedOutput, RecentDir,
};
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
    /// Input the caller could fix, as opposed to something that went wrong. Separate so
    /// the UI can show the message as guidance rather than as a failure.
    #[error("{0}")]
    Invalid(String),
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

            -- Saved terminal output, so a pane can be restored with its history rather
            -- than as an empty rectangle.
            --
            -- Keyed by the pane id recorded in the saved session, not by the id the pane
            -- gets on the next run: those are generated per process, so restoring has to
            -- map old key to new pane.
            CREATE TABLE IF NOT EXISTS pane_scrollback (
                pane_key   TEXT PRIMARY KEY,
                saved_at   TEXT NOT NULL,
                -- The command the pane was running. Restoring output from a different
                -- program would put someone else's session on screen.
                program    TEXT,
                cwd        TEXT,
                body       TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS pane_scrollback_saved ON pane_scrollback(saved_at);

            -- Commands worth keeping, with their varying parts named.
            CREATE TABLE IF NOT EXISTS saved_commands (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                template    TEXT NOT NULL,
                description TEXT,
                uses        INTEGER NOT NULL DEFAULT 0,
                created_at  TEXT NOT NULL,
                last_used   TEXT
            );
            -- Names are how people find these, so two with the same name is a mistake
            -- rather than a feature.
            CREATE UNIQUE INDEX IF NOT EXISTS saved_commands_name ON saved_commands(name);
            -- Directories visited, so `cd` can offer somewhere you have actually been.
            --
            -- Both a count and a timestamp, because neither alone ranks well: pure
            -- recency loses the directory you live in the moment you visit anywhere
            -- else, and a pure count keeps a place you abandoned months ago at the top.
            CREATE TABLE IF NOT EXISTS recent_dirs (
                path      TEXT PRIMARY KEY,
                visits    INTEGER NOT NULL DEFAULT 1,
                last_used TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS recent_dirs_used ON recent_dirs(last_used DESC);

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

        // Both statements in this loop are built with `format!`, and the only thing
        // that makes them safe is `ADDED` being string literals in this file. A name
        // that arrived at runtime could not be rescued by binding it: `ALTER TABLE ?
        // ADD COLUMN ...` is a syntax error, because SQLite binds values and never
        // identifiers. So a column that has to be decided at run time is not a
        // parameter — it is a lookup against a fixed list, resolved before it reaches
        // either string.
        for (table, column, definition) in ADDED {
            // `{table}` is `ADDED.0`, a literal. This statement alone could bind it —
            // `pragma_table_info(?)` does accept a parameter — but binding only here
            // would leave the `ALTER TABLE` below taking the same name from the same
            // constant, which is a guard that looks total and is not. The constant is
            // the invariant; the shape of this statement is not.
            let exists: bool = conn
                .prepare(&format!("SELECT * FROM pragma_table_info('{table}')"))?
                .query_map([], |r| r.get::<_, String>("name"))?
                .filter_map(std::result::Result::ok)
                .any(|name| name == *column);
            if !exists {
                // All three come from `ADDED`. `definition` is SQL rather than a value
                // — `TEXT`, `INTEGER NOT NULL DEFAULT 0` — so this line cannot be
                // parameterised even in principle, and a `definition` that came from
                // outside this file would be a statement written by whoever supplied it.
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
        //
        // The three interpolated pieces are all written above and none of them carries
        // a value: `where_clause` joins the literal predicates pushed onto `wheres`,
        // `order` is one of three arms of a match over `SortOrder`, and `PREVIEW_CHARS`
        // is a `usize` constant. Everything the caller supplied — including the LIKE
        // patterns and the FTS5 expression — went into `binds` and arrives as a
        // parameter. A filter field appended to this string instead would be the one
        // place in this file where a Block's own text could become SQL.
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
    #[allow(
        clippy::too_many_arguments,
        reason = "the arguments are the audit record's columns; a struct would only \
                  move the same list one layer away from the SQL that consumes it"
    )]
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

    // -------------------------------------------------------- recent directories

    /// How many directories are remembered.
    ///
    /// Enough to cover months of real work, small enough that ranking them all in memory
    /// is cheaper than asking SQLite to do it.
    pub const MAX_RECENT_DIRS: usize = 500;

    /// Note that a directory was visited.
    pub fn record_directory(&self, path: &str) -> Result<()> {
        let path = path.trim_end_matches('/');
        // The root is not a place anyone means to go back to, and an empty path is not a
        // place at all.
        if path.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO recent_dirs (path, visits, last_used) VALUES (?1, 1, ?2)
             ON CONFLICT(path) DO UPDATE SET visits = visits + 1, last_used = ?2",
            params![path, rfc3339(&tervin_core::now())],
        )?;

        // Trimmed here rather than by a background job: the table is tiny and this keeps
        // the bound true at all times instead of eventually.
        conn.execute(
            "DELETE FROM recent_dirs WHERE path NOT IN (
                 SELECT path FROM recent_dirs ORDER BY last_used DESC LIMIT ?1
             )",
            params![Self::MAX_RECENT_DIRS as i64],
        )?;
        Ok(())
    }

    /// Every remembered directory with its visit count and age in hours.
    ///
    /// Returned unranked so the caller can combine this with a fuzzy match on the query —
    /// ranking here and filtering there would apply the two in the wrong order.
    pub fn recent_directories(&self) -> Result<Vec<RecentDir>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT path, visits, last_used FROM recent_dirs ORDER BY last_used DESC LIMIT ?1",
        )?;
        let now = tervin_core::now();
        let rows = stmt.query_map(params![Self::MAX_RECENT_DIRS as i64], |r| {
            let path: String = r.get(0)?;
            let visits: i64 = r.get(1)?;
            let last: String = r.get(2)?;
            Ok((path, visits, last))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (path, visits, last) = row?;
            let age_hours = chrono::DateTime::parse_from_rfc3339(&last)
                .map(|t| (now - t.with_timezone(&chrono::Utc)).num_minutes() as f64 / 60.0)
                .unwrap_or(f64::MAX);
            out.push(RecentDir {
                path,
                visits: visits.max(0) as u32,
                age_hours: age_hours.max(0.0),
            });
        }
        Ok(out)
    }

    /// Forget a directory — used when it no longer exists.
    pub fn forget_directory(&self, path: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM recent_dirs WHERE path = ?1", params![path])?;
        Ok(())
    }

    // ------------------------------------------------------- pane scrollback

    /// Largest scrollback kept for one pane.
    ///
    /// A pane with 10,000 lines of colourised output serialises to megabytes, and a
    /// dozen of those would make startup read tens of megabytes before drawing
    /// anything. Restoring the most recent screenfuls is what people actually want;
    /// the full history is what the Blocks store is for.
    pub const MAX_SCROLLBACK_BYTES: usize = 256 * 1024;

    /// Save a pane's output.
    ///
    /// Over-long text is trimmed from the *front*, keeping the end: the newest output
    /// is what a restored pane should show, and cutting the tail would restore a
    /// screen that stops mid-session.
    pub fn save_scrollback(
        &self,
        pane_key: &str,
        program: Option<&str>,
        cwd: Option<&str>,
        body: &str,
    ) -> Result<()> {
        let trimmed = if body.len() > Self::MAX_SCROLLBACK_BYTES {
            let mut start = body.len() - Self::MAX_SCROLLBACK_BYTES;
            // Never split a multi-byte character; the result is written to a terminal.
            while start < body.len() && !body.is_char_boundary(start) {
                start += 1;
            }
            &body[start..]
        } else {
            body
        };

        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO pane_scrollback (pane_key, saved_at, program, cwd, body)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                pane_key,
                rfc3339(&tervin_core::now()),
                program,
                cwd,
                trimmed
            ],
        )?;
        Ok(())
    }

    /// Load a pane's saved output, if it was running the same program.
    ///
    /// The program is checked rather than trusted: a saved session is keyed by pane id,
    /// and restoring a shell's history into what is now an SSH session — or an agent's
    /// TUI — would show output that never belonged to it.
    pub fn load_scrollback(&self, pane_key: &str, program: Option<&str>) -> Result<Option<String>> {
        let conn = self.conn.lock();
        let row: Option<(Option<String>, String)> = conn
            .query_row(
                "SELECT program, body FROM pane_scrollback WHERE pane_key = ?1",
                params![pane_key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((saved_program, body)) if saved_program.as_deref() == program => Some(body),
            _ => None,
        })
    }

    /// Forget saved output for panes that no longer exist in the saved session.
    ///
    /// Without this the table grows for the lifetime of the install, holding output from
    /// panes closed months ago — which is both waste and a needless amount of old
    /// terminal output sitting on disk.
    pub fn retain_scrollback(&self, keep: &[String]) -> Result<usize> {
        let conn = self.conn.lock();
        if keep.is_empty() {
            return Ok(conn.execute("DELETE FROM pane_scrollback", [])?);
        }
        // The only thing interpolated is a run of `?` characters, and the length of
        // that run comes from `keep.len()` — a count, computed here, never text from
        // the caller. Every pane key is bound below through `params_from_iter`.
        // Inlining the keys to save the binds would look harmless, because a pane key
        // is a short internal string; it is also stored data read back off disk, so
        // that version of this line would let the database decide what statement runs
        // against it.
        let placeholders = std::iter::repeat_n("?", keep.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("DELETE FROM pane_scrollback WHERE pane_key NOT IN ({placeholders})");
        let params = rusqlite::params_from_iter(keep.iter());
        Ok(conn.execute(&sql, params)?)
    }

    /// Drop saved output older than the retention window.
    pub fn prune_scrollback(&self, days: u32) -> Result<usize> {
        if days == 0 {
            return Ok(0);
        }
        let cutoff = tervin_core::now() - chrono::Duration::days(days as i64);
        let conn = self.conn.lock();
        Ok(conn.execute(
            "DELETE FROM pane_scrollback WHERE saved_at < ?1",
            params![rfc3339(&cutoff)],
        )?)
    }

    // --------------------------------------------------------- command history

    /// Commands you have run, deduplicated and ranked.
    ///
    /// This is the thing a shell's own history search cannot do. `Ctrl-R` searches *this*
    /// shell's history: one machine, one session's ancestry, no idea whether the command
    /// worked. Tervin already records every command with its exit status, directory and
    /// project, so it can answer "that command I ran last week in the other repo" and say
    /// whether it succeeded when you last ran it.
    ///
    /// Ranked by frecency over the *distinct* command text, so running something twenty
    /// times makes it easy to find rather than filling the list with twenty rows.
    /// No query parameter: a `LIKE` prefilter would exclude the non-contiguous matches a
    /// fuzzy search exists to find, so the newest distinct commands are returned and the
    /// caller ranks them.
    pub fn command_history(&self, project: Option<&str>, limit: usize) -> Result<Vec<CommandHit>> {
        let conn = self.conn.lock();
        // Grouped in SQL rather than in Rust: the blocks table is the largest thing here,
        // and pulling every row out to deduplicate would defeat the index.
        let mut sql = String::from(
            "SELECT command,
                    COUNT(*) AS uses,
                    MAX(started_at) AS last_used,
                    -- The status of the most recent run, not the best or the worst one:
                    -- what someone wants to know is whether it worked *last time*.
                    (SELECT b2.status FROM blocks b2
                      WHERE b2.command = b1.command
                      ORDER BY b2.started_at DESC LIMIT 1) AS last_status
               FROM blocks b1
              WHERE TRIM(command) <> ''",
        );
        if project.is_some() {
            sql.push_str(" AND project = ?2");
        }
        sql.push_str(" GROUP BY command ORDER BY last_used DESC LIMIT ?1");

        let now = tervin_core::now();
        let mut stmt = conn.prepare(&sql)?;
        // A generous window before ranking, because the fuzzy match happens in the caller
        // and the newest rows are not necessarily the ones that match.
        let scan = (limit.max(1) * 20).min(2_000) as i64;

        let map = |r: &Row<'_>| -> rusqlite::Result<(String, i64, String, String)> {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        };
        let rows: Vec<(String, i64, String, String)> = match project {
            Some(project) => stmt
                .query_map(params![scan, project], map)?
                .collect::<rusqlite::Result<_>>()?,
            None => stmt
                .query_map(params![scan], map)?
                .collect::<rusqlite::Result<_>>()?,
        };

        Ok(rows
            .into_iter()
            .map(|(command, uses, last_used, last_status)| {
                let age_hours = DateTime::parse_from_rfc3339(&last_used)
                    .map(|t| (now - t.with_timezone(&Utc)).num_minutes() as f64 / 60.0)
                    .unwrap_or(f64::MAX)
                    .max(0.0);
                CommandHit {
                    command,
                    uses: uses.max(0) as u32,
                    age_hours,
                    last_status,
                }
            })
            .collect())
    }

    // ----------------------------------------------------------- saved commands

    /// Save a command, replacing one with the same name.
    ///
    /// Replacing rather than erroring: someone refining a command they saved yesterday
    /// expects to overwrite it, and a duplicate-name error at that moment is an obstacle
    /// rather than a safeguard.
    pub fn upsert_saved_command(&self, command: &crate::saved::SavedCommand) -> Result<()> {
        let name = command.name.trim();
        let template = command.template.trim();
        if name.is_empty() || template.is_empty() {
            return Err(StoreError::Invalid(
                "A saved command needs a name and a command.".to_string(),
            ));
        }
        if template.len() > crate::saved::MAX_TEMPLATE {
            return Err(StoreError::Invalid(format!(
                "That command is {} bytes and the limit is {}.",
                template.len(),
                crate::saved::MAX_TEMPLATE
            )));
        }

        let conn = self.conn.lock();
        // Keyed on the name so a save overwrites — but the existing id and use count
        // survive, because refining a command should not reset how often you have used it.
        conn.execute(
            "INSERT INTO saved_commands (id, name, template, description, uses, created_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)
             ON CONFLICT(name) DO UPDATE SET template = ?3, description = ?4",
            params![
                command.id,
                name,
                template,
                command.description.as_deref().map(str::trim),
                rfc3339(&tervin_core::now()),
            ],
        )?;
        Ok(())
    }

    /// Every saved command, most used first.
    pub fn saved_commands(&self) -> Result<Vec<crate::saved::SavedCommand>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, template, description, uses FROM saved_commands
             ORDER BY uses DESC, last_used DESC, name ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(crate::saved::SavedCommand {
                id: r.get(0)?,
                name: r.get(1)?,
                template: r.get(2)?,
                description: r.get(3)?,
                uses: r.get::<_, i64>(4)?.max(0) as u32,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Note that a saved command was used, so the list ranks by what you reach for.
    pub fn record_saved_command_use(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE saved_commands SET uses = uses + 1, last_used = ?2 WHERE id = ?1",
            params![id, rfc3339(&tervin_core::now())],
        )?;
        Ok(())
    }

    pub fn delete_saved_command(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM saved_commands WHERE id = ?1", params![id])?;
        Ok(())
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

/// Turn what somebody typed into an FTS5 `MATCH` expression.
///
/// Every term becomes a quoted phrase with a prefix marker, so `AND`, `NEAR(` and
/// `foo(` typed mid-thought are matched as text instead of parsed as operators, and a
/// term with nothing to search for is dropped rather than carried. That is about not
/// erroring on a half-typed query, not about safety: the string this returns is bound
/// as a parameter at both call sites, so nothing here can reach the statement.
fn fts_query(input: &str) -> String {
    let terms: Vec<String> = input
        .split_whitespace()
        // A term with nothing the tokenizer would index — `--`, `*`, `';` — becomes an
        // empty phrase, and an empty phrase matches no row at all. Left in the `AND`
        // chain it takes the rest of the query down with it, so `-- hello` would find
        // nothing while `hello` found rows: a search box reporting "no matches" about a
        // document it can see. Both indexes are declared `tokenize = 'unicode61'`, which
        // indexes letters and numbers and treats every other character as a separator,
        // so `is_alphanumeric` asks the same question in Rust. Where the two disagree
        // the term survives and the query matches nothing, which is what it did before.
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .map(|t| {
            // A quote would close the phrase early and turn the rest of the term into
            // operators; a space cannot, and splits it into two tokens of one phrase.
            let cleaned = t.replace('"', " ");
            format!("\"{}\"*", cleaned.trim())
        })
        .collect();
    if terms.is_empty() {
        // Matches nothing rather than erroring on an empty query. "Nothing" and not
        // "everything": input that is only punctuation is not a request for the whole
        // database.
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

/// What the search box does with input nobody meant as a query.
///
/// Both search paths build an FTS5 expression with [`fts_query`] and bind it as a
/// parameter, so the question is not whether a quote can close the statement — it
/// cannot — but whether an operator, an unbalanced quote, or something written to end
/// the statement changes what the statement *means*, and what the user is told when it
/// does. Driven through [`Store::query_blocks`] and [`Store::search_prompts`] against a
/// real database on disk with real rows in it: a statement that damaged the schema
/// would have to damage a file for the claim to be worth making.
#[cfg(test)]
mod search_tests {
    use super::*;
    use tervin_core::{AgentIdentity, EventPayload, TervinEvent, Tier};

    /// Input nobody typed as a search: FTS5 operators, unbalanced punctuation, and a
    /// payload written to close the statement and drop the table it reads from.
    ///
    /// `don't` is in the list because it is the one entry somebody types on purpose,
    /// and it is the one that tells the two mistakes apart. A sanitised expression
    /// pasted into the statement instead of bound survives every other probe here —
    /// `fts_query` emits double quotes and never a single one — and stops surviving
    /// the moment an apostrophe reaches it.
    const ADVERSARIAL: &[&str] = &[
        "\"",
        "--",
        "NEAR(a b)",
        "*",
        "a OR b",
        "foo(",
        "don't stop",
        "'; DROP TABLE blocks; --",
        "",
    ];

    fn on_disk(dir: &tempfile::TempDir) -> Store {
        Store::open(&dir.path().join("workspace.db")).unwrap()
    }

    fn block(command: &str, output: &str) -> Block {
        let mut b = Block::new(PaneId::new(), SessionId::new(), command, "/proj", "local");
        b.status = BlockStatus::Succeeded;
        b.output.inline = output.as_bytes().to_vec();
        b.output.total_bytes = output.len() as u64;
        b
    }

    fn prompt(text: &str) -> TervinEvent {
        TervinEvent::new(
            AgentIdentity::new("claude-code", "Claude Code", Tier::Structured),
            "asked something",
            EventPayload::UserPrompted {
                text: text.to_string(),
            },
        )
    }

    fn find_blocks(store: &Store, text: &str) -> usize {
        store
            .query_blocks(&BlockFilter {
                text: Some(text.to_string()),
                ..Default::default()
            })
            .unwrap()
            .len()
    }

    /// The tables the database actually has, read back rather than assumed.
    fn tables(store: &Store) -> Vec<String> {
        let conn = store.conn.lock();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<String>>>()
            .unwrap()
    }

    #[test]
    fn an_adversarial_search_query_cannot_change_what_the_statement_means() {
        let dir = tempfile::tempdir().unwrap();
        let store = on_disk(&dir);
        let seeded = block("echo hi", "hi there");
        store.upsert_block(&seeded).unwrap();
        store
            .append_event(&prompt("what does this do"), None)
            .unwrap();

        let schema = tables(&store);
        assert!(
            schema.contains(&"blocks".to_string()),
            "the fixture is wrong: {schema:?}"
        );

        for probe in ADVERSARIAL {
            let blocks = store.query_blocks(&BlockFilter {
                text: Some((*probe).to_string()),
                ..Default::default()
            });
            assert!(
                blocks.is_ok(),
                "searching Blocks for {probe:?} errored: {:?}",
                blocks.err()
            );

            let prompts = store.search_prompts(probe, 10);
            assert!(
                prompts.is_ok(),
                "searching prompts for {probe:?} errored: {:?}",
                prompts.err()
            );

            // "It did not error" is the weak half. A statement that ran the payload
            // would also return cleanly, so the schema and the rows are what say it
            // did not: the table is still there and still holds what it held.
            assert_eq!(
                tables(&store),
                schema,
                "the schema changed while searching for {probe:?}"
            );
            assert!(
                store.get_block(&seeded.id).unwrap().is_some(),
                "the Block did not survive a search for {probe:?}"
            );
            assert_eq!(
                store.search_prompts("", 10).unwrap().len(),
                1,
                "the prompt index did not survive a search for {probe:?}"
            );
        }
    }

    #[test]
    fn a_search_mixing_punctuation_with_a_word_still_finds_the_word() {
        // `--` holds no character `unicode61` indexes, so it tokenises to an empty
        // phrase, and an empty phrase matches no row. Joined with ` AND ` it took the
        // rest of the query with it: `-- hello` found nothing while `hello` found the
        // row, which is a search box saying "no matches" about a document it can see.
        // Nobody types `--` deliberately, but `-->`, a pasted diff marker and the
        // injection payload above all arrive with a fragment like it.
        let dir = tempfile::tempdir().unwrap();
        let store = on_disk(&dir);
        store
            .upsert_block(&block("cargo test", "hello from the fixture"))
            .unwrap();
        store
            .append_event(&prompt("hello, can you look at this"), None)
            .unwrap();

        assert_eq!(find_blocks(&store, "hello"), 1, "the fixture is wrong");
        assert_eq!(
            find_blocks(&store, "-- hello"),
            1,
            "a fragment with nothing to search for swallowed the rest of the query"
        );
        assert_eq!(find_blocks(&store, "hello --"), 1);
        assert_eq!(store.search_prompts("-- hello", 10).unwrap().len(), 1);

        // The other half, or dropping the fragment would be a widening: input that is
        // only punctuation is unsearchable, and unsearchable means no rows rather than
        // every row.
        assert_eq!(
            find_blocks(&store, "--"),
            0,
            "a query with nothing to match returned rows"
        );
        assert_eq!(store.search_prompts("-- ;", 10).unwrap().len(), 0);
    }

    #[test]
    fn an_injection_payload_is_searched_for_as_text() {
        // What the statement means, stated positively. The words in that payload are
        // ordinary words, and someone who ran the command and is looking for it again
        // should get the Block back — the same answer the search gives for any other
        // text, which is the point.
        let dir = tempfile::tempdir().unwrap();
        let store = on_disk(&dir);
        store
            .upsert_block(&block(
                "psql tervin",
                "tervin=# DROP TABLE blocks;\nERROR:  permission denied for table blocks",
            ))
            .unwrap();

        assert_eq!(find_blocks(&store, "'; DROP TABLE blocks; --"), 1);
        assert!(
            tables(&store).contains(&"blocks".to_string()),
            "the table named in the payload is gone"
        );
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

/// Saved terminal output for restoring a pane.
#[cfg(test)]
mod scrollback_tests {
    use super::*;

    #[test]
    fn round_trips_output_for_the_same_program() {
        let store = Store::open_in_memory().unwrap();
        store
            .save_scrollback("pane_1", Some("/bin/zsh"), Some("/proj"), "$ ls\nsrc\n")
            .unwrap();
        assert_eq!(
            store.load_scrollback("pane_1", Some("/bin/zsh")).unwrap(),
            Some("$ ls\nsrc\n".to_string())
        );
    }

    #[test]
    fn refuses_to_restore_output_from_a_different_program() {
        // A saved session is keyed by pane id, and ids are reused across runs. Restoring
        // a local shell's history into what is now an SSH session would put output on
        // screen that never belonged to it — and on a remote host, that is misleading in
        // a way that matters.
        let store = Store::open_in_memory().unwrap();
        store
            .save_scrollback("pane_1", Some("/bin/zsh"), None, "local output")
            .unwrap();

        assert_eq!(store.load_scrollback("pane_1", Some("ssh")).unwrap(), None);
        assert_eq!(store.load_scrollback("pane_1", None).unwrap(), None);
    }

    #[test]
    fn a_plain_shell_pane_is_matched_by_its_absent_program() {
        // A pane with no explicit program is the user's default shell. `None` has to
        // match `None`, or an ordinary pane would never restore.
        let store = Store::open_in_memory().unwrap();
        store
            .save_scrollback("pane_1", None, None, "output")
            .unwrap();
        assert_eq!(
            store.load_scrollback("pane_1", None).unwrap(),
            Some("output".to_string())
        );
    }

    #[test]
    fn over_long_output_keeps_the_end_and_stays_valid_utf8() {
        let store = Store::open_in_memory().unwrap();
        // Multi-byte, so a byte-wise trim would split a character and produce output
        // that renders as a replacement glyph in the restored pane.
        let filler = "é".repeat(Store::MAX_SCROLLBACK_BYTES);
        let body = format!("{filler}THE-NEWEST-LINE");
        store.save_scrollback("pane_1", None, None, &body).unwrap();

        let loaded = store.load_scrollback("pane_1", None).unwrap().unwrap();
        assert!(loaded.len() <= Store::MAX_SCROLLBACK_BYTES);
        // The newest output is what a restored pane should show; cutting the tail would
        // restore a screen that stops mid-session.
        assert!(
            loaded.ends_with("THE-NEWEST-LINE"),
            "the end of the output was discarded"
        );
        // Valid UTF-8 by construction — `String` would not have survived otherwise, but
        // assert the boundary was respected rather than relying on that.
        assert!(!loaded.starts_with('\u{FFFD}'));
    }

    #[test]
    fn saving_again_replaces_rather_than_accumulates() {
        let store = Store::open_in_memory().unwrap();
        store
            .save_scrollback("pane_1", None, None, "first")
            .unwrap();
        store
            .save_scrollback("pane_1", None, None, "second")
            .unwrap();
        assert_eq!(
            store.load_scrollback("pane_1", None).unwrap(),
            Some("second".to_string())
        );
    }

    #[test]
    fn retaining_drops_panes_that_are_no_longer_in_the_session() {
        let store = Store::open_in_memory().unwrap();
        for key in ["a", "b", "c"] {
            store.save_scrollback(key, None, None, "x").unwrap();
        }

        let removed = store
            .retain_scrollback(&["a".to_string(), "c".to_string()])
            .unwrap();
        assert_eq!(removed, 1);
        assert!(store.load_scrollback("a", None).unwrap().is_some());
        assert!(store.load_scrollback("b", None).unwrap().is_none());
        assert!(store.load_scrollback("c", None).unwrap().is_some());
    }

    #[test]
    fn retaining_nothing_clears_everything() {
        // The case that matters: session restore turned off should not leave old
        // terminal output sitting on disk.
        let store = Store::open_in_memory().unwrap();
        store.save_scrollback("a", None, None, "x").unwrap();
        assert_eq!(store.retain_scrollback(&[]).unwrap(), 1);
        assert!(store.load_scrollback("a", None).unwrap().is_none());
    }

    #[test]
    fn pruning_respects_the_retention_window_and_forever_means_forever() {
        let store = Store::open_in_memory().unwrap();
        store.save_scrollback("recent", None, None, "x").unwrap();

        // Nothing is old enough yet.
        assert_eq!(store.prune_scrollback(30).unwrap(), 0);
        // 0 days is "keep indefinitely", not "delete everything" — the same meaning the
        // history retention control uses, and getting it backwards would silently
        // destroy data.
        assert_eq!(store.prune_scrollback(0).unwrap(), 0);
        assert!(store.load_scrollback("recent", None).unwrap().is_some());
    }

    #[test]
    fn a_pane_with_no_saved_output_simply_has_none() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.load_scrollback("never-seen", None).unwrap(), None);
    }
}

/// Saved commands: storage, replacement, and ranking.
#[cfg(test)]
mod saved_command_tests {
    use super::*;
    use crate::saved::SavedCommand;

    fn cmd(name: &str, template: &str) -> SavedCommand {
        SavedCommand {
            id: format!("sc_{name}"),
            name: name.to_string(),
            template: template.to_string(),
            description: None,
            uses: 0,
        }
    }

    #[test]
    fn round_trips_a_saved_command() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_saved_command(&cmd("deploy", "deploy {{env:staging}}"))
            .unwrap();

        let all = store.saved_commands().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "deploy");
        assert_eq!(all[0].template, "deploy {{env:staging}}");
    }

    #[test]
    fn saving_the_same_name_refines_it_without_resetting_its_use_count() {
        // Someone tweaking yesterday's command expects to overwrite it, and losing the
        // ranking as a side effect would push it back down the list for no reason.
        let store = Store::open_in_memory().unwrap();
        store.upsert_saved_command(&cmd("deploy", "old")).unwrap();
        let id = store.saved_commands().unwrap()[0].id.clone();
        store.record_saved_command_use(&id).unwrap();
        store.record_saved_command_use(&id).unwrap();

        let mut refined = cmd("deploy", "new and better");
        // A caller that generates a fresh id must not create a second row.
        refined.id = "sc_different".to_string();
        store.upsert_saved_command(&refined).unwrap();

        let all = store.saved_commands().unwrap();
        assert_eq!(all.len(), 1, "a duplicate row was created");
        assert_eq!(all[0].template, "new and better");
        assert_eq!(all[0].uses, 2, "the use count was reset");
        assert_eq!(all[0].id, id, "the id changed under existing references");
    }

    #[test]
    fn ranks_by_what_you_reach_for() {
        let store = Store::open_in_memory().unwrap();
        for name in ["alpha", "beta", "gamma"] {
            store.upsert_saved_command(&cmd(name, "x")).unwrap();
        }
        let beta = store
            .saved_commands()
            .unwrap()
            .into_iter()
            .find(|c| c.name == "beta")
            .unwrap();
        store.record_saved_command_use(&beta.id).unwrap();

        assert_eq!(store.saved_commands().unwrap()[0].name, "beta");
    }

    #[test]
    fn unused_commands_are_ordered_predictably() {
        // With no use counts to separate them the list must not shuffle between opens.
        let store = Store::open_in_memory().unwrap();
        for name in ["gamma", "alpha", "beta"] {
            store.upsert_saved_command(&cmd(name, "x")).unwrap();
        }
        let names: Vec<String> = store
            .saved_commands()
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn a_command_with_no_name_or_no_body_is_refused_with_a_reason() {
        let store = Store::open_in_memory().unwrap();
        for (name, template) in [("", "ls"), ("  ", "ls"), ("name", ""), ("name", "   ")] {
            let err = store
                .upsert_saved_command(&cmd(name, template))
                .expect_err("empty input should be refused");
            // Guidance, not a failure: the message is shown to the person who typed it.
            assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
            assert!(err.to_string().contains("needs a name"));
        }
        assert!(store.saved_commands().unwrap().is_empty());
    }

    #[test]
    fn an_absurdly_long_command_is_refused_and_says_the_limit() {
        let store = Store::open_in_memory().unwrap();
        let huge = "x".repeat(crate::saved::MAX_TEMPLATE + 1);
        let err = store
            .upsert_saved_command(&cmd("pasted", &huge))
            .expect_err("an over-long template should be refused");
        assert!(err
            .to_string()
            .contains(&crate::saved::MAX_TEMPLATE.to_string()));
    }

    #[test]
    fn name_and_body_are_trimmed_so_a_stray_space_is_not_a_new_command() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_saved_command(&cmd("deploy", "  deploy  "))
            .unwrap();
        store
            .upsert_saved_command(&cmd("  deploy  ", "again"))
            .unwrap();

        let all = store.saved_commands().unwrap();
        assert_eq!(all.len(), 1, "a trailing space created a second command");
        assert_eq!(all[0].template, "again");
    }

    #[test]
    fn deleting_removes_only_that_command() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_saved_command(&cmd("keep", "x")).unwrap();
        store.upsert_saved_command(&cmd("drop", "y")).unwrap();

        let drop_id = store
            .saved_commands()
            .unwrap()
            .into_iter()
            .find(|c| c.name == "drop")
            .unwrap()
            .id;
        store.delete_saved_command(&drop_id).unwrap();

        let names: Vec<String> = store
            .saved_commands()
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["keep"]);
    }

    #[test]
    fn recording_a_use_for_something_gone_is_harmless() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.record_saved_command_use("sc_never").is_ok());
        assert!(store.delete_saved_command("sc_never").is_ok());
    }
}

/// Recent directories: recording, bounding, and what frecency is for.
#[cfg(test)]
mod recent_dir_tests {
    use super::*;
    use crate::model::RecentDir;

    #[test]
    fn records_a_visit_and_counts_repeats() {
        let store = Store::open_in_memory().unwrap();
        store.record_directory("/proj/app").unwrap();
        store.record_directory("/proj/app").unwrap();
        store.record_directory("/proj/docs").unwrap();

        let dirs = store.recent_directories().unwrap();
        let app = dirs.iter().find(|d| d.path == "/proj/app").unwrap();
        assert_eq!(app.visits, 2);
        assert_eq!(dirs.len(), 2);
    }

    #[test]
    fn a_trailing_slash_is_the_same_directory() {
        // OSC 7 reports a trailing slash on some shells and not others. Two entries for
        // one directory would split its visit count and rank it lower than it deserves.
        let store = Store::open_in_memory().unwrap();
        store.record_directory("/proj/app").unwrap();
        store.record_directory("/proj/app/").unwrap();

        let dirs = store.recent_directories().unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].visits, 2);
    }

    #[test]
    fn an_empty_path_is_not_a_place() {
        let store = Store::open_in_memory().unwrap();
        store.record_directory("").unwrap();
        store.record_directory("/").unwrap();
        // `/` normalises to empty once the trailing slash is stripped, and nobody means
        // to navigate back to the root from a picker.
        assert!(store.recent_directories().unwrap().is_empty());
    }

    #[test]
    fn the_table_stays_bounded() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..Store::MAX_RECENT_DIRS + 40 {
            store.record_directory(&format!("/proj/dir{i}")).unwrap();
        }
        // Trimmed on write, so the bound holds at all times rather than eventually.
        assert_eq!(
            store.recent_directories().unwrap().len(),
            Store::MAX_RECENT_DIRS
        );
    }

    #[test]
    fn forgetting_removes_one_directory() {
        let store = Store::open_in_memory().unwrap();
        store.record_directory("/proj/gone").unwrap();
        store.record_directory("/proj/here").unwrap();
        store.forget_directory("/proj/gone").unwrap();

        let paths: Vec<String> = store
            .recent_directories()
            .unwrap()
            .into_iter()
            .map(|d| d.path)
            .collect();
        assert_eq!(paths, vec!["/proj/here".to_string()]);
    }

    #[test]
    fn a_freshly_recorded_directory_reads_as_recent() {
        let store = Store::open_in_memory().unwrap();
        store.record_directory("/proj/app").unwrap();
        // The age is computed at read time, so a just-recorded directory must land in
        // the "today" band rather than at some default.
        assert!(store.recent_directories().unwrap()[0].age_hours < 1.0);
    }

    /// The ranking itself, which is the part that decides whether the feature feels
    /// right. Tested on the value rather than through the database, so each case is one
    /// clear comparison.
    #[test]
    fn frecency_prefers_where_you_actually_work() {
        let dir = |visits, age_hours| RecentDir {
            path: "/x".to_string(),
            visits,
            age_hours,
        };

        // The case pure recency gets wrong: you glance at a directory once, and it
        // outranks the one you have been in fifty times today.
        let daily = dir(50, 2.0);
        let glanced = dir(1, 0.1);
        assert!(daily.frecency() > glanced.frecency());

        // The case a pure visit count gets wrong: somewhere you lived in months ago and
        // have not opened since stays at the top forever.
        let abandoned = dir(80, 24.0 * 90.0);
        let current = dir(10, 3.0);
        assert!(current.frecency() > abandoned.frecency());

        // And within the same band, more visits wins.
        assert!(dir(9, 3.0).frecency() > dir(3, 3.0).frecency());
        // While across bands, the same count decays.
        assert!(dir(5, 1.0).frecency() > dir(5, 24.0 * 10.0).frecency());
    }

    #[test]
    fn frecency_never_goes_negative_or_undefined() {
        // `age_hours` is clamped at read time, but a row with a corrupt timestamp reads
        // as f64::MAX and must still produce a usable ordering rather than a NaN that
        // makes the sort comparator arbitrary.
        let broken = RecentDir {
            path: "/x".to_string(),
            visits: 0,
            age_hours: f64::MAX,
        };
        assert!(broken.frecency().is_finite());
        assert!(broken.frecency() >= 0.0);
    }
}

/// Command history: the thing a shell's own `Ctrl-R` cannot do.
#[cfg(test)]
mod command_history_tests {
    use super::*;
    use crate::model::CommandHit;

    fn block_at(
        command: &str,
        status: BlockStatus,
        project: Option<&str>,
        minutes_ago: i64,
    ) -> Block {
        let mut b = Block::new(
            PaneId::from_external("pane_1"),
            SessionId::new(),
            command,
            "/proj",
            "local",
        );
        b.status = status;
        b.project = project.map(str::to_string);
        b.started_at = tervin_core::now() - chrono::Duration::minutes(minutes_ago);
        b
    }

    #[test]
    fn deduplicates_a_command_run_many_times() {
        // Twenty rows for one command would fill the list rather than making it findable.
        let store = Store::open_in_memory().unwrap();
        for i in 0..5 {
            store
                .upsert_block(&block_at("cargo test", BlockStatus::Succeeded, None, i))
                .unwrap();
        }
        store
            .upsert_block(&block_at("cargo build", BlockStatus::Succeeded, None, 10))
            .unwrap();

        let hits = store.command_history(None, 50).unwrap();
        assert_eq!(hits.len(), 2);
        let test = hits.iter().find(|h| h.command == "cargo test").unwrap();
        assert_eq!(test.uses, 5);
    }

    #[test]
    fn reports_the_status_of_the_most_recent_run_not_the_best_one() {
        // What someone wants to know before reusing a command is whether it worked *last
        // time*. Reporting the best outcome would be reassuring and wrong.
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_block(&block_at("flaky", BlockStatus::Succeeded, None, 120))
            .unwrap();
        store
            .upsert_block(&block_at("flaky", BlockStatus::Failed, None, 5))
            .unwrap();

        let hit = &store.command_history(None, 10).unwrap()[0];
        assert_eq!(hit.last_status, "failed");
        assert!(hit.failed_last_time());
    }

    #[test]
    fn a_command_that_succeeded_most_recently_is_not_flagged() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_block(&block_at("ok", BlockStatus::Failed, None, 120))
            .unwrap();
        store
            .upsert_block(&block_at("ok", BlockStatus::Succeeded, None, 5))
            .unwrap();
        assert!(!store.command_history(None, 10).unwrap()[0].failed_last_time());
    }

    #[test]
    fn scopes_to_a_project_when_asked() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_block(&block_at(
                "in-tervin",
                BlockStatus::Succeeded,
                Some("tervin"),
                1,
            ))
            .unwrap();
        store
            .upsert_block(&block_at(
                "in-other",
                BlockStatus::Succeeded,
                Some("other"),
                2,
            ))
            .unwrap();

        let scoped: Vec<String> = store
            .command_history(Some("tervin"), 10)
            .unwrap()
            .into_iter()
            .map(|h| h.command)
            .collect();
        assert_eq!(scoped, vec!["in-tervin"]);
        // And unscoped sees both, because "that command from the other repo" is the whole
        // reason this beats a shell's own history.
        assert_eq!(store.command_history(None, 10).unwrap().len(), 2);
    }

    #[test]
    fn an_empty_command_is_not_history() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_block(&block_at("   ", BlockStatus::Succeeded, None, 1))
            .unwrap();
        assert!(store.command_history(None, 10).unwrap().is_empty());
    }

    #[test]
    fn a_fresh_command_reads_as_recent() {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_block(&block_at("just now", BlockStatus::Succeeded, None, 0))
            .unwrap();
        assert!(store.command_history(None, 10).unwrap()[0].age_hours < 1.0);
    }

    #[test]
    fn frecency_prefers_what_you_actually_run() {
        let hit = |uses, age_hours| CommandHit {
            command: "x".to_string(),
            uses,
            age_hours,
            last_status: "succeeded".to_string(),
        };
        // The case pure recency gets wrong.
        assert!(hit(40, 3.0).frecency() > hit(1, 0.1).frecency());
        // The case a pure count gets wrong.
        assert!(hit(10, 2.0).frecency() > hit(90, 24.0 * 120.0).frecency());
        // And a corrupt timestamp must still order rather than produce a NaN.
        assert!(hit(0, f64::MAX).frecency().is_finite());
    }

    #[test]
    fn no_query_filter_is_applied_so_a_fuzzy_match_can_still_find_things() {
        // A SQL `LIKE` prefilter would exclude the non-contiguous matches a fuzzy search
        // exists to find, which is why the query is not a parameter here at all.
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_block(&block_at(
                "cargo test --workspace",
                BlockStatus::Succeeded,
                None,
                1,
            ))
            .unwrap();
        // `ctw` matches nothing by substring but everything by subsequence.
        assert_eq!(store.command_history(None, 10).unwrap().len(), 1);
    }
}
