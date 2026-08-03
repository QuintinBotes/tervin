//! PTY sessions.
//!
//! One `PtySession` owns one pseudo-terminal and the process group inside it.
//! Reading is done on a dedicated OS thread because the PTY read is blocking and
//! must never be able to stall the UI.
//!
//! Output takes two paths from a single read: the bytes go to the renderer
//! verbatim, and a tap extracts shell-integration signals. Both arrive in one
//! chunk with signal offsets, so consumers can reconstruct exact ordering.

use crate::osc::TerminalQuery;
use crate::osc::{ModeChange, OscScanner, PendingMarker, PrivateMode};
use crate::signals::{self, ShellSignal};
use parking_lot::Mutex;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tervin_core::PaneId;

/// Flush the coalescing buffer at least this often. Chosen to sit under one
/// frame at 60Hz so interactive echo still feels immediate.
const FLUSH_INTERVAL: Duration = Duration::from_millis(6);

/// Flush early once a batch reaches this size, to bound per-message cost during
/// heavy output such as a build log or `cat` of a large file.
const FLUSH_BYTES: usize = 32 * 1024;

/// Size of each blocking PTY read.
const READ_BUF: usize = 64 * 1024;

/// How often to ask whether the child has taken over the terminal.
const INPUT_GATE_POLL: Duration = Duration::from_millis(5);

/// Longest to hold input waiting for a child that may never take over.
///
/// Bounded because plenty of programs read in canonical mode for their whole life
/// and are perfectly able to receive input. They discard nothing, so gating them
/// would delay real keystrokes for no benefit.
const INPUT_GATE_MAX: Duration = Duration::from_millis(1500);

/// Whether the child has put the terminal into a mode it reads input from itself.
///
/// A line editor — zsh's ZLE, bash's readline — clears `ICANON` as it takes over,
/// and it does so with a `tcsetattr` that discards input already queued. Once the
/// flag is clear that call has happened, so anything written from here on survives.
///
/// The master's termios reports the pty's line discipline, which makes this the
/// child's own answer rather than a guess from elapsed time or from output, both of
/// which say only that a prompt was *printed* — a different and earlier moment.
#[cfg(unix)]
fn accepts_input(fd: std::os::unix::io::RawFd) -> bool {
    // SAFETY: `fd` is the pty master, owned by this session and open for as long as
    // the watcher runs. `tcgetattr` only reads, and only into the local struct.
    unsafe {
        let mut settings: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut settings) != 0 {
            return false;
        }
        settings.c_lflag & libc::ICANON == 0
    }
}

/// Longest a synchronized-output frame may hold back a flush.
///
/// An application that sets DEC 2026 and then blocks — or crashes before
/// clearing it — must not be able to freeze the pane. Past this the batch is
/// released regardless, trading a possible tear for a responsive terminal.
const MAX_SYNC_HOLD: Duration = Duration::from_millis(120);

#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("failed to open pty: {0}")]
    Open(String),
    #[error("failed to spawn `{program}`: {source}")]
    Spawn {
        program: String,
        source: anyhow_lite::Error,
    },
    #[error("pty i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("session {0} is not running")]
    NotRunning(PaneId),
}

/// A tiny error wrapper so this crate does not take an `anyhow` dependency just
/// to carry `portable-pty`'s boxed errors.
pub mod anyhow_lite {
    #[derive(Debug)]
    pub struct Error(pub String);

    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl std::error::Error for Error {}
}

/// What to launch, and how the terminal should present itself.
#[derive(Debug, Clone)]
pub struct PtyConfig {
    pub pane_id: PaneId,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// Extra environment on top of the inherited environment.
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    /// Advertised via `$TERM`.
    pub term: String,
}

impl PtyConfig {
    /// A login shell for the given pane, using the user's own `$SHELL`.
    pub fn login_shell(pane_id: PaneId, cwd: Option<String>) -> Self {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        Self {
            pane_id,
            program: shell,
            // `-l` so the user's real environment, PATH, and prompt apply. A
            // terminal that silently runs a non-login shell surprises people.
            args: vec!["-l".to_string()],
            cwd,
            env: Vec::new(),
            cols: 80,
            rows: 24,
            term: "xterm-256color".to_string(),
        }
    }

    /// An arbitrary managed command — the Tier 3 agent case.
    pub fn command(
        pane_id: PaneId,
        program: impl Into<String>,
        args: Vec<String>,
        cwd: Option<String>,
    ) -> Self {
        Self {
            pane_id,
            program: program.into(),
            args,
            cwd,
            env: Vec::new(),
            cols: 80,
            rows: 24,
            term: "xterm-256color".to_string(),
        }
    }
}

/// A signal and the span of bytes that carried it.
#[derive(Debug, Clone)]
pub struct PositionedSignal {
    /// Offset of the sequence's first byte within `PtyChunk::bytes`, or `None`
    /// when the sequence started in an earlier chunk.
    pub start: Option<usize>,
    /// Offset into `PtyChunk::bytes` one past the sequence that produced it.
    pub end: usize,
    pub signal: ShellSignal,
}

/// One coalesced batch of terminal output.
#[derive(Debug, Clone)]
pub struct PtyChunk {
    pub pane_id: PaneId,
    /// Raw bytes, exactly as the program wrote them. Forward verbatim.
    pub bytes: Vec<u8>,
    /// Signals found in `bytes`, ordered by offset.
    pub signals: Vec<PositionedSignal>,
    /// Set when `bytes` ends part-way through an escape sequence, so a consumer
    /// capturing output can hold those trailing bytes back instead of storing a
    /// fragment of a marker.
    pub pending_marker: PendingMarker,
    /// DEC private mode changes inside `bytes`, in order.
    pub mode_changes: Vec<ModeChange>,
    /// Requests the program made that the terminal is expected to answer.
    ///
    /// Surfaced rather than answered in the pump, for the same reason as an OSC 52
    /// clipboard write: replying is a decision, and the reply goes back as input to the
    /// program — so it belongs where the rest of Tervin's writes are made.
    pub queries: Vec<TerminalQuery>,
    /// Whether the program asked to be told about colour-scheme changes (mode 2031).
    ///
    /// Carried on every chunk rather than derived from `mode_changes`, so a consumer
    /// that starts mid-stream still knows whether this pane wants the report. Without
    /// it, sending one to a shell that never subscribed would put stray text on the
    /// command line.
    pub color_scheme_updates: bool,
    /// Whether the alternate screen is active at the end of this chunk.
    ///
    /// Carried on every chunk rather than derived from `mode_changes`, so a
    /// consumer that starts mid-stream still knows the current state.
    pub alternate_screen: bool,
}

impl PtyChunk {
    /// A chunk of plain output, with no markers, queries or mode changes.
    ///
    /// For tests and for callers that synthesise output. A constructor rather than
    /// `Default`, because a chunk without a pane is not a meaningful value — and every
    /// field added since has meant editing a dozen struct literals.
    pub fn plain(pane_id: PaneId, bytes: Vec<u8>) -> Self {
        Self {
            pane_id,
            bytes,
            signals: Vec::new(),
            pending_marker: PendingMarker::None,
            mode_changes: Vec::new(),
            queries: Vec::new(),
            color_scheme_updates: false,
            alternate_screen: false,
        }
    }
}

/// Everything a session emits.
#[derive(Debug, Clone)]
pub enum PtyEvent {
    Chunk(PtyChunk),
    /// The child exited. `exit_code` is `None` if it could not be determined.
    Exited {
        pane_id: PaneId,
        exit_code: Option<i32>,
    },
}

type EventSink = Arc<dyn Fn(PtyEvent) + Send + Sync>;

/// A live pseudo-terminal and its child process.
pub struct PtySession {
    pane_id: PaneId,
    /// Behind a mutex so the session is `Sync`.
    ///
    /// `MasterPty` is `Send` but not `Sync`, and the session is shared across
    /// threads by the pane registry and every IPC command that touches it.
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// Shared with the readiness watcher, which flushes any gated input.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    alive: Arc<AtomicBool>,
    size: Mutex<(u16, u16)>,
    /// Input written before the child could receive it.
    ///
    /// `Some` while the child is still taking over the terminal, `None` once
    /// anything written goes straight through. See [`PtySession::write`].
    pending_input: Arc<Mutex<Option<Vec<u8>>>>,
}

impl PtySession {
    /// Open a PTY, spawn the program, and start pumping output to `sink`.
    ///
    /// Two threads are started: a reader that does blocking PTY reads, and a
    /// pump that coalesces those reads into batches. Splitting them is what lets
    /// coalescing be time-bounded without ever interrupting a read.
    pub fn spawn(config: PtyConfig, sink: EventSink) -> Result<Self, PtyError> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: config.rows,
                cols: config.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Open(e.to_string()))?;

        let mut cmd = CommandBuilder::new(&config.program);
        for arg in &config.args {
            cmd.arg(arg);
        }
        if let Some(cwd) = &config.cwd {
            cmd.cwd(cwd);
        }

        // Identify ourselves so shell hooks and programs can adapt, and declare
        // truecolor support up front.
        cmd.env("TERM", &config.term);
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Tervin");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        cmd.env("TERVIN_PANE", config.pane_id.as_str());

        // A pane is a fresh shell, not a continuation of whatever started Tervin.
        //
        // These mark a process as living inside an agent session. Tervin inherits
        // them whenever it is launched from one — from a Claude Code session, or
        // from a terminal that was itself started by an agent — and a shell that
        // inherits them is not the shell the user thinks they opened: `claude` run
        // in that pane sees the marker, concludes it is a child session, and stops
        // saving transcripts, which is how this was noticed.
        //
        // The agent runtime already scrubs these for Threads and says why. The same
        // reasoning applies to panes and was simply never applied there.
        for key in [
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_PID",
            "CLAUDE_PARENT_SESSION_ID",
        ] {
            cmd.env_remove(key);
        }

        // The other half of the same problem: the build tool that started Tervin.
        //
        // `pnpm app` exports eighteen of these, and every one reaches the user's
        // shell — `INIT_CWD` pointing at wherever the build was run, an
        // `npm_config_user_agent` claiming this shell is pnpm, `npm_lifecycle_event`
        // saying it is running a script called `app`. A shell that believes it is
        // inside `npm run` is not a shell anyone opened, and tools that check these
        // will behave differently in a Tervin pane than in Terminal for no reason
        // the user can see.
        //
        // Matched by prefix because the set is open-ended and grows with the
        // package manager. `NODE_` is deliberately not swept: `NODE_OPTIONS` and
        // friends are things a user may genuinely set for themselves.
        let inherited: Vec<String> = std::env::vars()
            .map(|(k, _)| k)
            .filter(|k| k.starts_with("npm_") || k.starts_with("PNPM_") || k == "INIT_CWD")
            .collect();
        for key in inherited {
            cmd.env_remove(key);
        }

        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let child = pair.slave.spawn_command(cmd).map_err(|e| PtyError::Spawn {
            program: config.program.clone(),
            source: anyhow_lite::Error(e.to_string()),
        })?;

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::Open(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::Open(e.to_string()))?;

        let alive = Arc::new(AtomicBool::new(true));
        let child = Arc::new(Mutex::new(child));

        // Reader thread: blocking reads, forwarded immediately and unparsed.
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        {
            let alive = alive.clone();
            let pane = config.pane_id.clone();
            std::thread::Builder::new()
                .name(format!("tervin-pty-read-{pane}"))
                .spawn(move || {
                    let mut reader = reader;
                    let mut buf = vec![0u8; READ_BUF];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                if tx.send(buf[..n].to_vec()).is_err() {
                                    break;
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(_) => break,
                        }
                    }
                    alive.store(false, Ordering::SeqCst);
                })
                .map_err(PtyError::Io)?;
        }

        // Pump thread: coalesce reads into batches and run the OSC tap.
        {
            let pane = config.pane_id.clone();
            let child_for_exit = child.clone();
            std::thread::Builder::new()
                .name(format!("tervin-pty-pump-{pane}"))
                .spawn(move || {
                    let mut scanner = OscScanner::new();
                    let mut batch: Vec<u8> = Vec::with_capacity(FLUSH_BYTES);
                    let mut signals: Vec<PositionedSignal> = Vec::new();
                    let mut modes: Vec<ModeChange> = Vec::new();
                    let mut queries: Vec<TerminalQuery> = Vec::new();
                    let mut pending = PendingMarker::None;
                    let mut first_byte_at: Option<Instant> = None;
                    // Screen and repaint state, tracked across the whole session.
                    let mut alternate_screen = false;
                    // Whether this pane asked to be told about colour-scheme changes.
                    let mut color_scheme_updates = false;
                    let mut synchronized = false;
                    let mut sync_started_at: Option<Instant> = None;

                    // Emit whatever has accumulated, if anything.
                    macro_rules! flush {
                        () => {
                            if !batch.is_empty() {
                                sink(PtyEvent::Chunk(PtyChunk {
                                    pane_id: pane.clone(),
                                    bytes: std::mem::take(&mut batch),
                                    signals: std::mem::take(&mut signals),
                                    pending_marker: pending,
                                    mode_changes: std::mem::take(&mut modes),
                                    queries: std::mem::take(&mut queries),
                                    color_scheme_updates,
                                    alternate_screen,
                                }));
                                batch.reserve(FLUSH_BYTES);
                                pending = PendingMarker::None;
                                first_byte_at = None;
                            }
                        };
                    }

                    loop {
                        let timeout = match first_byte_at {
                            Some(start) => FLUSH_INTERVAL.saturating_sub(start.elapsed()),
                            None => FLUSH_INTERVAL,
                        };

                        match rx.recv_timeout(timeout) {
                            Ok(data) => {
                                let base = batch.len();
                                for hit in scanner.feed_indexed(&data) {
                                    if let Some(signal) = signals::parse(&hit.payload) {
                                        signals.push(PositionedSignal {
                                            // Rebase onto the batch: offsets are
                                            // relative to this read, but the
                                            // consumer sees the whole batch.
                                            start: hit.start_offset.map(|s| base + s),
                                            end: base + hit.end_offset,
                                            signal,
                                        });
                                    }
                                }
                                // Rebase the pending-marker offset onto the batch
                                // for the same reason as the signal offsets.
                                pending = match scanner.pending_marker() {
                                    PendingMarker::StartedAt(i) => {
                                        PendingMarker::StartedAt(base + i)
                                    }
                                    other => other,
                                };

                                for change in scanner.mode_changes() {
                                    match change.mode {
                                        PrivateMode::AlternateScreen => {
                                            alternate_screen = change.enabled
                                        }
                                        PrivateMode::SynchronizedOutput => {
                                            synchronized = change.enabled;
                                            sync_started_at = change.enabled.then(Instant::now);
                                        }
                                        PrivateMode::ColorSchemeUpdates => {
                                            color_scheme_updates = change.enabled
                                        }
                                        _ => {}
                                    }
                                    modes.push(ModeChange {
                                        end_offset: base + change.end_offset,
                                        ..*change
                                    });
                                }

                                // Offsets are rebased onto the batch for the same reason
                                // as the signals': a consumer cutting output at them
                                // would otherwise capture the query's own bytes.
                                for query in scanner.queries() {
                                    queries.push(match *query {
                                        TerminalQuery::ColorScheme { end_offset } => {
                                            TerminalQuery::ColorScheme {
                                                end_offset: base + end_offset,
                                            }
                                        }
                                    });
                                }

                                batch.extend_from_slice(&data);
                                if first_byte_at.is_none() {
                                    first_byte_at = Some(Instant::now());
                                }
                                // Synchronized output: the application has asked
                                // not to be repainted mid-frame. Holding the
                                // batch removes tearing and costs nothing, since
                                // the pump already batches — but the hold is
                                // bounded, because a program that sets 2026 and
                                // then blocks must not freeze the pane.
                                let holding = synchronized
                                    && sync_started_at
                                        .map(|t| t.elapsed() < MAX_SYNC_HOLD)
                                        .unwrap_or(false);

                                if !holding && batch.len() >= FLUSH_BYTES {
                                    flush!();
                                }
                            }
                            Err(RecvTimeoutError::Timeout) => {
                                let holding = synchronized
                                    && sync_started_at
                                        .map(|t| t.elapsed() < MAX_SYNC_HOLD)
                                        .unwrap_or(false);
                                if !holding {
                                    flush!();
                                }
                            }
                            Err(RecvTimeoutError::Disconnected) => {
                                // Final flush before exiting. Written out rather
                                // than reusing `flush!` because resetting the
                                // timer here would be dead code.
                                if !batch.is_empty() {
                                    sink(PtyEvent::Chunk(PtyChunk {
                                        pane_id: pane.clone(),
                                        bytes: std::mem::take(&mut batch),
                                        signals: std::mem::take(&mut signals),
                                        pending_marker: pending,
                                        mode_changes: std::mem::take(&mut modes),
                                        queries: std::mem::take(&mut queries),
                                        color_scheme_updates,
                                        alternate_screen,
                                    }));
                                }
                                break;
                            }
                        }
                    }

                    // The reader hit EOF, so the child is gone or going. Reap it
                    // for the real status rather than reporting a guess.
                    let exit_code = child_for_exit
                        .lock()
                        .wait()
                        .ok()
                        .map(|status| status.exit_code() as i32);
                    sink(PtyEvent::Exited {
                        pane_id: pane.clone(),
                        exit_code,
                    });
                })
                .map_err(PtyError::Io)?;
        }

        let pending_input = Arc::new(Mutex::new(Some(Vec::new())));
        let master = Mutex::new(pair.master);
        let writer = Arc::new(Mutex::new(writer));

        // Release input as soon as the child can actually receive it.
        #[cfg(unix)]
        {
            let fd = master.lock().as_raw_fd();
            let pending = pending_input.clone();
            let writer = writer.clone();
            let alive = alive.clone();
            std::thread::Builder::new()
                .name(format!("tervin-pty-ready-{}", config.pane_id))
                .spawn(move || {
                    let deadline = Instant::now() + INPUT_GATE_MAX;
                    while Instant::now() < deadline
                        && alive.load(Ordering::SeqCst)
                        && !fd.is_some_and(accepts_input)
                    {
                        std::thread::sleep(INPUT_GATE_POLL);
                    }

                    // Open the gate either way, and under the same lock `write`
                    // takes, so nothing written afterwards can overtake what was
                    // held. A child that never leaves canonical mode — `cat`, a
                    // pager reading lines — discards nothing and must not be gated.
                    let mut pending = pending.lock();
                    if let Some(queued) = pending.take().filter(|q| !q.is_empty()) {
                        let mut w = writer.lock();
                        let _ = w.write_all(&queued).and_then(|()| w.flush());
                    }
                })
                .map_err(PtyError::Io)?;
        }
        #[cfg(not(unix))]
        {
            *pending_input.lock() = None;
        }

        Ok(Self {
            pane_id: config.pane_id,
            master,
            writer,
            child,
            alive,
            size: Mutex::new((config.cols, config.rows)),
            pending_input,
        })
    }

    pub fn pane_id(&self) -> &PaneId {
        &self.pane_id
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn size(&self) -> (u16, u16) {
        *self.size.lock()
    }

    /// Write user input to the PTY.
    ///
    /// Input written before the child has taken over the terminal is held rather
    /// than sent. A shell's line editor calls `tcsetattr` with `TCSAFLUSH` when it
    /// starts reading, and that **discards whatever is already queued** — so input
    /// sent a moment too early is not delayed, it is destroyed. Tervin writes to a
    /// pane programmatically as well as on keystrokes, and a restored session or an
    /// agent-issued command lands squarely in that window: the symptom is a command
    /// that runs with its first character missing.
    ///
    /// The gate is held under the same lock the watcher releases it with, so a write
    /// arriving after the gate opens can never overtake one that was held. It costs
    /// one uncontended lock once the pane is running.
    pub fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        if !self.is_alive() {
            return Err(PtyError::NotRunning(self.pane_id.clone()));
        }
        {
            let mut pending = self.pending_input.lock();
            if let Some(queue) = pending.as_mut() {
                queue.extend_from_slice(data);
                return Ok(());
            }
        }
        let mut w = self.writer.lock();
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }

    /// Resize the PTY, which also delivers `SIGWINCH` to the child so full-screen
    /// programs reflow.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        self.master
            .lock()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Open(e.to_string()))?;
        *self.size.lock() = (cols, rows);
        Ok(())
    }

    /// Ask the child to terminate.
    pub fn kill(&self) -> Result<(), PtyError> {
        let _ = self.child.lock().kill();
        Ok(())
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // A dropped session must not leave an orphaned process holding the PTY.
        let _ = self.child.lock().kill();
    }
}
