//! Detection of terminal queries in the PTY output stream, and the replies a
//! real terminal would write back.
//!
//! A real terminal answers queries such as DSR 6 (cursor position) and DA1
//! (device attributes) by writing the reply onto the line as if typed.
//! libghostty-vt parses these sequences but has no reply channel, so a program
//! that waits for an answer (crossterm's `cursor::position()`, vim's startup
//! probes) hangs until its own timeout (#3103). The engine feeds output bytes
//! through [`QueryScanner`] and answers each detected [`TerminalQuery`] by
//! writing [`TerminalQuery::reply`] back to the PTY as terminal input.

/// `ESC` (0x1b), the introducer of every escape sequence.
const ESC: u8 = 0x1b;
/// `BEL` (0x07), one of the two OSC terminators.
const BEL: u8 = 0x07;
/// `CAN` (0x18) and `SUB` (0x1a): per ECMA-48 either one aborts an in-progress
/// escape sequence or control string.
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

/// Longest CSI parameter/intermediate run that can still be a query this crate
/// answers (`6`, `5`, ``, `0`); a longer run is some other sequence, so the
/// scanner stops buffering and lets the final byte pass unmatched.
const MAX_PARAMS: usize = 8;

/// A terminal query that expects a reply written back as terminal input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalQuery {
    /// DSR 6 (`CSI 6 n`): report the cursor position as `CSI row ; col R`.
    CursorPosition,
    /// DSR 5 (`CSI 5 n`): report operating status.
    OperatingStatus,
    /// DA1 (`CSI c` / `CSI 0 c`): report primary device attributes.
    PrimaryDeviceAttributes,
}

impl TerminalQuery {
    /// The reply bytes a real terminal writes for this query.
    ///
    /// `cursor` is the zero-based `(col, row)` cursor position, only consulted
    /// for [`Self::CursorPosition`]; `None` (cursor scrolled out of the
    /// viewport) reports `1;1`, mirroring `snapshot_to_cursor`.
    #[must_use]
    pub fn reply(self, cursor: Option<(u16, u16)>) -> Vec<u8> {
        match self {
            // 1-based row;col, exactly what ghostty itself reports
            // (src/termio/stream_handler.zig `deviceStatusReport`).
            Self::CursorPosition => {
                let (col, row) = cursor.unwrap_or((0, 0));
                format!("\x1b[{};{}R", u32::from(row) + 1, u32::from(col) + 1).into_bytes()
            }
            // "OK", the only status a healthy terminal reports.
            Self::OperatingStatus => b"\x1b[0n".to_vec(),
            // VT220 (62) with ANSI color (22): ghostty's own DA1 reply minus
            // the clipboard capability (52), which this emulator lacks.
            Self::PrimaryDeviceAttributes => b"\x1b[?62;22c".to_vec(),
        }
    }
}

/// A query detected by [`QueryScanner::scan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedQuery {
    /// Offset one past the query's final byte in the scanned chunk, so the
    /// caller can feed everything up to and including the query before
    /// answering (the reply must reflect the bytes that preceded it).
    pub end: usize,
    /// The query to answer.
    pub query: TerminalQuery,
}

/// Where the scanner is within the output byte stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    /// Outside any escape sequence.
    #[default]
    Ground,
    /// Saw `ESC`; the next byte selects the sequence kind.
    Escape,
    /// Inside a CSI sequence, buffering parameter/intermediate bytes.
    Csi,
    /// Inside an OSC/DCS/SOS/PM/APC control string; queries cannot start here.
    /// `ESC` ends the string (ghostty terminates on any `ESC`, clean ST or
    /// not), so leaving this state reuses [`State::Escape`].
    ControlString,
}

/// A stateful scanner that finds answerable terminal queries in raw output.
///
/// Byte boundaries are arbitrary: state persists across [`scan`] calls, so a
/// query split across PTY reads still matches.
///
/// [`scan`]: Self::scan
#[derive(Default)]
pub struct QueryScanner {
    state: State,
    /// CSI parameter and intermediate bytes of the sequence being scanned.
    params: Vec<u8>,
    /// The current CSI's parameters overflowed [`MAX_PARAMS`], so its final
    /// byte must not be matched against the (short) known queries.
    overflow: bool,
}

impl QueryScanner {
    /// Create a scanner in the ground state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: State::Ground,
            params: Vec::new(),
            overflow: false,
        }
    }

    /// Scan a chunk of raw terminal output, returning each detected query in
    /// stream order.
    pub fn scan(&mut self, data: &[u8]) -> Vec<DetectedQuery> {
        let mut found = Vec::new();
        for (i, &byte) in data.iter().enumerate() {
            if let Some(query) = self.step(byte) {
                found.push(DetectedQuery { end: i + 1, query });
            }
        }
        found
    }

    /// Advance one byte, returning a query when this byte completes one.
    fn step(&mut self, byte: u8) -> Option<TerminalQuery> {
        match self.state {
            State::Ground => {
                if byte == ESC {
                    self.state = State::Escape;
                }
                None
            }
            State::Escape => {
                match byte {
                    b'[' => {
                        self.params.clear();
                        self.overflow = false;
                        self.state = State::Csi;
                    }
                    // OSC / DCS / SOS / PM / APC open a control string.
                    b']' | b'P' | b'X' | b'^' | b'_' => self.state = State::ControlString,
                    // Consecutive ESC: still waiting on the selector.
                    ESC => {}
                    _ => self.state = State::Ground,
                }
                None
            }
            State::Csi => match byte {
                ESC => {
                    self.state = State::Escape;
                    None
                }
                CAN | SUB => {
                    self.state = State::Ground;
                    None
                }
                // Parameter (0x30-0x3f) and intermediate (0x20-0x2f) bytes.
                0x20..=0x3f => {
                    if self.params.len() < MAX_PARAMS {
                        self.params.push(byte);
                    } else {
                        self.overflow = true;
                    }
                    None
                }
                // Final byte: the sequence is complete.
                0x40..=0x7e => {
                    self.state = State::Ground;
                    if self.overflow {
                        return None;
                    }
                    self.match_query(byte)
                }
                // Other C0 controls are executed mid-sequence without
                // disturbing it (ECMA-48), so stay in CSI.
                _ => None,
            },
            State::ControlString => {
                match byte {
                    BEL | CAN | SUB => self.state = State::Ground,
                    ESC => self.state = State::Escape,
                    _ => {}
                }
                None
            }
        }
    }

    /// Match a completed CSI against the queries this crate answers.
    ///
    /// Matches are exact: a private-marker form such as `CSI ? 6 n` (DECXCPR)
    /// or `CSI > c` (DA2) is a different query and stays unanswered.
    fn match_query(&self, final_byte: u8) -> Option<TerminalQuery> {
        match (final_byte, self.params.as_slice()) {
            (b'n', b"6") => Some(TerminalQuery::CursorPosition),
            (b'n', b"5") => Some(TerminalQuery::OperatingStatus),
            (b'c', b"" | b"0") => Some(TerminalQuery::PrimaryDeviceAttributes),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DetectedQuery, QueryScanner, TerminalQuery};

    #[test]
    fn detects_dsr_and_da1_with_end_offsets() {
        let mut scanner = QueryScanner::new();
        assert_eq!(
            scanner.scan(b"hi\x1b[6n"),
            vec![DetectedQuery {
                end: 6,
                query: TerminalQuery::CursorPosition
            }]
        );
        assert_eq!(
            scanner.scan(b"\x1b[5n\x1b[c\x1b[0c"),
            vec![
                DetectedQuery {
                    end: 4,
                    query: TerminalQuery::OperatingStatus
                },
                DetectedQuery {
                    end: 7,
                    query: TerminalQuery::PrimaryDeviceAttributes
                },
                DetectedQuery {
                    end: 11,
                    query: TerminalQuery::PrimaryDeviceAttributes
                },
            ]
        );
    }

    #[test]
    fn matches_query_split_across_chunks() {
        let mut scanner = QueryScanner::new();
        assert!(scanner.scan(b"\x1b[6").is_empty());
        assert_eq!(
            scanner.scan(b"n"),
            vec![DetectedQuery {
                end: 1,
                query: TerminalQuery::CursorPosition
            }]
        );
    }

    #[test]
    fn private_and_secondary_forms_stay_unanswered() {
        let mut scanner = QueryScanner::new();
        // DECXCPR (private DSR) and DA2 are different queries.
        assert!(scanner.scan(b"\x1b[?6n").is_empty());
        assert!(scanner.scan(b"\x1b[>c").is_empty());
        // SGR and cursor movement share the CSI grammar but are not queries.
        assert!(scanner.scan(b"\x1b[38;5;196m\x1b[6C").is_empty());
    }

    #[test]
    fn control_string_payload_is_not_scanned() {
        let mut scanner = QueryScanner::new();
        // Query-shaped bytes inside an OSC payload (no inner ESC) are payload.
        assert!(scanner.scan(b"\x1b]0;[6n title\x07").is_empty());
        // But an ESC ends the string, so a real query right after it matches.
        assert_eq!(
            scanner.scan(b"\x1b]0;t\x1b\\\x1b[6n"),
            vec![DetectedQuery {
                end: 11,
                query: TerminalQuery::CursorPosition
            }]
        );
    }

    #[test]
    fn oversized_parameters_never_match() {
        let mut scanner = QueryScanner::new();
        assert!(scanner.scan(b"\x1b[0;0;0;0;0;0;0;6n").is_empty());
    }

    #[test]
    fn replies_are_one_based_and_match_ghostty() {
        assert_eq!(
            TerminalQuery::CursorPosition.reply(Some((5, 0))),
            b"\x1b[1;6R"
        );
        assert_eq!(TerminalQuery::CursorPosition.reply(None), b"\x1b[1;1R");
        assert_eq!(TerminalQuery::OperatingStatus.reply(None), b"\x1b[0n");
        assert_eq!(
            TerminalQuery::PrimaryDeviceAttributes.reply(None),
            b"\x1b[?62;22c"
        );
    }
}
