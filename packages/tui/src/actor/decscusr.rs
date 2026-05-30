//! Incremental sniffer for the `DECSCUSR` cursor-shape escape.
//!
//! vt100 parses and applies cursor *position* but does not model cursor
//! *shape*, so the actor watches the same byte stream it feeds the parser and
//! folds out the latest `DECSCUSR` sequence: `CSI Ps SP q`, i.e. `ESC [`, an
//! optional decimal parameter, a space (`0x20`), then `q`. The scanner is fed
//! the raw PTY reads in order, and a sequence split across two reads resumes
//! from the carried state, so a child that emits the escape in pieces is still
//! recognized.
//!
//! Only this one final byte (`q`) is matched. Any other intermediate or final
//! byte abandons the in-progress sequence, so unrelated CSI sequences (colors,
//! cursor moves) never produce a false shape change.

use crate::types::CursorShape;

/// Where the scanner is inside a candidate `CSI Ps SP q` sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    /// No escape in progress.
    #[default]
    Ground,
    /// Saw `ESC`, waiting for `[`.
    Escape,
    /// Inside `CSI`, accumulating the decimal parameter (and looking for the
    /// space that ends it).
    Params,
    /// Saw the space after the parameter, waiting for the final `q`.
    Intermediate,
}

/// A byte-stream scanner that surfaces the most recent `DECSCUSR` shape.
///
/// One scanner lives per terminal actor. Feed it every PTY read with
/// [`Scanner::feed`]; it returns `Some(shape)` for each `DECSCUSR` it completes
/// so the caller can publish the latest one.
#[derive(Debug, Default)]
pub struct Scanner {
    state: State,
    /// The decimal parameter accumulated in [`State::Params`], capped so a
    /// hostile child cannot overflow it; real parameters are one or two digits.
    param: u16,
}

const ESC: u8 = 0x1b;
const CSI_OPEN: u8 = b'[';
const SPACE: u8 = b' ';
const FINAL: u8 = b'q';

impl Scanner {
    /// Scan `bytes`, returning the last completed `DECSCUSR` shape if any.
    ///
    /// Returns the final shape in the buffer rather than every intermediate one
    /// because the actor only tracks the current shape; an earlier escape in
    /// the same read is immediately superseded.
    pub fn feed(&mut self, bytes: &[u8]) -> Option<CursorShape> {
        let mut latest = None;
        for &byte in bytes {
            if let Some(shape) = self.step(byte) {
                latest = Some(shape);
            }
        }
        latest
    }

    fn step(&mut self, byte: u8) -> Option<CursorShape> {
        match self.state {
            State::Ground => {
                if byte == ESC {
                    self.state = State::Escape;
                }
                None
            }
            State::Escape => {
                self.state = if byte == CSI_OPEN {
                    self.param = 0;
                    State::Params
                } else if byte == ESC {
                    State::Escape
                } else {
                    State::Ground
                };
                None
            }
            State::Params => {
                if byte.is_ascii_digit() {
                    self.param = self
                        .param
                        .saturating_mul(10)
                        .saturating_add(u16::from(byte - b'0'));
                } else if byte == SPACE {
                    self.state = State::Intermediate;
                } else {
                    // Any other byte (another intermediate, a different final,
                    // or a stray ESC) ends this candidate.
                    self.restart_on(byte);
                }
                None
            }
            State::Intermediate => {
                let shape = (byte == FINAL).then(|| CursorShape::from_decscusr(self.param));
                self.restart_on(byte);
                shape
            }
        }
    }

    /// Reset to ground, but honor a fresh `ESC` so back-to-back sequences with
    /// no separator are not dropped.
    const fn restart_on(&mut self, byte: u8) {
        self.state = if byte == ESC { State::Escape } else { State::Ground };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_decscusr_parameter_to_a_shape() {
        // The boundary between the three shape buckets, including the unknown
        // fallback to block.
        let cases = [
            (b"\x1b[0 q".as_slice(), CursorShape::Block),
            (b"\x1b[2 q", CursorShape::Block),
            (b"\x1b[3 q", CursorShape::Underline),
            (b"\x1b[4 q", CursorShape::Underline),
            (b"\x1b[5 q", CursorShape::Bar),
            (b"\x1b[6 q", CursorShape::Bar),
            (b"\x1b[ q", CursorShape::Block),
            (b"\x1b[99 q", CursorShape::Block),
        ];
        for (bytes, want) in cases {
            let mut scanner = Scanner::default();
            assert_eq!(scanner.feed(bytes), Some(want), "for {bytes:?}");
        }
    }

    #[test]
    fn ignores_unrelated_csi_sequences() {
        // A color SGR and a cursor move must not be read as a shape change.
        let mut scanner = Scanner::default();
        assert_eq!(scanner.feed(b"\x1b[1;31mhi\x1b[2;5H"), None);
    }

    #[test]
    fn resumes_a_sequence_split_across_feeds() {
        let mut scanner = Scanner::default();
        assert_eq!(scanner.feed(b"\x1b[5"), None);
        assert_eq!(scanner.feed(b" q"), Some(CursorShape::Bar));
    }

    #[test]
    fn returns_the_last_shape_in_one_feed() {
        let mut scanner = Scanner::default();
        assert_eq!(
            scanner.feed(b"\x1b[3 q text \x1b[6 q"),
            Some(CursorShape::Bar)
        );
    }
}
