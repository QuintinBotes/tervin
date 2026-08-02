//! A non-destructive OSC tap.
//!
//! Tervin needs shell-integration signals out of the PTY byte stream, but the
//! stream itself must reach the renderer *byte for byte* — terminal correctness
//! comes first, and anything that rewrites output risks breaking Neovim, tmux,
//! interactive CLIs, or partial UTF-8 at a chunk boundary.
//!
//! So this is a tap, not a filter. `feed` observes bytes and returns the
//! sequences it recognised; the caller forwards the original slice unchanged.
//!
//! It is a minimal escape-state machine rather than a full VT parser because it
//! only has to answer one question: is this byte inside an OSC payload? It still
//! has to track CSI, DCS, APC, and PM sequences, since `]` (0x5D) is a legal CSI
//! final byte and would otherwise be mistaken for the start of an OSC string.

/// Maximum OSC payload we will buffer. A garbled or hostile stream must not be
/// able to grow this without bound; oversized payloads are dropped, not truncated
/// into something that might parse as a different command.
const MAX_OSC_PAYLOAD: usize = 64 * 1024;

/// Cap on buffered CSI parameter bytes. Real sequences are a handful of bytes.
const MAX_CSI_PARAMS: usize = 64;

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1A;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    /// Saw ESC; the next byte selects the sequence type.
    Escape,
    /// Inside `ESC [ … final`.
    Csi,
    /// Inside an OSC payload, terminated by BEL or ST.
    Osc,
    /// Saw ESC while inside an OSC payload — checking for `\` (ST).
    OscEscape,
    /// Inside DCS/APC/PM; consumed and discarded, terminated by ST or BEL.
    StringPassthrough,
    /// Saw ESC while inside a passthrough string.
    StringPassthroughEscape,
}

/// One recognised OSC sequence and where it sat in the fed slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OscHit {
    /// Offset of the leading `ESC`, or `None` when the sequence began in an
    /// earlier chunk.
    ///
    /// Consumers capturing output need this to exclude the marker's own bytes;
    /// without it, invisible control sequences accumulate inside every Block and
    /// leak into plain-text exports.
    pub start_offset: Option<usize>,
    /// Offset one past the sequence's final byte.
    ///
    /// Block boundaries are cut here. Without it, the bytes of a `command
    /// finished` marker — and the prompt that follows it — would be captured as
    /// part of the command's own output.
    pub end_offset: usize,
    pub payload: Vec<u8>,
}

/// DEC private modes Tervin tracks, and why each one matters.
///
/// These are set and reset with `CSI ? <n> h` / `CSI ? <n> l`. Tervin does not
/// implement them — the renderer does — but it has to *know* about them, because
/// each changes what the surrounding machinery should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateMode {
    /// 1049 / 1047 / 47 — the alternate screen.
    ///
    /// Entered by every full-screen program: vim, less, htop, an interactive
    /// agent TUI. While it is active, output is a stream of redraws rather than
    /// a command's results, so capturing it into a Block would store megabytes of
    /// cursor movement instead of anything a person wants to read.
    AlternateScreen,
    /// 2026 — synchronized output.
    ///
    /// An application declares "do not repaint until I am done". Honouring it
    /// removes tearing on full-screen redraws, and it is free here because the
    /// PTY pump already batches.
    SynchronizedOutput,
    /// 2004 — bracketed paste. Surfaced so paste safety can tell whether the
    /// program will receive the paste as one unit or as keystrokes.
    BracketedPaste,
    /// 1004 — focus reporting.
    FocusReporting,
    /// 1000 / 1002 / 1003 / 1006 — mouse reporting.
    ///
    /// Tracked so a click can be routed to the program rather than starting a
    /// selection.
    MouseReporting,
    /// 2031 — colour-scheme change notification.
    ///
    /// An application asks to be told when the terminal's background goes light or
    /// dark, so it can restyle itself instead of rendering unreadably. Tervin ships
    /// fifteen themes and switching between them is a normal thing to do, which makes
    /// this worth honouring rather than ignoring.
    ///
    /// Tracked per pane, because the report goes only to programs that asked: sending
    /// an unsolicited `CSI ? 997` to a shell that never enabled the mode would put
    /// stray text on the command line.
    ColorSchemeUpdates,
}

/// A request from the program that the terminal is expected to answer.
///
/// Collected rather than answered, so replying stays the caller's decision — the same
/// rule OSC 52 reads follow, where Tervin deliberately never answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalQuery {
    /// `CSI ? 996 n` — whether the background is light or dark.
    ColorScheme {
        /// Offset one past the sequence.
        end_offset: usize,
    },
}

/// Which way round the terminal's background is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Dark,
    Light,
}

impl ColorScheme {
    /// The reply an application expects: `CSI ? 997 ; 1 n` for dark, `; 2 n` for light.
    ///
    /// The same bytes serve both purposes — answering `CSI ? 996 n` and reporting a later
    /// change to a pane that enabled mode 2031 — so there is one definition of them.
    pub fn report(self) -> &'static [u8] {
        match self {
            Self::Dark => b"\x1b[?997;1n",
            Self::Light => b"\x1b[?997;2n",
        }
    }

    /// Decide from a background colour's perceived brightness.
    ///
    /// Rec. 601 luma rather than a plain average: green contributes far more to
    /// perceived brightness than blue, and averaging calls a saturated blue background
    /// light when no one would read it that way.
    pub fn from_background(r: u8, g: u8, b: u8) -> Self {
        let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
        if luma < 128.0 {
            Self::Dark
        } else {
            Self::Light
        }
    }
}

/// A DEC private mode changing state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeChange {
    pub mode: PrivateMode,
    /// True for `h` (set), false for `l` (reset).
    pub enabled: bool,
    /// Offset one past the sequence, so a consumer can split on it.
    pub end_offset: usize,
}

/// Whether a slice ended part-way through an OSC sequence.
///
/// A PTY read can split anywhere, including between the `ESC ]` introducer and
/// its terminator. Consumers that exclude marker bytes from captured output need
/// to know those trailing bytes are the head of a sequence, not real output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PendingMarker {
    /// The slice ended cleanly.
    #[default]
    None,
    /// An unterminated sequence began at this offset within the slice.
    StartedAt(usize),
    /// An unterminated sequence began before this slice, so the whole slice is
    /// a continuation of it.
    Earlier,
}

/// Scans a byte stream for OSC sequences without altering it.
///
/// The scanner is stateful across `feed` calls, so a sequence split across two
/// PTY reads is still recognised.
#[derive(Debug)]
pub struct OscScanner {
    state: State,
    payload: Vec<u8>,
    /// Set when the current payload exceeded `MAX_OSC_PAYLOAD`, so we keep
    /// consuming to the terminator but emit nothing.
    overflowed: bool,
    /// Where the in-progress sequence started, relative to the current `feed`
    /// call. `None` once we know it began in an earlier chunk.
    pending_start: Option<usize>,
    /// Parameter bytes of the CSI sequence currently being scanned.
    ///
    /// Bounded: a CSI parameter list is a handful of bytes in practice, and a
    /// malformed stream must not be able to grow this without limit.
    csi_params: Vec<u8>,
    /// Private-mode changes found during the current `feed` call.
    mode_changes: Vec<ModeChange>,
    /// Queries the program made during the current `feed` call.
    queries: Vec<TerminalQuery>,
}

impl Default for OscScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl OscScanner {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            payload: Vec::with_capacity(256),
            overflowed: false,
            pending_start: None,
            csi_params: Vec::with_capacity(16),
            mode_changes: Vec::new(),
            queries: Vec::new(),
        }
    }

    /// Observe `bytes`, returning every complete OSC payload found.
    ///
    /// The input is never modified and the caller must forward it verbatim.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.feed_indexed(bytes)
            .into_iter()
            .map(|hit| hit.payload)
            .collect()
    }

    /// As [`OscScanner::feed`], but reporting where each sequence ended so the
    /// caller can split the chunk on exact boundaries.
    pub fn feed_indexed(&mut self, bytes: &[u8]) -> Vec<OscHit> {
        let mut found = Vec::new();
        self.mode_changes.clear();
        self.queries.clear();

        // A sequence already in progress began before this slice, so it has no
        // start offset within it.
        if matches!(self.state, State::Osc | State::OscEscape) {
            self.pending_start = None;
        }

        for (index, &b) in bytes.iter().enumerate() {
            match self.state {
                State::Ground => {
                    if b == ESC {
                        self.state = State::Escape;
                    }
                }

                State::Escape => match b {
                    b']' => {
                        self.payload.clear();
                        self.overflowed = false;
                        // The ESC sits one byte back. `checked_sub` returning
                        // None means the ESC was the tail of the previous chunk.
                        self.pending_start = index.checked_sub(1);
                        self.state = State::Osc;
                    }
                    b'[' => {
                        self.csi_params.clear();
                        self.state = State::Csi;
                    }
                    // DCS, APC, PM, SOS: string sequences we must consume so
                    // their contents cannot be mistaken for an OSC start.
                    b'P' | b'_' | b'^' | b'X' => self.state = State::StringPassthrough,
                    // A second ESC restarts the escape.
                    ESC => self.state = State::Escape,
                    // Everything else is a short two-byte escape.
                    _ => self.state = State::Ground,
                },

                State::Csi => {
                    // Parameter and intermediate bytes are 0x20–0x3F; the final
                    // byte is 0x40–0x7E and ends the sequence.
                    match b {
                        0x40..=0x7E => {
                            if b == b'h' || b == b'l' {
                                self.decode_private_modes(b == b'h', index + 1);
                            } else if b == b'n' && self.csi_params == b"?996" {
                                // `CSI ? 996 n` — "is your background light or dark?".
                                // Recorded rather than answered here: this type does not
                                // write to the terminal, and a scanner that replied would
                                // be doing something its name does not say.
                                self.queries.push(TerminalQuery::ColorScheme {
                                    end_offset: index + 1,
                                });
                            }
                            self.csi_params.clear();
                            self.state = State::Ground;
                        }
                        ESC => {
                            self.csi_params.clear();
                            self.state = State::Escape;
                        }
                        CAN | SUB => {
                            self.csi_params.clear();
                            self.state = State::Ground;
                        }
                        _ => {
                            if self.csi_params.len() < MAX_CSI_PARAMS {
                                self.csi_params.push(b);
                            }
                        }
                    }
                }

                State::Osc => match b {
                    BEL => {
                        self.finish_osc(&mut found, index + 1);
                    }
                    ESC => self.state = State::OscEscape,
                    CAN | SUB => {
                        // Cancel: abandon the payload without emitting.
                        self.payload.clear();
                        self.state = State::Ground;
                    }
                    _ => self.push_payload(b),
                },

                State::OscEscape => {
                    if b == b'\\' {
                        // ESC \ is ST — a properly terminated OSC.
                        self.finish_osc(&mut found, index + 1);
                    } else if b == ESC {
                        // Stay armed; a lone ESC inside a payload is malformed
                        // but we tolerate it rather than losing the stream.
                        self.state = State::OscEscape;
                    } else {
                        // Not a terminator: the ESC was literal payload content.
                        self.push_payload(ESC);
                        self.push_payload(b);
                        self.state = State::Osc;
                    }
                }

                State::StringPassthrough => match b {
                    BEL => self.state = State::Ground,
                    ESC => self.state = State::StringPassthroughEscape,
                    _ => {}
                },

                State::StringPassthroughEscape => {
                    if b == b'\\' {
                        self.state = State::Ground;
                    } else if b == ESC {
                        self.state = State::StringPassthroughEscape;
                    } else {
                        self.state = State::StringPassthrough;
                    }
                }
            }
        }

        found
    }

    /// Private-mode changes seen in the most recent `feed` call, in order.
    pub fn mode_changes(&self) -> &[ModeChange] {
        &self.mode_changes
    }

    /// Queries seen in the most recent `feed` call, in order.
    pub fn queries(&self) -> &[TerminalQuery] {
        &self.queries
    }

    /// Decode `CSI ? <n>;<n> h|l` into the modes Tervin tracks.
    ///
    /// A single sequence can carry several semicolon-separated modes, and
    /// anything not recognised is ignored rather than guessed at.
    fn decode_private_modes(&mut self, enabled: bool, end_offset: usize) {
        // Only private modes, which are prefixed with '?'.
        let Some(params) = self.csi_params.strip_prefix(b"?") else {
            return;
        };
        for part in params.split(|&c| c == b';') {
            let Ok(text) = std::str::from_utf8(part) else {
                continue;
            };
            let Ok(number) = text.trim().parse::<u16>() else {
                continue;
            };
            let mode = match number {
                47 | 1047 | 1049 => PrivateMode::AlternateScreen,
                2026 => PrivateMode::SynchronizedOutput,
                2004 => PrivateMode::BracketedPaste,
                1004 => PrivateMode::FocusReporting,
                1000 | 1002 | 1003 | 1006 => PrivateMode::MouseReporting,
                2031 => PrivateMode::ColorSchemeUpdates,
                _ => continue,
            };
            self.mode_changes.push(ModeChange {
                mode,
                enabled,
                end_offset,
            });
        }
    }

    /// Whether the most recently fed slice ended inside an OSC sequence.
    ///
    /// Valid immediately after a `feed` call; the offset is relative to that
    /// slice.
    pub fn pending_marker(&self) -> PendingMarker {
        if !matches!(self.state, State::Osc | State::OscEscape) {
            return PendingMarker::None;
        }
        match self.pending_start {
            Some(i) => PendingMarker::StartedAt(i),
            None => PendingMarker::Earlier,
        }
    }

    fn push_payload(&mut self, b: u8) {
        if self.payload.len() >= MAX_OSC_PAYLOAD {
            self.overflowed = true;
            return;
        }
        self.payload.push(b);
    }

    fn finish_osc(&mut self, found: &mut Vec<OscHit>, end_offset: usize) {
        if !self.overflowed && !self.payload.is_empty() {
            found.push(OscHit {
                start_offset: self.pending_start,
                end_offset,
                payload: std::mem::take(&mut self.payload),
            });
        } else {
            self.payload.clear();
        }
        self.overflowed = false;
        self.pending_start = None;
        self.state = State::Ground;
    }
}

/// Split an OSC payload on `;` into at most `limit` fields.
///
/// The final field keeps any remaining separators, so a value that legitimately
/// contains `;` (a hyperlink URI, a base64 command) survives intact.
pub fn split_params(payload: &[u8], limit: usize) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut rest = payload;
    while out.len() + 1 < limit {
        match rest.iter().position(|&c| c == b';') {
            Some(i) => {
                out.push(&rest[..i]);
                rest = &rest[i + 1..];
            }
            None => break,
        }
    }
    out.push(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(input: &[u8]) -> Vec<String> {
        let mut s = OscScanner::new();
        s.feed(input)
            .into_iter()
            .map(|p| String::from_utf8_lossy(&p).to_string())
            .collect()
    }

    #[test]
    fn finds_bel_terminated_osc() {
        assert_eq!(scan(b"\x1b]0;my title\x07"), vec!["0;my title"]);
    }

    #[test]
    fn finds_st_terminated_osc() {
        assert_eq!(scan(b"\x1b]133;D;0\x1b\\"), vec!["133;D;0"]);
    }

    #[test]
    fn ignores_csi_with_bracket_final_byte() {
        // ESC [ 0 ] is a CSI sequence whose final byte is ']'. Treating that as
        // an OSC start would desynchronise the scanner for the rest of the stream.
        assert!(scan(b"\x1b[0]hello").is_empty());
    }

    #[test]
    fn survives_split_across_chunks() {
        let mut s = OscScanner::new();
        assert!(s.feed(b"\x1b]7;file://host/tm").is_empty());
        let got = s.feed(b"p/dir\x07");
        assert_eq!(
            got.iter()
                .map(|p| String::from_utf8_lossy(p).to_string())
                .collect::<Vec<_>>(),
            vec!["7;file://host/tmp/dir"]
        );
    }

    #[test]
    fn skips_dcs_payload_containing_osc_lookalike() {
        // A DCS payload can contain anything; an OSC-looking run inside it must
        // not be reported.
        assert!(scan(b"\x1bP1$r\x1b]133;A\x07\x1b\\").is_empty());
    }

    #[test]
    fn finds_osc_after_interleaved_sequences() {
        let got = scan(b"\x1b[1;31mred\x1b[0m\x1b]133;A\x07done");
        assert_eq!(got, vec!["133;A"]);
    }

    #[test]
    fn drops_oversized_payload_without_desync() {
        let mut huge = Vec::from(&b"\x1b]133;"[..]);
        huge.extend(std::iter::repeat_n(b'x', MAX_OSC_PAYLOAD + 64));
        huge.push(BEL);
        huge.extend_from_slice(b"\x1b]133;A\x07");

        let mut s = OscScanner::new();
        let got: Vec<String> = s
            .feed(&huge)
            .into_iter()
            .map(|p| String::from_utf8_lossy(&p).to_string())
            .collect();
        // The oversized payload is discarded, but the scanner recovers and finds
        // the next well-formed sequence.
        assert_eq!(got, vec!["133;A"]);
    }

    #[test]
    fn literal_esc_inside_payload_is_kept() {
        // ESC not followed by '\' is payload content, not a terminator.
        assert_eq!(scan(b"\x1b]777;a\x1bXb\x07"), vec!["777;a\x1bXb"]);
    }

    #[test]
    fn reports_end_offset_for_block_boundaries() {
        // The offset must land one past the terminator so that "out" belongs to
        // the finished command and "next" does not.
        //          0..3 "out"   3..13 marker      13.. "next"
        let stream = b"out\x1b]133;D;0\x07next";
        let mut s = OscScanner::new();
        let hits = s.feed_indexed(stream);
        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.payload, b"133;D;0");
        assert_eq!(hit.start_offset, Some(3));
        assert_eq!(hit.end_offset, 13);
        // Output belongs to the command; the marker's own bytes belong to nobody
        // and must be excluded from both.
        assert_eq!(&stream[..hit.start_offset.unwrap()], b"out");
        // Everything after the marker belongs to whatever comes next — the
        // prompt, typically — and must not land in the finished block.
        assert_eq!(&stream[hit.end_offset..], b"next");
    }

    #[test]
    fn start_offset_is_none_when_sequence_spans_chunks() {
        // The ESC arrived in the previous read, so there is no start offset in
        // this slice; consumers must treat the whole slice prefix as marker.
        let mut s = OscScanner::new();
        assert!(s.feed_indexed(b"tail\x1b").is_empty());
        let hits = s.feed_indexed(b"]133;A\x07rest");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start_offset, None);
        assert_eq!(hits[0].end_offset, 7);
    }

    #[test]
    fn detects_alternate_screen_entry_and_exit() {
        // Every full-screen program does this, and Block capture depends on it.
        let mut s = OscScanner::new();
        s.feed_indexed(b"\x1b[?1049h");
        assert_eq!(
            s.mode_changes(),
            &[ModeChange {
                mode: PrivateMode::AlternateScreen,
                enabled: true,
                end_offset: 8
            }]
        );

        s.feed_indexed(b"\x1b[?1049l");
        assert!(!s.mode_changes()[0].enabled);
    }

    #[test]
    fn detects_synchronized_output() {
        let mut s = OscScanner::new();
        s.feed_indexed(b"\x1b[?2026h");
        assert_eq!(s.mode_changes()[0].mode, PrivateMode::SynchronizedOutput);
        assert!(s.mode_changes()[0].enabled);
    }

    #[test]
    fn one_sequence_can_carry_several_modes() {
        // `CSI ? 1049 ; 2004 h` is legal and both modes must register.
        let mut s = OscScanner::new();
        s.feed_indexed(b"\x1b[?1049;2004h");
        let modes: Vec<PrivateMode> = s.mode_changes().iter().map(|c| c.mode).collect();
        assert!(modes.contains(&PrivateMode::AlternateScreen));
        assert!(modes.contains(&PrivateMode::BracketedPaste));
    }

    #[test]
    fn non_private_modes_are_ignored() {
        // `CSI 4 h` is insert mode, not a DEC private mode. Treating it as one
        // would misread ordinary output as a screen switch.
        let mut s = OscScanner::new();
        s.feed_indexed(b"\x1b[4h");
        assert!(s.mode_changes().is_empty());
    }

    #[test]
    fn unknown_private_modes_are_ignored_not_guessed() {
        let mut s = OscScanner::new();
        s.feed_indexed(b"\x1b[?9999h");
        assert!(s.mode_changes().is_empty());
    }

    #[test]
    fn mode_changes_reset_between_feeds() {
        // Otherwise a consumer would replay stale transitions on every chunk.
        let mut s = OscScanner::new();
        s.feed_indexed(b"\x1b[?1049h");
        assert_eq!(s.mode_changes().len(), 1);
        s.feed_indexed(b"plain output");
        assert!(s.mode_changes().is_empty());
    }

    #[test]
    fn colour_sequences_do_not_register_as_modes() {
        // `CSI 1;31m` shares the CSI shape; only h/l finals decode modes.
        let mut s = OscScanner::new();
        s.feed_indexed(b"\x1b[1;31mred\x1b[0m");
        assert!(s.mode_changes().is_empty());
    }

    #[test]
    fn splits_params_keeping_tail_separators() {
        let p = b"8;;https://example.com/a;b";
        let parts = split_params(p, 3);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], b"8");
        assert_eq!(parts[1], b"");
        assert_eq!(parts[2], b"https://example.com/a;b");
    }
}

/// Colour-scheme reporting: DEC mode 2031 and the `CSI ? 996 n` query.
#[cfg(test)]
mod color_scheme_tests {
    use super::*;

    fn modes(input: &[u8]) -> Vec<(PrivateMode, bool)> {
        let mut s = OscScanner::new();
        s.feed(input);
        s.mode_changes()
            .iter()
            .map(|m| (m.mode, m.enabled))
            .collect()
    }

    fn queries(input: &[u8]) -> Vec<TerminalQuery> {
        let mut s = OscScanner::new();
        s.feed(input);
        s.queries().to_vec()
    }

    #[test]
    fn tracks_a_program_subscribing_to_colour_scheme_changes() {
        assert_eq!(
            modes(b"\x1b[?2031h"),
            vec![(PrivateMode::ColorSchemeUpdates, true)]
        );
        assert_eq!(
            modes(b"\x1b[?2031l"),
            vec![(PrivateMode::ColorSchemeUpdates, false)]
        );
    }

    #[test]
    fn reads_the_light_or_dark_query() {
        assert_eq!(
            queries(b"\x1b[?996n"),
            // ESC [ ? 9 9 6 n — seven bytes.
            vec![TerminalQuery::ColorScheme { end_offset: 7 }]
        );
    }

    #[test]
    fn the_query_offset_points_past_the_sequence() {
        // Consumers cut output at this offset; if it were wrong, the query's own bytes
        // would be captured into a Block as if the program had printed them.
        let input = b"before\x1b[?996nafter";
        let TerminalQuery::ColorScheme { end_offset } = queries(input)[0];
        assert_eq!(&input[end_offset..], b"after");
    }

    #[test]
    fn other_status_reports_are_not_mistaken_for_it() {
        // `CSI 6 n` is the cursor-position report, which programs send constantly.
        // Answering it with a colour scheme would corrupt their input.
        assert!(queries(b"\x1b[6n").is_empty());
        assert!(queries(b"\x1b[?6n").is_empty());
        assert!(queries(b"\x1b[?997n").is_empty());
        assert!(queries(b"\x1b[?996h").is_empty());
    }

    #[test]
    fn a_query_split_across_two_reads_is_still_recognised() {
        // A PTY read can end anywhere, including mid-sequence.
        let mut s = OscScanner::new();
        s.feed(b"\x1b[?99");
        assert!(s.queries().is_empty());
        s.feed(b"6n");
        assert_eq!(s.queries().len(), 1);
    }

    #[test]
    fn queries_do_not_accumulate_across_feeds() {
        // They are per-feed, like mode changes; a stale query would be answered twice.
        let mut s = OscScanner::new();
        s.feed(b"\x1b[?996n");
        assert_eq!(s.queries().len(), 1);
        s.feed(b"plain output");
        assert!(s.queries().is_empty());
    }

    #[test]
    fn the_reply_is_the_sequence_applications_expect() {
        assert_eq!(ColorScheme::Dark.report(), b"\x1b[?997;1n");
        assert_eq!(ColorScheme::Light.report(), b"\x1b[?997;2n");
    }

    #[test]
    fn brightness_is_judged_by_luma_not_by_average() {
        // Tervin's own themes, at both ends.
        assert_eq!(
            ColorScheme::from_background(0x0d, 0x11, 0x17),
            ColorScheme::Dark
        );
        assert_eq!(
            ColorScheme::from_background(0xff, 0xff, 0xff),
            ColorScheme::Light
        );
        assert_eq!(
            ColorScheme::from_background(0xfa, 0xf4, 0xed),
            ColorScheme::Light
        );

        // The case that makes luma the right choice: a saturated blue averages to 85,
        // which a mean would call dark-ish, but its green channel is what the eye reads.
        // Green at the same value is unambiguously light; blue is not.
        assert_eq!(ColorScheme::from_background(0, 0, 0xff), ColorScheme::Dark);
        assert_eq!(ColorScheme::from_background(0, 0xff, 0), ColorScheme::Light);
    }
}
