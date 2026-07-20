//! Extraction of the private ix-term OSC (number 5522, from index#3797) out
//! of a raw PTY byte stream.
//!
//! The `ixterm` CLI (`packages/ixterm`) writes `ESC ] 5522 ; open ; <abs
//! path> BEL` to the session pts in a single `write(2)`. The server scans the
//! PTY output for exactly that sequence, strips it from the bytes fed to the
//! VT engine, and surfaces each occurrence as an [`OscEvent`]. Interception
//! happens here, in front of libghostty-vt, because ghostty's OSC command
//! enum is closed: an unknown code like 5522 parses to `INVALID` with no
//! accessible payload, so the engine would swallow the sequence silently.
//! This scanner is also the seam that keeps the VT engine swappable — any
//! replacement engine sees the same cleaned stream.
//!
//! Everything that is not an `OSC 5522` sequence passes through byte for
//! byte, including every other OSC, so the engine sees exactly what the
//! application wrote. The scanner is streaming: a sequence split across
//! arbitrary read boundaries is still recognized, and bytes are only held
//! back while they are a live prefix of the opener.
//!
//! The CLI terminates with `BEL`; this scanner additionally accepts the 7-bit
//! string terminator `ESC \` so hand-written emitters that follow ECMA-48
//! framing work too. The 8-bit C1 forms (`0x9d` opener, `0x9c` ST) are not
//! recognized, matching the CLI and `ix_vt::OscTitleTracker`.

/// `ESC` (0x1b), the introducer of every escape sequence.
const ESC: u8 = 0x1b;
/// `BEL` (0x07), the OSC terminator the `ixterm` CLI emits.
const BEL: u8 = 0x07;
/// `CAN` (0x18): per ECMA-48 aborts an in-progress control string.
const CAN: u8 = 0x18;
/// `SUB` (0x1a): like `CAN`, aborts an in-progress control string.
const SUB: u8 = 0x1a;

/// The exact opener of the private sequence: `ESC ] 5522 ;`.
const PREFIX: &[u8] = b"\x1b]5522;";

/// Cap on the bytes buffered for one payload. Real payloads are `open;<abs
/// path>`, far below this; the cap bounds memory if a writer opens the
/// sequence and never terminates it.
const MAX_PAYLOAD: usize = 4096;

/// A parsed occurrence of the private OSC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscEvent {
    /// `OSC 5522 ; open ; <path>`: the session asked the server to open
    /// `path` (an absolute path to an HTML file) in the session's split view.
    Open(String),
    /// A 5522 sequence arrived but its payload was not a well-formed `open`
    /// request. The sequence is still stripped from the stream; the server
    /// renders the reason in the session UI (the issue's "no backchannel"
    /// error path).
    Malformed(String),
}

/// Where the scanner is within an `ESC ] 5522 ; … (BEL | ESC \)` sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Outside the private sequence; bytes pass through.
    Ground,
    /// The last `n` bytes matched `PREFIX[..n]`; they are held back until the
    /// prefix completes or diverges.
    Prefix(usize),
    /// Inside the payload of a recognized `ESC ] 5522 ;`.
    Payload,
    /// Saw `ESC` inside the payload: either the start of `ESC \` (ST) or an
    /// abort that opens a new escape sequence.
    PayloadEsc,
}

/// A streaming scanner that strips `OSC 5522` sequences from a byte stream.
pub struct Scanner {
    state: State,
    payload: Vec<u8>,
    /// The payload exceeded [`MAX_PAYLOAD`]; excess bytes are dropped and the
    /// sequence reports as [`OscEvent::Malformed`] at its terminator.
    overflowed: bool,
}

impl Scanner {
    /// Create a scanner in the ground state.
    pub const fn new() -> Self {
        Self {
            state: State::Ground,
            payload: Vec::new(),
            overflowed: false,
        }
    }

    /// Scan `input`, appending non-5522 bytes to `passthrough` and parsed
    /// 5522 sequences to `events`.
    pub fn feed(&mut self, input: &[u8], passthrough: &mut Vec<u8>, events: &mut Vec<OscEvent>) {
        for &byte in input {
            self.step(byte, passthrough, events);
        }
    }

    /// Flush bytes held as a live opener prefix at end of stream.
    ///
    /// A partial prefix was ordinary output that happened to end the stream;
    /// an unterminated payload has no such interpretation (its bytes were
    /// only ever addressed to the server) and is dropped.
    pub fn flush_pending(&mut self, passthrough: &mut Vec<u8>) {
        if let State::Prefix(matched) = self.state {
            passthrough.extend_from_slice(&PREFIX[..matched]);
        }
        self.payload.clear();
        self.overflowed = false;
        self.state = State::Ground;
    }

    /// Advance the state machine by one byte.
    ///
    /// The loop re-dispatches a byte after a state change that consumed no
    /// input (prefix divergence, payload abort), so held bytes are replayed
    /// without recursion.
    fn step(&mut self, byte: u8, passthrough: &mut Vec<u8>, events: &mut Vec<OscEvent>) {
        loop {
            match self.state {
                State::Ground => {
                    if byte == ESC {
                        self.state = State::Prefix(1);
                    } else {
                        passthrough.push(byte);
                    }
                    return;
                }
                State::Prefix(matched) => {
                    if byte == PREFIX[matched] {
                        self.state = if matched + 1 == PREFIX.len() {
                            self.payload.clear();
                            self.overflowed = false;
                            State::Payload
                        } else {
                            State::Prefix(matched + 1)
                        };
                        return;
                    }
                    // Divergence: the held bytes were ordinary output (for
                    // example another OSC's opener). Release them and rescan
                    // the current byte from the ground state.
                    passthrough.extend_from_slice(&PREFIX[..matched]);
                    self.state = State::Ground;
                }
                State::Payload => {
                    match byte {
                        BEL => {
                            self.finish(events);
                            self.state = State::Ground;
                        }
                        ESC => self.state = State::PayloadEsc,
                        // An aborted control string produces nothing; its
                        // bytes were never output.
                        CAN | SUB => {
                            self.payload.clear();
                            self.state = State::Ground;
                        }
                        _ => self.push_payload(byte),
                    }
                    return;
                }
                State::PayloadEsc => {
                    if byte == b'\\' {
                        self.finish(events);
                        self.state = State::Ground;
                        return;
                    }
                    // Per ECMA-48 an ESC that is not part of ST aborts the
                    // control string and opens a new escape sequence; rescan
                    // the current byte as the byte after that ESC.
                    self.payload.clear();
                    self.state = State::Prefix(1);
                }
            }
        }
    }

    fn push_payload(&mut self, byte: u8) {
        if self.payload.len() >= MAX_PAYLOAD {
            self.overflowed = true;
        } else {
            self.payload.push(byte);
        }
    }

    /// Terminate the current payload and emit its event.
    fn finish(&mut self, events: &mut Vec<OscEvent>) {
        let payload = std::mem::take(&mut self.payload);
        if std::mem::take(&mut self.overflowed) {
            events.push(OscEvent::Malformed(format!(
                "OSC 5522 payload exceeded {MAX_PAYLOAD} bytes"
            )));
            return;
        }
        events.push(parse_payload(&payload));
    }
}

/// Parse a complete 5522 payload (`open;<abs path>`).
fn parse_payload(payload: &[u8]) -> OscEvent {
    let Some(path) = payload.strip_prefix(b"open;") else {
        return OscEvent::Malformed(format!(
            "unknown OSC 5522 request {:?} (expected \"open;<abs path>\")",
            String::from_utf8_lossy(payload)
        ));
    };
    let Ok(path) = std::str::from_utf8(path) else {
        return OscEvent::Malformed("OSC 5522 open path is not UTF-8".to_owned());
    };
    if !path.starts_with('/') {
        return OscEvent::Malformed(format!("OSC 5522 open path is not absolute: {path:?}"));
    }
    OscEvent::Open(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{OscEvent, Scanner};

    /// Run `input` through a fresh scanner in one feed.
    fn scan(input: &[u8]) -> (Vec<u8>, Vec<OscEvent>) {
        let mut scanner = Scanner::new();
        let mut passthrough = Vec::new();
        let mut events = Vec::new();
        scanner.feed(input, &mut passthrough, &mut events);
        (passthrough, events)
    }

    #[test]
    fn plain_bytes_pass_through_byte_exact() {
        let input = b"hello \x1b[31mred\x1b[0m world\r\n";
        let (out, events) = scan(input);
        assert_eq!(out, input);
        assert_eq!(events, vec![]);
    }

    #[test]
    fn bel_terminated_open_is_extracted() {
        // The exact bytes `ixterm open` writes (packages/ixterm/src/osc.rs).
        let (out, events) = scan(b"before\x1b]5522;open;/tmp/report.html\x07after");
        assert_eq!(out, b"beforeafter");
        assert_eq!(events, vec![OscEvent::Open("/tmp/report.html".to_owned())]);
    }

    #[test]
    fn st_terminated_open_is_extracted() {
        let (out, events) = scan(b"a\x1b]5522;open;/x/y.html\x1b\\b");
        assert_eq!(out, b"ab");
        assert_eq!(events, vec![OscEvent::Open("/x/y.html".to_owned())]);
    }

    #[test]
    fn every_split_boundary_scans_identically() {
        let input: &[u8] =
            b"pre\x1b]5522;open;/tmp/x.html\x07mid\x1b]5522;open;/tmp/y.html\x1b\\post";
        for split in 0..=input.len() {
            let mut scanner = Scanner::new();
            let mut out = Vec::new();
            let mut events = Vec::new();
            scanner.feed(&input[..split], &mut out, &mut events);
            scanner.feed(&input[split..], &mut out, &mut events);
            assert_eq!(out, b"premidpost", "split at {split}");
            assert_eq!(
                events,
                vec![
                    OscEvent::Open("/tmp/x.html".to_owned()),
                    OscEvent::Open("/tmp/y.html".to_owned()),
                ],
                "split at {split}"
            );
        }
    }

    #[test]
    fn other_osc_sequences_pass_through_untouched() {
        for input in [
            b"\x1b]0;window title\x07".as_slice(),
            b"\x1b]52;c;aGVsbG8=\x1b\\".as_slice(),
            // A code that shares the 5522 digit prefix but diverges.
            b"\x1b]55221;x\x07".as_slice(),
            b"\x1b]55;x\x07".as_slice(),
        ] {
            let (out, events) = scan(input);
            assert_eq!(out, input);
            assert_eq!(events, vec![]);
        }
    }

    #[test]
    fn double_escape_releases_the_first() {
        let (out, events) = scan(b"\x1b\x1b]5522;open;/a.html\x07");
        assert_eq!(out, b"\x1b");
        assert_eq!(events, vec![OscEvent::Open("/a.html".to_owned())]);
    }

    #[test]
    fn unknown_verb_is_malformed() {
        let (out, events) = scan(b"\x1b]5522;close;/a.html\x07");
        assert_eq!(out, b"");
        assert!(matches!(&events[..], [OscEvent::Malformed(_)]));
    }

    #[test]
    fn relative_path_is_malformed() {
        let (_, events) = scan(b"\x1b]5522;open;tmp/x.html\x07");
        assert!(matches!(&events[..], [OscEvent::Malformed(_)]));
    }

    #[test]
    fn can_aborts_the_sequence_silently() {
        let (out, events) = scan(b"a\x1b]5522;open;/x\x18b");
        assert_eq!(out, b"ab");
        assert_eq!(events, vec![]);
    }

    #[test]
    fn esc_abort_opens_a_new_sequence() {
        // The aborting ESC starts a fresh, complete 5522 sequence.
        let (out, events) = scan(b"\x1b]5522;open;/dropped\x1b]5522;open;/kept.html\x07");
        assert_eq!(out, b"");
        assert_eq!(events, vec![OscEvent::Open("/kept.html".to_owned())]);
    }

    #[test]
    fn oversized_payload_is_malformed_not_unbounded() {
        let mut input = b"\x1b]5522;open;/".to_vec();
        input.extend(std::iter::repeat_n(b'a', super::MAX_PAYLOAD * 2));
        input.push(0x07);
        let (out, events) = scan(&input);
        assert_eq!(out, b"");
        assert!(matches!(&events[..], [OscEvent::Malformed(_)]));
    }

    #[test]
    fn flush_pending_releases_a_partial_prefix() {
        let mut scanner = Scanner::new();
        let mut out = Vec::new();
        let mut events = Vec::new();
        scanner.feed(b"tail\x1b]55", &mut out, &mut events);
        assert_eq!(out, b"tail");
        scanner.flush_pending(&mut out);
        assert_eq!(out, b"tail\x1b]55");
        assert_eq!(events, vec![]);
    }
}
