//! Turning a byte stream plus shell signals into Blocks.
//!
//! One builder per pane. It consumes coalesced PTY chunks, splits them on signal
//! boundaries so marker bytes never land in captured output, and emits Blocks as
//! they start and finish.
//!
//! Without shell integration the builder simply never opens a Block, and the pane
//! behaves as a continuous terminal. That is the honest fallback: inferring
//! command boundaries by scraping prompts guesses wrong on multi-line prompts,
//! right-hand prompts, and reflow, and a wrong boundary is worse than none.

use crate::model::{
    Block, BlockOutput, BlockStatus, GitContext, DEFAULT_MAX_SPILL, MAX_INLINE_OUTPUT,
};
use crate::parse;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;
use terminal_core::{AgentActivity, CommandMeta, PendingMarker, PtyChunk, ShellSignal};
use tervin_core::{BlockId, PaneId, SessionId, ThreadId};

/// What the builder produces as it observes a pane.
#[derive(Debug, Clone)]
pub enum BlockEvent {
    /// A command started. The Block is `Running` and has no output yet.
    Started(Block),
    /// More output arrived for the open Block.
    Progress { block_id: BlockId, total_bytes: u64 },
    /// A command finished. This Block is complete and ready to persist.
    Finished(Block),
    /// The working directory changed, which the status rail reflects even when
    /// no command is running.
    CwdChanged { cwd: String, host: Option<String> },
    /// A program asked to write the system clipboard via OSC 52.
    ///
    /// Surfaced rather than performed: honouring it blindly would let anything
    /// running — including a process on a remote host — take the local clipboard.
    ClipboardRequested { selection: String, bytes: Vec<u8> },
    /// An agent running inside the pane reported its own lifecycle.
    ///
    /// Carried through the Block engine because it arrives in the same byte stream
    /// and the builder is what already sees every signal — but it is not a Block.
    /// The pane id travels with it so the app layer can say *which* pane an agent
    /// is working in, which is the only reason to surface it at all.
    AgentActivity {
        pane_id: PaneId,
        activity: AgentActivity,
    },
    /// A program asked for a desktop notification via OSC 777.
    ///
    /// Surfaced, never raised here: a long build finishing is worth a banner, and
    /// a process deciding that for itself is not.
    NotificationRequested {
        pane_id: PaneId,
        title: String,
        body: String,
    },
}

/// Where the builder is in the prompt/command cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// No integration seen yet, or between sessions. Output is not captured.
    Unsynced,
    /// A prompt is displayed; the user is typing.
    AtPrompt,
    /// A command is running; output is being captured.
    Executing,
}

/// Output accumulation for the Block currently running.
struct InFlight {
    block: Block,
    started: Instant,
    inline: Vec<u8>,
    /// Opened lazily once output outgrows the inline cap.
    spill: Option<(PathBuf, File)>,
    written: u64,
    truncated: bool,
}

/// Builds Blocks for one pane.
pub struct BlockBuilder {
    pane_id: PaneId,
    session_id: SessionId,
    host: String,
    cwd: String,
    shell: Option<String>,
    project: Option<String>,
    git: GitContext,
    thread_id: Option<ThreadId>,

    phase: Phase,
    /// Metadata from the most recent OSC 7373, consumed by the next command start.
    pending_meta: Option<CommandMeta>,
    current: Option<InFlight>,
    /// True while a full-screen program owns the screen.
    ///
    /// Output is then a stream of redraws, not a command's results, so it is
    /// counted but not stored — see `append_output`.
    alternate_screen: bool,
    /// Bytes produced while the alternate screen was active, so the Block can say
    /// what happened instead of appearing empty.
    alt_screen_bytes: u64,
    /// Trailing bytes of an escape sequence that a chunk cut in half.
    ///
    /// Held back rather than captured: if the next chunk completes a sequence we
    /// recognise, these bytes were marker and must be dropped; if it turns out to
    /// be something else, they were real output and get flushed.
    marker_carry: Vec<u8>,

    spill_dir: PathBuf,
    max_spill: u64,
}

impl BlockBuilder {
    pub fn new(
        pane_id: PaneId,
        session_id: SessionId,
        cwd: impl Into<String>,
        spill_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            pane_id,
            session_id,
            host: "local".to_string(),
            cwd: cwd.into(),
            shell: None,
            project: None,
            git: GitContext::default(),
            thread_id: None,
            phase: Phase::Unsynced,
            pending_meta: None,
            current: None,
            alternate_screen: false,
            alt_screen_bytes: 0,
            marker_carry: Vec::new(),
            spill_dir: spill_dir.into(),
            max_spill: DEFAULT_MAX_SPILL,
        }
    }

    /// Attribute Blocks from this pane to an agent Thread.
    pub fn set_thread(&mut self, thread_id: Option<ThreadId>) {
        self.thread_id = thread_id;
    }

    /// Update cached Git context, resolved off the prompt path by `git-service`.
    pub fn set_git(&mut self, git: GitContext) {
        self.git = git;
    }

    pub fn set_project(&mut self, project: Option<String>) {
        self.project = project;
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// True once shell integration has been observed on this pane.
    pub fn is_synced(&self) -> bool {
        self.phase != Phase::Unsynced
    }

    pub fn open_block_id(&self) -> Option<BlockId> {
        self.current.as_ref().map(|f| f.block.id.clone())
    }

    /// Consume one coalesced chunk of terminal output.
    pub fn consume(&mut self, chunk: &PtyChunk) -> Vec<BlockEvent> {
        let mut events = Vec::new();
        let len = chunk.bytes.len();
        let mut cursor = 0usize;
        // The chunk carries the state at its end, which is what the next chunk
        // will be governed by. Mid-chunk transitions are handled by the offsets
        // below.
        self.alternate_screen = chunk.alternate_screen;

        for sig in &chunk.signals {
            match sig.start {
                Some(start) => {
                    // This marker began inside this chunk, so anything carried
                    // from the previous chunk completed as a sequence we do not
                    // consume: those bytes were output after all.
                    self.flush_carry(&mut events);
                    let boundary = start.max(cursor).min(len);
                    if boundary > cursor {
                        self.append_output(&chunk.bytes[cursor..boundary], &mut events);
                    }
                }
                None => {
                    // The marker began in an earlier chunk; the carried bytes are
                    // its head and must not be captured.
                    self.marker_carry.clear();
                }
            }
            cursor = sig.end.min(len);
            self.handle_signal(&sig.signal, &mut events);
        }

        // Split the tail on whether it ends mid-sequence.
        match chunk.pending_marker {
            PendingMarker::StartedAt(start) if start >= cursor => {
                self.flush_carry(&mut events);
                if start > cursor {
                    self.append_output(&chunk.bytes[cursor..start], &mut events);
                }
                self.marker_carry.extend_from_slice(&chunk.bytes[start..]);
            }
            PendingMarker::Earlier => {
                // The whole remaining tail continues a sequence already open.
                self.marker_carry.extend_from_slice(&chunk.bytes[cursor..]);
            }
            _ => {
                self.flush_carry(&mut events);
                if cursor < len {
                    self.append_output(&chunk.bytes[cursor..], &mut events);
                }
            }
        }

        events
    }

    /// Emit carried bytes as ordinary output, having learned they were not part
    /// of a marker Tervin consumes.
    fn flush_carry(&mut self, events: &mut Vec<BlockEvent>) {
        if self.marker_carry.is_empty() {
            return;
        }
        let carried = std::mem::take(&mut self.marker_carry);
        self.append_output(&carried, events);
    }

    /// The pane's process exited. Close any open Block rather than leaving it
    /// running forever.
    pub fn on_session_end(&mut self, exit_code: Option<i32>) -> Vec<BlockEvent> {
        let mut events = Vec::new();
        if self.current.is_some() {
            self.finish_command(exit_code, &mut events);
        }
        self.phase = Phase::Unsynced;
        events
    }

    fn handle_signal(&mut self, signal: &ShellSignal, events: &mut Vec<BlockEvent>) {
        match signal {
            ShellSignal::PromptStart => {
                // A prompt while a command is open means we missed the completion
                // mark; close it as Unknown rather than leaving it running.
                if self.current.is_some() {
                    self.finish_command(None, events);
                }
                self.phase = Phase::AtPrompt;
            }

            ShellSignal::PromptEnd => {
                self.phase = Phase::AtPrompt;
            }

            ShellSignal::CommandExecuted => {
                self.start_command(events);
            }

            ShellSignal::CommandFinished { exit_code } => {
                if self.current.is_some() {
                    self.finish_command(*exit_code, events);
                }
                self.phase = Phase::AtPrompt;
            }

            ShellSignal::Cwd { host, path } => {
                self.cwd = path.clone();
                if let Some(h) = host {
                    self.host = h.clone();
                }
                if self.phase == Phase::Unsynced {
                    self.phase = Phase::AtPrompt;
                }
                events.push(BlockEvent::CwdChanged {
                    cwd: path.clone(),
                    host: host.clone(),
                });
            }

            ShellSignal::Meta { meta } => {
                if let Some(shell) = &meta.shell {
                    self.shell = Some(shell.clone());
                }
                if let Some(branch) = &meta.git_branch {
                    self.git.branch = Some(branch.clone());
                }
                if let Some(dirty) = meta.git_dirty {
                    self.git.dirty = Some(dirty);
                }
                // Merge rather than replace: the command arrives in one OSC and
                // git context may arrive in another.
                match &mut self.pending_meta {
                    Some(existing) => {
                        if meta.command.is_some() {
                            existing.command = meta.command.clone();
                        }
                        if meta.duration_ms.is_some() {
                            existing.duration_ms = meta.duration_ms;
                        }
                        if meta.exit_code.is_some() {
                            existing.exit_code = meta.exit_code;
                        }
                    }
                    None => self.pending_meta = Some(meta.clone()),
                }
                if self.phase == Phase::Unsynced {
                    self.phase = Phase::AtPrompt;
                }
            }

            ShellSignal::ClipboardWriteRequested { selection, bytes } => {
                events.push(BlockEvent::ClipboardRequested {
                    selection: selection.clone(),
                    bytes: bytes.clone(),
                });
            }

            ShellSignal::AgentActivity { activity } => {
                // Deliberately does not touch Block state. An agent's TUI runs as
                // one long-lived command, and treating a prompt as a command
                // boundary would chop that Block into fragments while the agent is
                // still running.
                events.push(BlockEvent::AgentActivity {
                    pane_id: self.pane_id.clone(),
                    activity: activity.clone(),
                });
            }

            ShellSignal::Notification { title, body } => {
                events.push(BlockEvent::NotificationRequested {
                    pane_id: self.pane_id.clone(),
                    title: title.clone(),
                    body: body.clone(),
                });
            }

            // Titles are the shell's business, not a Block's.
            ShellSignal::Title { .. } | ShellSignal::Hyperlink { .. } => {}
        }
    }

    fn start_command(&mut self, events: &mut Vec<BlockEvent>) {
        // A second execute mark without a completion mark: close the previous.
        if self.current.is_some() {
            self.finish_command(None, events);
        }

        let meta = self.pending_meta.take().unwrap_or_default();
        let command = meta.command.unwrap_or_default();

        let mut block = Block::new(
            self.pane_id.clone(),
            self.session_id.clone(),
            command,
            self.cwd.clone(),
            self.host.clone(),
        );
        block.shell = self.shell.clone();
        block.project = self.project.clone();
        block.git = self.git.clone();
        block.thread_id = self.thread_id.clone();

        self.phase = Phase::Executing;
        events.push(BlockEvent::Started(block.clone()));
        self.current = Some(InFlight {
            block,
            started: Instant::now(),
            inline: Vec::with_capacity(8 * 1024),
            spill: None,
            written: 0,
            truncated: false,
        });
    }

    fn append_output(&mut self, bytes: &[u8], events: &mut Vec<BlockEvent>) {
        if bytes.is_empty() {
            return;
        }
        // Outside a command there is nothing to capture: prompts, and the shell's
        // own redraws, are not Block content.
        let Some(flight) = self.current.as_mut() else {
            return;
        };

        // A full-screen program (vim, less, htop) repaints the whole screen
        // continuously. Storing that would fill a Block with cursor movement and
        // bury whatever the command actually produced, so it is counted and
        // summarised instead. The renderer still receives every byte.
        if self.alternate_screen {
            self.alt_screen_bytes += bytes.len() as u64;
            return;
        }

        flight.written += bytes.len() as u64;

        // Fill the inline buffer first; it is what the row stores and what a
        // collapsed Block renders from.
        if flight.inline.len() < MAX_INLINE_OUTPUT {
            let room = MAX_INLINE_OUTPUT - flight.inline.len();
            let take = room.min(bytes.len());
            flight.inline.extend_from_slice(&bytes[..take]);
        }

        // Everything is mirrored to the spill file once one exists, so the raw
        // output stays complete and available.
        if flight.written > MAX_INLINE_OUTPUT as u64 {
            if flight.spill.is_none() {
                if let Some(opened) = open_spill(&self.spill_dir, &flight.block.id) {
                    let (path, mut file) = opened;
                    // Seed with the inline head so the file is the whole output.
                    let _ = file.write_all(&flight.inline);
                    flight.spill = Some((path, file));
                }
            }
            if let Some((_, file)) = flight.spill.as_mut() {
                if flight.written <= self.max_spill {
                    let _ = file.write_all(bytes);
                } else if !flight.truncated {
                    flight.truncated = true;
                    let _ = file.write_all(
                        b"\n[tervin] output exceeded the per-block capture limit and was truncated here\n",
                    );
                }
            }
        }

        events.push(BlockEvent::Progress {
            block_id: flight.block.id.clone(),
            total_bytes: flight.written,
        });
    }

    fn finish_command(&mut self, exit_code: Option<i32>, events: &mut Vec<BlockEvent>) {
        let Some(mut flight) = self.current.take() else {
            return;
        };

        if let Some((_, file)) = flight.spill.as_mut() {
            let _ = file.flush();
        }

        let duration = flight.started.elapsed();
        let mut block = flight.block;
        block.exit_code = exit_code;
        block.status = BlockStatus::from_exit_code(exit_code);
        block.ended_at = Some(tervin_core::now());
        block.duration_ms = Some(duration.as_millis() as u64);
        block.output = BlockOutput {
            inline: flight.inline,
            spill_path: flight.spill.map(|(p, _)| p),
            total_bytes: flight.written,
            truncated: flight.truncated,
        };
        // Parse the inline head only. It holds the command's first output, which
        // is where compiler errors and served URLs appear; scanning a multi-
        // gigabyte spill on the completion path would stall the UI.
        block.parsed = parse::extract(&block.output.inline_text(), &block.cwd);

        // Say what happened rather than presenting an empty Block.
        if self.alt_screen_bytes > 0 {
            block.note = Some(format!(
                "A full-screen program used this terminal. {} of screen redraws were \
                 not captured as Block output.",
                human_bytes(self.alt_screen_bytes)
            ));
            self.alt_screen_bytes = 0;
        }

        self.phase = Phase::AtPrompt;
        events.push(BlockEvent::Finished(block));
    }
}

/// Round bytes for a human-readable note.
fn human_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn open_spill(dir: &Path, block_id: &BlockId) -> Option<(PathBuf, File)> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("{block_id}.raw"));
    let file = File::create(&path).ok()?;
    Some((path, file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use terminal_core::{OscScanner, PositionedSignal};

    /// Build a chunk the way the PTY pump does, so tests exercise the real
    /// offset arithmetic rather than hand-written offsets.
    fn chunk(pane: &PaneId, bytes: &[u8]) -> PtyChunk {
        let mut scanner = OscScanner::new();
        let signals = scanner
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
        PtyChunk {
            pane_id: pane.clone(),
            bytes: bytes.to_vec(),
            signals,
            pending_marker: scanner.pending_marker(),
            mode_changes: scanner.mode_changes().to_vec(),
            queries: Vec::new(),
            color_scheme_updates: false,
            alternate_screen: false,
        }
    }

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    fn builder() -> (BlockBuilder, PaneId) {
        let pane = PaneId::new();
        let dir = std::env::temp_dir().join(format!("tervin-blocks-{}", uuid::Uuid::new_v4()));
        (
            BlockBuilder::new(pane.clone(), SessionId::new(), "/tmp", dir),
            pane,
        )
    }

    #[test]
    fn builds_a_block_from_a_full_command_cycle() {
        let (mut b, pane) = builder();
        let stream = format!(
            "\x1b]7373;cmd={}\x07\x1b]133;C\x07hello\r\n\x1b]133;D;0\x07",
            b64("echo hello")
        );
        let events = b.consume(&chunk(&pane, stream.as_bytes()));

        let started = events.iter().find_map(|e| match e {
            BlockEvent::Started(b) => Some(b),
            _ => None,
        });
        assert!(started.is_some(), "no Started event: {events:?}");

        let finished = events
            .iter()
            .find_map(|e| match e {
                BlockEvent::Finished(b) => Some(b),
                _ => None,
            })
            .expect("no Finished event");

        assert_eq!(finished.command, "echo hello");
        assert_eq!(finished.exit_code, Some(0));
        assert_eq!(finished.status, BlockStatus::Succeeded);
        // Critically: the output is exactly the command's output, with no
        // marker bytes and no prompt.
        assert_eq!(finished.output.inline, b"hello\r\n");
    }

    #[test]
    fn excludes_marker_bytes_and_prompt_from_output() {
        let (mut b, pane) = builder();
        let stream = format!(
            "\x1b]7373;cmd={}\x07\x1b]133;C\x07out\x1b]133;D;1\x07\x1b]7;file://h/tmp\x07\x1b]133;A\x07user@host $ ",
            b64("false")
        );
        let events = b.consume(&chunk(&pane, stream.as_bytes()));
        let finished = events
            .iter()
            .find_map(|e| match e {
                BlockEvent::Finished(b) => Some(b),
                _ => None,
            })
            .unwrap();

        assert_eq!(finished.output.inline, b"out");
        assert_eq!(finished.status, BlockStatus::Failed);
        // The prompt that followed must not have been captured.
        assert!(!finished.output.inline_text().contains("user@host"));
    }

    #[test]
    fn ignores_output_produced_outside_a_command() {
        // A bare Enter, or the shell redrawing its prompt, must not create or
        // pollute a Block.
        let (mut b, pane) = builder();
        let events = b.consume(&chunk(&pane, b"\x1b]133;A\x07prompt text here"));
        assert!(!events
            .iter()
            .any(|e| matches!(e, BlockEvent::Started(_) | BlockEvent::Finished(_))));
    }

    #[test]
    fn handles_a_cycle_split_across_chunks() {
        // Real PTY reads split anywhere, including mid-escape-sequence.
        let (mut b, pane) = builder();
        let mut scanner = OscScanner::new();
        let full = format!(
            "\x1b]7373;cmd={}\x07\x1b]133;C\x07part-one part-two\x1b]133;D;0\x07",
            b64("ls -la")
        );
        let bytes = full.as_bytes();

        // Split at an awkward point: inside the trailing completion sequence.
        let split = bytes.len() - 5;
        let mut all_events = Vec::new();
        for piece in [&bytes[..split], &bytes[split..]] {
            let signals = scanner
                .feed_indexed(piece)
                .into_iter()
                .filter_map(|hit| {
                    terminal_core::signals::parse(&hit.payload).map(|signal| PositionedSignal {
                        start: hit.start_offset,
                        end: hit.end_offset,
                        signal,
                    })
                })
                .collect();
            all_events.extend(b.consume(&PtyChunk {
                pane_id: pane.clone(),
                bytes: piece.to_vec(),
                signals,
                pending_marker: scanner.pending_marker(),
                mode_changes: scanner.mode_changes().to_vec(),
                queries: Vec::new(),
                color_scheme_updates: false,
                alternate_screen: false,
            }));
        }

        let finished = all_events
            .iter()
            .find_map(|e| match e {
                BlockEvent::Finished(b) => Some(b),
                _ => None,
            })
            .expect("no Finished event across chunk boundary");
        assert_eq!(finished.command, "ls -la");
        assert_eq!(finished.output.inline, b"part-one part-two");
    }

    #[test]
    fn a_missing_completion_mark_closes_the_block_as_unknown() {
        // If the shell dies mid-command we must not leave a Block running.
        let (mut b, pane) = builder();
        b.consume(&chunk(
            &pane,
            format!(
                "\x1b]7373;cmd={}\x07\x1b]133;C\x07working",
                b64("sleep 100")
            )
            .as_bytes(),
        ));
        let events = b.on_session_end(None);
        let finished = events
            .iter()
            .find_map(|e| match e {
                BlockEvent::Finished(b) => Some(b),
                _ => None,
            })
            .unwrap();
        assert_eq!(finished.status, BlockStatus::Unknown);
        assert_eq!(finished.exit_code, None);
    }

    #[test]
    fn a_new_prompt_closes_an_orphaned_block() {
        let (mut b, pane) = builder();
        b.consume(&chunk(
            &pane,
            format!("\x1b]7373;cmd={}\x07\x1b]133;C\x07x", b64("cmd")).as_bytes(),
        ));
        // Prompt start without a completion mark.
        let events = b.consume(&chunk(&pane, b"\x1b]133;A\x07"));
        assert!(events
            .iter()
            .any(|e| matches!(e, BlockEvent::Finished(b) if b.status == BlockStatus::Unknown)));
    }

    #[test]
    fn tracks_cwd_changes_and_stamps_them_on_later_blocks() {
        let (mut b, pane) = builder();
        let events = b.consume(&chunk(&pane, b"\x1b]7;file://mac/Users/dev/proj\x07"));
        assert!(events
            .iter()
            .any(|e| matches!(e, BlockEvent::CwdChanged { cwd, .. } if cwd == "/Users/dev/proj")));
        assert_eq!(b.cwd(), "/Users/dev/proj");

        let events = b.consume(&chunk(
            &pane,
            format!(
                "\x1b]7373;cmd={}\x07\x1b]133;C\x07\x1b]133;D;0\x07",
                b64("pwd")
            )
            .as_bytes(),
        ));
        let finished = events
            .iter()
            .find_map(|e| match e {
                BlockEvent::Finished(b) => Some(b),
                _ => None,
            })
            .unwrap();
        assert_eq!(finished.cwd, "/Users/dev/proj");
        assert_eq!(finished.host, "mac");
    }

    #[test]
    fn surfaces_clipboard_writes_instead_of_performing_them() {
        let (mut b, pane) = builder();
        let events = b.consume(&chunk(&pane, b"\x1b]52;c;c2VjcmV0\x07"));
        assert!(events.iter().any(|e| matches!(
            e,
            BlockEvent::ClipboardRequested { bytes, .. } if bytes == b"secret"
        )));
    }

    #[test]
    fn spills_large_output_to_disk_and_keeps_it_complete() {
        let (mut b, pane) = builder();
        b.consume(&chunk(
            &pane,
            format!("\x1b]7373;cmd={}\x07\x1b]133;C\x07", b64("yes")).as_bytes(),
        ));

        // Push past the inline cap in several chunks.
        let piece = vec![b'A'; 64 * 1024];
        for _ in 0..6 {
            b.consume(&PtyChunk {
                pane_id: pane.clone(),
                bytes: piece.clone(),
                signals: vec![],
                pending_marker: PendingMarker::None,
                mode_changes: vec![],
                queries: Vec::new(),
                color_scheme_updates: false,
                alternate_screen: false,
            });
        }
        let events = b.consume(&chunk(&pane, b"\x1b]133;D;0\x07"));
        let finished = events
            .iter()
            .find_map(|e| match e {
                BlockEvent::Finished(b) => Some(b),
                _ => None,
            })
            .unwrap();

        assert_eq!(finished.output.total_bytes, 6 * 64 * 1024);
        assert_eq!(finished.output.inline.len(), MAX_INLINE_OUTPUT);
        let spill = finished
            .output
            .spill_path
            .clone()
            .expect("expected a spill file");
        let on_disk = std::fs::metadata(&spill).unwrap().len();
        // The spill holds the whole stream, not just the overflow.
        assert_eq!(on_disk, finished.output.total_bytes);
        std::fs::remove_file(spill).ok();
    }

    #[test]
    fn a_full_screen_program_does_not_fill_the_block_with_redraws() {
        // Running `vim` inside a command would otherwise store megabytes of
        // cursor movement as that command's output.
        let (mut b, pane) = builder();
        b.consume(&chunk(
            &pane,
            format!("\x1b]7373;cmd={}\x07\x1b]133;C\x07", b64("vim notes.md")).as_bytes(),
        ));

        // Enter the alternate screen and repaint heavily.
        let redraws = vec![b'\x1b'; 200_000];
        b.consume(&PtyChunk {
            pane_id: pane.clone(),
            bytes: redraws,
            signals: vec![],
            pending_marker: PendingMarker::None,
            mode_changes: vec![],
            queries: Vec::new(),
            color_scheme_updates: false,
            alternate_screen: true,
        });

        // Leave it, then finish the command.
        let events = b.consume(&PtyChunk {
            pane_id: pane.clone(),
            bytes: b"\x1b]133;D;0\x07".to_vec(),
            signals: {
                let mut sc = terminal_core::OscScanner::new();
                sc.feed_indexed(b"\x1b]133;D;0\x07")
                    .into_iter()
                    .filter_map(|hit| {
                        terminal_core::signals::parse(&hit.payload).map(|signal| PositionedSignal {
                            start: hit.start_offset,
                            end: hit.end_offset,
                            signal,
                        })
                    })
                    .collect()
            },
            pending_marker: PendingMarker::None,
            mode_changes: vec![],
            queries: Vec::new(),
            color_scheme_updates: false,
            alternate_screen: false,
        });

        let finished = events
            .iter()
            .find_map(|e| match e {
                BlockEvent::Finished(b) => Some(b),
                _ => None,
            })
            .expect("no Finished event");

        assert_eq!(
            finished.output.total_bytes, 0,
            "redraws must not be captured as Block output"
        );
        // But the Block says what happened rather than looking empty.
        let note = finished.note.as_deref().unwrap_or("");
        assert!(note.contains("full-screen"), "note was {note:?}");
        assert!(
            note.contains("KB") || note.contains("MB"),
            "note was {note:?}"
        );
    }

    #[test]
    fn parses_diagnostics_from_captured_output() {
        let (mut b, pane) = builder();
        let stream = format!(
            "\x1b]7373;cmd={}\x07\x1b]133;C\x07error[E0308]: mismatched types\n  --> src/a.rs:3:9\n\x1b]133;D;101\x07",
            b64("cargo build")
        );
        let events = b.consume(&chunk(&pane, stream.as_bytes()));
        let finished = events
            .iter()
            .find_map(|e| match e {
                BlockEvent::Finished(b) => Some(b),
                _ => None,
            })
            .unwrap();
        assert_eq!(finished.parsed.error_count, 1);
        assert_eq!(finished.parsed.diagnostics[0].line, Some(3));
        assert!(finished.is_notable());
    }
}
