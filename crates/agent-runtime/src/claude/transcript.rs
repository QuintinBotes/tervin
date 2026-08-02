//! Reading a Claude Code session transcript from disk.
//!
//! When someone runs `claude` themselves in a pane, Tervin is not the one driving
//! it — there is no stdio to read, and no `--output-format stream-json` to ask for.
//! What there *is* is the `transcript_path` the agent announces when a turn ends
//! (see [`crate::claude::hooks`] and the OSC 777 envelope in `terminal-core`).
//!
//! That path points at the session's JSONL log, which is the same data Tervin would
//! have received live. So a session someone started for themselves can still become
//! a real Thread with a real timeline, rather than a line saying "an agent ran here".
//!
//! ## Reading is incremental and tolerant
//!
//! The agent is still appending while we read, so:
//!
//! - a trailing partial line is not consumed, and the offset stops before it;
//! - unparseable lines are skipped rather than aborting the read, because one bad
//!   line must not cost the rest of the session;
//! - each call reads a bounded amount, so a very long session cannot stall the
//!   caller.
//!
//! The format is not documented and will change. Everything here treats a missing
//! or unexpected field as "nothing to report" rather than as an error, and entry
//! types it does not recognise are counted and skipped — the same discipline the
//! live adapters follow with `runtime.unclassified`.

use serde::Deserialize;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tervin_core::events::{FileChange, FileChangeKind};
use tervin_core::EventPayload;

/// Most bytes one [`TranscriptReader::read_new`] call will consume.
///
/// A long session's log reaches tens of megabytes. Parsing all of it on the thread
/// that pumps the PTY would stall drawing, so a read is capped and the remainder is
/// picked up by the next one.
const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

/// Longest message text kept. Beyond this the text is truncated with a marker, so
/// a pasted file in a prompt does not become a multi-megabyte timeline row.
const MAX_TEXT: usize = 16 * 1024;

/// A transcript being followed.
///
/// Holds a byte offset rather than a line count so re-reading is cheap and does not
/// depend on the file's earlier content staying identical.
#[derive(Debug)]
pub struct TranscriptReader {
    path: PathBuf,
    offset: u64,
    /// Shell tool calls seen but not yet resolved, so a `tool_result` can be recognised
    /// as a command's outcome rather than some other tool's.
    shell_calls: ShellCalls,
    /// Lines that were not valid JSON, or were a conversational turn this build
    /// could make nothing of.
    ///
    /// Reported rather than hidden: a jump here after an agent update is the signal
    /// that this parser needs revisiting. Bookkeeping entries are *not* counted —
    /// they are deliberately not events, and folding them in would swamp the number
    /// and make it useless as a signal.
    unrecognised: usize,
}

/// One thing that happened in a session, ready to become a [`TervinEvent`].
///
/// [`TervinEvent`]: tervin_core::TervinEvent
#[derive(Debug, Clone)]
pub struct TranscriptEntry {
    pub payload: EventPayload,
    /// RFC 3339, as written by the agent. Kept as text because it is only used for
    /// ordering and display, and reparsing it would invent precision.
    pub ts: Option<String>,
    /// True when this came from a subagent rather than the main conversation.
    pub sidechain: bool,
}

impl TranscriptReader {
    /// Follow `path` from the beginning.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: 0,
            shell_calls: ShellCalls::default(),
            unrecognised: 0,
        }
    }

    /// Follow `path`, ignoring everything already in it.
    ///
    /// Used when a session is noticed part-way through: replaying a conversation the
    /// user had before Tervin was watching would put events in the timeline that
    /// look current but are not.
    pub fn from_end(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let offset = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Self {
            path,
            offset,
            shell_calls: ShellCalls::default(),
            unrecognised: 0,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How many lines could not be read. See the field's note for what counts.
    pub fn unrecognised(&self) -> usize {
        self.unrecognised
    }

    /// Read whatever has been appended since the last call.
    ///
    /// Returns an empty vector when there is nothing new, which is the common case
    /// and must stay cheap.
    pub fn read_new(&mut self) -> std::io::Result<Vec<TranscriptEntry>> {
        let mut file = std::fs::File::open(&self.path)?;
        let len = file.metadata()?.len();

        // A shorter file than last time means it was replaced — a `/clear`, or a new
        // session reusing the path. Starting over is right; seeking to a stale
        // offset would read from the middle of a line.
        if len < self.offset {
            self.offset = 0;
        }
        if len == self.offset {
            return Ok(Vec::new());
        }

        file.seek(SeekFrom::Start(self.offset))?;
        let want = (len - self.offset).min(MAX_READ_BYTES) as usize;
        let mut buf = vec![0u8; want];
        let read = file.read(&mut buf)?;
        buf.truncate(read);

        // Stop at the last newline: anything after it is a line the agent has not
        // finished writing, and parsing half a JSON object yields nothing useful.
        let end = match buf.iter().rposition(|&b| b == b'\n') {
            Some(i) => i + 1,
            None => return Ok(Vec::new()),
        };
        self.offset += end as u64;

        let mut out = Vec::new();
        for line in buf[..end].split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<Line>(line) {
                Ok(parsed) => {
                    if !parsed.push_into(&mut out, &mut self.shell_calls) {
                        self.unrecognised += 1;
                    }
                }
                // Not valid JSON. Skipped, counted, and not fatal.
                Err(_) => self.unrecognised += 1,
            }
        }
        Ok(out)
    }
}

/// Shell tool calls awaiting a result.
///
/// Bounded, because a transcript read from the middle can contain results for calls whose
/// requests were never seen — and an unbounded set of ids that will never be resolved is
/// a slow leak for a long-running session.
#[derive(Debug, Default)]
struct ShellCalls {
    /// Call id to the command it ran, oldest first.
    pending: std::collections::VecDeque<(String, String)>,
}

impl ShellCalls {
    /// Most unresolved shell calls remembered at once.
    const MAX: usize = 64;

    fn started(&mut self, id: String, command: String) {
        if self.pending.len() >= Self::MAX {
            self.pending.pop_front();
        }
        self.pending.push_back((id, command));
    }

    /// Take the command for a call id, if it was a shell call.
    fn finish(&mut self, id: &str) -> Option<String> {
        let index = self.pending.iter().position(|(pending, _)| pending == id)?;
        self.pending.remove(index).map(|(_, command)| command)
    }
}

// ------------------------------------------------------------------ wire types
//
// Only the fields Tervin uses are declared. `serde` ignores the rest, so the agent
// adding fields is a non-event.

#[derive(Debug, Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    message: Option<Message>,
    timestamp: Option<String>,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: bool,
    /// Sits beside the `tool_result` block and carries what the tool actually produced.
    ///
    /// For a shell call that is `stdout`, `stderr` and `interrupted` — enough to build a
    /// real Block for a command run in a pane, rather than a bare "a tool ran" row.
    #[serde(rename = "toolUseResult")]
    tool_use_result: Option<ToolUseResult>,
}

#[derive(Debug, Deserialize)]
struct ToolUseResult {
    #[serde(default)]
    stdout: Option<String>,
    #[serde(default)]
    stderr: Option<String>,
    #[serde(default)]
    interrupted: bool,
}

#[derive(Debug, Deserialize)]
struct Message {
    #[serde(default)]
    content: Content,
    model: Option<String>,
}

/// `content` is a bare string on user turns and a block list on assistant turns.
///
/// Untagged, so serde tries these in order. The trailing catch-all is what keeps a
/// shape this build has never seen from failing the whole line — the rest of the
/// entry is still worth reading.
#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Blocks(Vec<Block>),
    /// A shape this build does not model. The value is not read — the variant exists
    /// only so serde has somewhere to land instead of failing the whole line.
    Unknown(#[allow(dead_code)] serde_json::Value),
    #[default]
    Absent,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    ToolResult {
        /// Present on every version seen. Used to tell a shell call's result from any
        /// other tool's, which is what makes a command's Block possible.
        #[serde(default)]
        tool_use_id: Option<String>,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        content: serde_json::Value,
    },
    #[serde(other)]
    Other,
}

impl Line {
    /// Turn one transcript line into zero or more events.
    ///
    /// Returns false only when this build could not account for the line: a
    /// conversational turn it read nothing out of. A bookkeeping entry returns true,
    /// because ignoring it is the intended outcome rather than a gap.
    fn push_into(self, out: &mut Vec<TranscriptEntry>, shell_calls: &mut ShellCalls) -> bool {
        let sidechain = self.is_sidechain;
        let ts = self.timestamp.clone();
        let before = out.len();
        let mut push = |payload: EventPayload| {
            out.push(TranscriptEntry {
                payload,
                ts: ts.clone(),
                sidechain,
            })
        };

        let Some(message) = self.message else {
            // No message at all: a queue operation, an attachment, a prompt record.
            return true;
        };
        // Anything that is not a conversational turn — queue operations, file
        // attachments, prompt bookkeeping — is not part of the story of the
        // session and is left out on purpose.
        let is_user = matches!(self.kind.as_deref(), Some("user"));
        let is_assistant = matches!(self.kind.as_deref(), Some("assistant"));
        if !is_user && !is_assistant {
            return true;
        }

        match message.content {
            Content::Text(text) => {
                let text = clamp(&text);
                if text.is_empty() {
                    // An empty turn is not a parser gap; there is simply nothing
                    // to record.
                    return true;
                }
                if is_user {
                    push(EventPayload::UserPrompted { text });
                } else {
                    push(EventPayload::AgentMessage {
                        text,
                        is_reasoning: false,
                        parent_tool_use_id: None,
                    });
                }
            }
            Content::Blocks(blocks) => {
                for block in blocks {
                    match block {
                        Block::Text { text } => {
                            let text = clamp(&text);
                            if text.is_empty() {
                                continue;
                            }
                            if is_user {
                                push(EventPayload::UserPrompted { text });
                            } else {
                                push(EventPayload::AgentMessage {
                                    text,
                                    is_reasoning: false,
                                    parent_tool_use_id: None,
                                });
                            }
                        }
                        Block::Thinking { thinking } => {
                            let text = clamp(&thinking);
                            if text.is_empty() {
                                continue;
                            }
                            // Marked, not discarded — and marked is what keeps it
                            // out of prompt search, where it would bury what the
                            // person actually wrote.
                            push(EventPayload::AgentMessage {
                                text,
                                is_reasoning: true,
                                parent_tool_use_id: None,
                            });
                        }
                        Block::ToolUse { id, name, input } => {
                            // A file edit is worth its own event: Review and the
                            // Deck key off `file.changed`, and a tool row alone
                            // would not reach them.
                            if let Some(change) = file_change(&name, &input) {
                                push(EventPayload::FileChanged { change });
                            }
                            // A shell call is a command, and a command should become a
                            // Block — searchable and bookmarkable with the rest of the
                            // user's history rather than only a tool row here.
                            if let Some(command) = shell_command(&name, &input) {
                                shell_calls.started(id.clone(), command.clone());
                                push(EventPayload::CommandStarted {
                                    command,
                                    block_id: None,
                                });
                            }
                            push(EventPayload::ToolRequested {
                                tool_use_id: id,
                                input_summary: summarise_input(&name, &input),
                                tool_name: name,
                                parent_tool_use_id: None,
                            });
                        }
                        Block::ToolResult {
                            tool_use_id,
                            is_error,
                            content,
                        } => {
                            let id = tool_use_id.unwrap_or_default();

                            // A result for a shell call closes that command. The output
                            // comes from `toolUseResult` when the line carries it, which
                            // has stdout and stderr apart, and falls back to the block's
                            // own content when it does not.
                            if let Some(command) = shell_calls.finish(&id) {
                                let (stdout, stderr, interrupted) = match &self.tool_use_result {
                                    Some(r) => (
                                        r.stdout.clone().unwrap_or_default(),
                                        r.stderr.clone().unwrap_or_default(),
                                        r.interrupted,
                                    ),
                                    None => (flatten_text(&content), String::new(), false),
                                };

                                for (stream, text) in [
                                    (tervin_core::events::OutputStream::Stdout, &stdout),
                                    (tervin_core::events::OutputStream::Stderr, &stderr),
                                ] {
                                    if !text.trim().is_empty() {
                                        push(EventPayload::CommandOutput {
                                            stream,
                                            excerpt: clamp(text),
                                            block_id: None,
                                        });
                                    }
                                }

                                push(EventPayload::CommandCompleted {
                                    command,
                                    // Derived, not measured. The transcript records
                                    // whether the call failed, never a status — which is
                                    // why the flag below is false and the Block shows no
                                    // number.
                                    exit_code: if interrupted {
                                        130
                                    } else if is_error {
                                        1
                                    } else {
                                        0
                                    },
                                    duration_ms: 0,
                                    exit_code_reported: false,
                                    block_id: None,
                                });
                            }

                            push(EventPayload::ToolCompleted {
                                tool_use_id: id,
                                // The transcript does not repeat the tool's name on the
                                // result, and guessing it from the id would be a lie.
                                tool_name: String::new(),
                                is_error,
                                output_summary: clamp(&flatten_text(&content)),
                                duration_ms: None,
                            });
                        }
                        Block::Other => {}
                    }
                }
            }
            Content::Unknown(_) | Content::Absent => {}
        }

        // A conversational turn that produced nothing is the interesting case: it
        // means the content shape changed under us.
        let understood = out.len() > before;

        // `model` is recorded on the assistant turn; it is the one field that says
        // which model actually answered, which can differ from what was requested.
        if is_assistant {
            if let Some(model) = message.model {
                tracing::trace!("transcript turn answered by {model}");
            }
        }

        understood
    }
}

/// The command a shell tool call ran, if this is one.
///
/// Only `Bash`: a call that reads a file or searches is not a command, and turning every
/// tool call into a Block would fill the Blocks list with rows that have no command line.
fn shell_command(tool: &str, input: &serde_json::Value) -> Option<String> {
    if tool != "Bash" {
        return None;
    }
    let command = input.get("command")?.as_str()?.trim();
    if command.is_empty() {
        return None;
    }
    Some(command.to_string())
}

/// Recognise the edit tools, so a change to a file becomes a `file.changed`.
fn file_change(tool: &str, input: &serde_json::Value) -> Option<FileChange> {
    let path = input.get("file_path")?.as_str()?.to_string();
    let kind = match tool {
        "Write" => FileChangeKind::Created,
        "Edit" | "NotebookEdit" => FileChangeKind::Modified,
        _ => return None,
    };
    Some(FileChange {
        path,
        kind,
        added_lines: None,
        removed_lines: None,
    })
}

/// One readable line describing a tool call.
///
/// Deliberately not a JSON dump: a timeline row has to be scannable, and `Read` of
/// a path is the whole story for most calls.
fn summarise_input(tool: &str, input: &serde_json::Value) -> String {
    let field = |key: &str| input.get(key).and_then(|v| v.as_str());

    let detail = field("file_path")
        .or_else(|| field("path"))
        .or_else(|| field("command"))
        .or_else(|| field("pattern"))
        .or_else(|| field("query"))
        .or_else(|| field("prompt"))
        .or_else(|| field("url"));

    match detail {
        Some(text) => {
            let one_line = text.split('\n').next().unwrap_or(text).trim();
            let shown: String = one_line.chars().take(160).collect();
            if shown.len() < one_line.len() {
                format!("{tool} {shown}…")
            } else {
                format!("{tool} {shown}")
            }
        }
        None => tool.to_string(),
    }
}

/// Pull text out of a tool result, which may be a string or a block list.
fn flatten_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Trim, and cut over-long text at a character boundary with a visible marker.
fn clamp(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= MAX_TEXT {
        return trimmed.to_string();
    }
    let mut end = MAX_TEXT;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… truncated", &trimmed[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Lines copied from a transcript Claude Code 2.1.220 actually wrote, rather
    /// than composed from the shape the parser expects.
    const REAL_USER: &str = r#"{"parentUuid":null,"isSidechain":false,"userType":"external","cwd":"/tmp/scratch","sessionId":"0ab9b986","version":"2.1.220","gitBranch":"main","type":"user","message":{"role":"user","content":"reply with the single word: ok"},"uuid":"u1","timestamp":"2026-08-02T15:29:00.000Z"}"#;

    const REAL_ASSISTANT: &str = r#"{"parentUuid":"u1","isSidechain":false,"cwd":"/tmp/scratch","sessionId":"0ab9b986","type":"assistant","message":{"model":"claude-opus-5","role":"assistant","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":2,"output_tokens":4}},"uuid":"a1","timestamp":"2026-08-02T15:29:02.000Z"}"#;

    /// The entry types that make up most of a real transcript and none of its
    /// story: queue bookkeeping, attachments, prompt records.
    const NOISE: &str = r#"{"type":"queue-operation","operation":"add","sessionId":"s","timestamp":"2026-08-02T15:29:00.000Z"}
{"type":"attachment","attachment":{"type":"file"},"sessionId":"s","timestamp":"2026-08-02T15:29:00.000Z"}
{"type":"last-prompt","sessionId":"s","timestamp":"2026-08-02T15:29:00.000Z"}"#;

    fn write(dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn texts(entries: &[TranscriptEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match &e.payload {
                EventPayload::UserPrompted { text } => format!("user: {text}"),
                EventPayload::AgentMessage {
                    text,
                    is_reasoning: true,
                    ..
                } => format!("thinking: {text}"),
                EventPayload::AgentMessage { text, .. } => format!("agent: {text}"),
                EventPayload::ToolRequested { input_summary, .. } => {
                    format!("tool: {input_summary}")
                }
                EventPayload::ToolCompleted { is_error, .. } => format!("result: error={is_error}"),
                EventPayload::FileChanged { change } => format!("changed: {}", change.path),
                other => format!("other: {}", other.kind()),
            })
            .collect()
    }

    #[test]
    fn reads_a_real_exchange() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "s.jsonl", &format!("{REAL_USER}\n{REAL_ASSISTANT}\n"));

        let mut reader = TranscriptReader::new(&path);
        let entries = reader.read_new().unwrap();

        assert_eq!(
            texts(&entries),
            vec!["user: reply with the single word: ok", "agent: ok"]
        );
        assert_eq!(
            entries[0].ts.as_deref(),
            Some("2026-08-02T15:29:00.000Z"),
            "the agent's own timestamp is kept, not the time we happened to read it"
        );
        assert_eq!(reader.unrecognised(), 0);
    }

    #[test]
    fn skips_bookkeeping_entries_without_counting_them_as_broken() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "s.jsonl", &format!("{NOISE}\n{REAL_USER}\n"));

        let mut reader = TranscriptReader::new(&path);
        let entries = reader.read_new().unwrap();

        // Only the conversation is a Thread's story.
        assert_eq!(
            texts(&entries),
            vec!["user: reply with the single word: ok"]
        );
        // They parsed fine and were intentionally not events, so flagging them
        // would make the unrecognised count useless as a signal.
        assert_eq!(reader.unrecognised(), 0);
    }

    #[test]
    fn does_not_consume_a_line_the_agent_is_still_writing() {
        let dir = tempfile::tempdir().unwrap();
        // Split a real line in half, so the tail is genuinely mid-JSON.
        let cut = REAL_ASSISTANT.len() / 2;
        let (head, tail) = REAL_ASSISTANT.split_at(cut);
        let path = write(&dir, "s.jsonl", &format!("{REAL_USER}\n{head}"));

        let mut reader = TranscriptReader::new(&path);
        assert_eq!(
            texts(&reader.read_new().unwrap()),
            vec!["user: reply with the single word: ok"]
        );
        // Nothing broken yet: the half-written line was left alone, not counted as
        // garbage. Counting it would have meant discarding it.
        assert_eq!(reader.unrecognised(), 0);

        // The agent finishes the line.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{tail}").unwrap();
        drop(file);

        // The partial line is now whole; nothing was lost and nothing was doubled.
        let second = reader.read_new().unwrap();
        assert!(
            second.iter().any(|e| matches!(
                &e.payload,
                EventPayload::AgentMessage { text, .. } if text == "ok"
            )),
            "got {:?}",
            texts(&second)
        );
    }

    #[test]
    fn reads_only_what_is_new() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "s.jsonl", &format!("{REAL_USER}\n"));

        let mut reader = TranscriptReader::new(&path);
        assert_eq!(reader.read_new().unwrap().len(), 1);
        // Re-reading an unchanged file must produce nothing, or every turn would
        // duplicate the whole conversation.
        assert_eq!(reader.read_new().unwrap().len(), 0);

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{REAL_ASSISTANT}").unwrap();
        drop(file);

        assert_eq!(texts(&reader.read_new().unwrap()), vec!["agent: ok"]);
    }

    #[test]
    fn from_end_ignores_a_conversation_that_predates_us() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "s.jsonl", &format!("{REAL_USER}\n{REAL_ASSISTANT}\n"));

        // Replaying an old conversation would put stale rows in a live timeline.
        let mut reader = TranscriptReader::from_end(&path);
        assert_eq!(reader.read_new().unwrap().len(), 0);

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{REAL_USER}").unwrap();
        drop(file);
        assert_eq!(reader.read_new().unwrap().len(), 1);
    }

    #[test]
    fn a_replaced_file_is_read_from_the_start_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "s.jsonl", &format!("{REAL_USER}\n{REAL_ASSISTANT}\n"));
        let mut reader = TranscriptReader::new(&path);
        assert_eq!(reader.read_new().unwrap().len(), 2);

        // Shorter than before — a `/clear`, or the path reused. Seeking to the old
        // offset would land mid-line and read nothing ever again.
        std::fs::write(&path, format!("{REAL_USER}\n")).unwrap();
        assert_eq!(
            texts(&reader.read_new().unwrap()),
            vec!["user: reply with the single word: ok"]
        );
    }

    #[test]
    fn one_broken_line_does_not_cost_the_rest_of_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "s.jsonl",
            &format!("{REAL_USER}\nnot json at all\n{REAL_ASSISTANT}\n"),
        );

        let mut reader = TranscriptReader::new(&path);
        let entries = reader.read_new().unwrap();
        assert_eq!(
            texts(&entries),
            vec!["user: reply with the single word: ok", "agent: ok"]
        );
        // Counted, so a spike after an agent update is visible rather than silent.
        assert_eq!(reader.unrecognised(), 1);
    }

    #[test]
    fn reasoning_is_marked_so_it_stays_out_of_prompt_search() {
        let dir = tempfile::tempdir().unwrap();
        let line = r#"{"type":"assistant","isSidechain":false,"message":{"role":"assistant","model":"m","content":[{"type":"thinking","thinking":"weighing two approaches"},{"type":"text","text":"here is the answer"}]},"timestamp":"2026-08-02T15:29:02.000Z"}"#;
        let path = write(&dir, "s.jsonl", &format!("{line}\n"));

        let entries = TranscriptReader::new(&path).read_new().unwrap();
        assert_eq!(
            texts(&entries),
            vec![
                "thinking: weighing two approaches",
                "agent: here is the answer"
            ]
        );
    }

    #[test]
    fn a_tool_call_becomes_a_readable_row_and_an_edit_becomes_a_file_change() {
        let dir = tempfile::tempdir().unwrap();
        // One line, as the agent writes it. The `\n` inside the command is escaped
        // JSON, so it is part of the string rather than a line break.
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test --workspace\nsecond line"}},{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"/proj/src/main.rs","old_string":"a","new_string":"b"}},{"type":"tool_use","id":"t3","name":"Read","input":{}}]},"timestamp":"2026-08-02T15:29:02.000Z"}"#;
        let path = write(&dir, "s.jsonl", &format!("{line}\n"));

        let entries = TranscriptReader::new(&path).read_new().unwrap();
        let rows = texts(&entries);
        // Only the first line of a multi-line command, so a row stays a row.
        assert!(
            rows.contains(&"tool: Bash cargo test --workspace".to_string()),
            "{rows:?}"
        );
        // An edit reaches Review and the Deck, which key off file.changed.
        assert!(
            rows.contains(&"changed: /proj/src/main.rs".to_string()),
            "{rows:?}"
        );
        // A call with no recognisable argument still names the tool.
        assert!(rows.contains(&"tool: Read".to_string()), "{rows:?}");
    }

    #[test]
    fn a_shell_call_becomes_a_command_that_starts_and_finishes() {
        // The point: a command an agent ran in a pane should become a Block, which needs a
        // start and a completion rather than a single "a tool ran" row.
        let dir = tempfile::tempdir().unwrap();
        let call = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test --workspace"}}]},"timestamp":"2026-08-02T15:29:00.000Z"}"#;
        // `toolUseResult` sits on the line and keeps stdout and stderr apart.
        let result = r#"{"type":"user","toolUseResult":{"stdout":"test result: ok. 470 passed","stderr":"","interrupted":false},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","is_error":false,"content":"test result: ok. 470 passed"}]},"timestamp":"2026-08-02T15:29:30.000Z"}"#;
        let path = write(&dir, "s.jsonl", &format!("{call}\n{result}\n"));

        let entries = TranscriptReader::new(&path).read_new().unwrap();
        let kinds: Vec<&str> = entries.iter().map(|e| e.payload.kind()).collect();

        assert!(kinds.contains(&"command.started"), "{kinds:?}");
        assert!(kinds.contains(&"command.output"), "{kinds:?}");
        assert!(kinds.contains(&"command.completed"), "{kinds:?}");

        let started = entries
            .iter()
            .find_map(|e| match &e.payload {
                EventPayload::CommandStarted { command, .. } => Some(command.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(started, "cargo test --workspace");

        match entries
            .iter()
            .find(|e| matches!(e.payload, EventPayload::CommandCompleted { .. }))
            .map(|e| &e.payload)
        {
            Some(EventPayload::CommandCompleted {
                exit_code,
                exit_code_reported,
                ..
            }) => {
                // The transcript records whether the call failed, never a status. Marking
                // the code as unreported is what keeps it off the Block.
                assert!(
                    !exit_code_reported,
                    "a derived exit code was marked as reported"
                );
                assert_eq!(*exit_code, 0);
            }
            other => panic!("expected a completion, got {other:?}"),
        }
    }

    #[test]
    fn a_failed_shell_call_and_an_interrupted_one_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let call = |id: &str| {
            format!(
                r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"{id}","name":"Bash","input":{{"command":"cargo test"}}}}]}},"timestamp":"2026-08-02T15:29:00.000Z"}}"#
            )
        };
        let failed = r#"{"type":"user","toolUseResult":{"stdout":"","stderr":"error: 2 failed","interrupted":false},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"failed"}]},"timestamp":"2026-08-02T15:29:30.000Z"}"#;
        let stopped = r#"{"type":"user","toolUseResult":{"stdout":"","stderr":"","interrupted":true},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","is_error":true,"content":"interrupted"}]},"timestamp":"2026-08-02T15:29:40.000Z"}"#;
        let path = write(
            &dir,
            "s.jsonl",
            &format!("{}\n{failed}\n{}\n{stopped}\n", call("t1"), call("t2")),
        );

        let entries = TranscriptReader::new(&path).read_new().unwrap();
        let codes: Vec<i32> = entries
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::CommandCompleted { exit_code, .. } => Some(*exit_code),
                _ => None,
            })
            .collect();
        // 1 for a failure, 130 for a stop — a distinction the Block turns into "Failed"
        // and "Interrupted" without ever showing the number.
        assert_eq!(codes, vec![1, 130]);

        // Stderr reaches the Block, which is where a test failure actually is.
        assert!(entries.iter().any(|e| matches!(
            &e.payload,
            EventPayload::CommandOutput { excerpt, .. } if excerpt.contains("2 failed")
        )));
    }

    #[test]
    fn a_tool_that_is_not_a_shell_call_produces_no_command() {
        // Turning every tool call into a Block would fill the Blocks list with rows that
        // have no command line.
        let dir = tempfile::tempdir().unwrap();
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/proj/src/main.rs"}}]},"timestamp":"2026-08-02T15:29:00.000Z"}"#;
        let path = write(&dir, "s.jsonl", &format!("{line}\n"));

        let kinds: Vec<&str> = TranscriptReader::new(&path)
            .read_new()
            .unwrap()
            .iter()
            .map(|e| e.payload.kind())
            .collect();
        assert!(!kinds.contains(&"command.started"), "{kinds:?}");
        assert!(kinds.contains(&"tool.requested"), "{kinds:?}");
    }

    #[test]
    fn a_result_pairs_with_its_own_call_even_out_of_order() {
        // Two calls in flight, resolved in the reverse order. Pairing by position would
        // attribute each command's output to the other one.
        let dir = tempfile::tempdir().unwrap();
        let calls = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"a","name":"Bash","input":{"command":"first-command"}},{"type":"tool_use","id":"b","name":"Bash","input":{"command":"second-command"}}]},"timestamp":"2026-08-02T15:29:00.000Z"}"#;
        let result_b = r#"{"type":"user","toolUseResult":{"stdout":"from second","stderr":"","interrupted":false},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"b","content":"ok"}]},"timestamp":"2026-08-02T15:29:10.000Z"}"#;
        let path = write(&dir, "s.jsonl", &format!("{calls}\n{result_b}\n"));

        let entries = TranscriptReader::new(&path).read_new().unwrap();
        let completed: Vec<String> = entries
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::CommandCompleted { command, .. } => Some(command.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(completed, vec!["second-command".to_string()]);
    }

    #[test]
    fn a_result_for_a_call_never_seen_is_not_turned_into_a_command() {
        // Reading a transcript from the middle gives results whose requests are behind the
        // offset. Inventing a command for them would produce a Block with no command line.
        let dir = tempfile::tempdir().unwrap();
        let orphan = r#"{"type":"user","toolUseResult":{"stdout":"out","stderr":"","interrupted":false},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"never-seen","content":"ok"}]},"timestamp":"2026-08-02T15:29:10.000Z"}"#;
        let path = write(&dir, "s.jsonl", &format!("{orphan}\n"));

        let kinds: Vec<&str> = TranscriptReader::new(&path)
            .read_new()
            .unwrap()
            .iter()
            .map(|e| e.payload.kind())
            .collect();
        assert!(!kinds.contains(&"command.completed"), "{kinds:?}");
        // The tool row still appears — something did happen.
        assert!(kinds.contains(&"tool.completed"), "{kinds:?}");
    }

    #[test]
    fn unresolved_shell_calls_do_not_accumulate_without_bound() {
        // A session where results are never seen must not grow a set of ids forever.
        let mut calls = ShellCalls::default();
        for i in 0..ShellCalls::MAX + 50 {
            calls.started(format!("id{i}"), format!("command {i}"));
        }
        assert_eq!(calls.pending.len(), ShellCalls::MAX);
        // The oldest were dropped, the newest kept — a result is far likelier to arrive
        // for a recent call.
        assert!(calls.finish("id0").is_none());
        assert!(calls
            .finish(&format!("id{}", ShellCalls::MAX + 49))
            .is_some());
    }

    #[test]
    fn a_subagent_turn_is_marked_as_one() {
        let dir = tempfile::tempdir().unwrap();
        let line = r#"{"type":"assistant","isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"from a subagent"}]},"timestamp":"2026-08-02T15:29:02.000Z"}"#;
        let path = write(&dir, "s.jsonl", &format!("{line}\n"));

        let entries = TranscriptReader::new(&path).read_new().unwrap();
        assert!(entries[0].sidechain);
    }

    #[test]
    fn over_long_text_is_truncated_visibly_at_a_character_boundary() {
        let dir = tempfile::tempdir().unwrap();
        // Multi-byte, so a naive byte cut would panic rather than truncate.
        let huge = "é".repeat(MAX_TEXT);
        let line = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": huge },
            "timestamp": "2026-08-02T15:29:00.000Z"
        });
        let path = write(&dir, "s.jsonl", &format!("{line}\n"));

        let entries = TranscriptReader::new(&path).read_new().unwrap();
        match &entries[0].payload {
            EventPayload::UserPrompted { text } => {
                assert!(text.ends_with("… truncated"), "not marked as cut");
                assert!(text.len() <= MAX_TEXT + 16);
            }
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_transcript_is_an_error_the_caller_can_ignore() {
        let dir = tempfile::tempdir().unwrap();
        // The path is announced by the agent; the file may be gone by the time we
        // look. That is not worth a warning, let alone a panic.
        let mut reader = TranscriptReader::new(dir.path().join("absent.jsonl"));
        assert!(reader.read_new().is_err());
    }
}
