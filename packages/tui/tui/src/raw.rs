//! Ring buffer of the most recent raw PTY output bytes.
//!
//! The VT engine consumes the byte stream destructively (escape sequences
//! become screen state and vanish), so debugging "what did the child actually
//! emit" (index#3110) needs the bytes kept as received, pre-parse. The ring is
//! bounded like scrollback: once full, the oldest bytes fall off.

use std::collections::VecDeque;

/// Bytes retained per terminal. A full 80x24 styled redraw is a few KB, so
/// this holds on the order of a hundred recent frames; a byte cap (rather
/// than scrollback's line cap) because the raw stream has no line structure.
pub(crate) const LIMIT: usize = 1 << 20;

/// The most recent raw PTY output bytes, as received and before VT parsing.
pub(crate) struct RawOutput {
    bytes: VecDeque<u8>,
    limit: usize,
}

impl RawOutput {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            limit,
        }
    }

    /// Append a chunk, dropping the oldest bytes past the limit.
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk);
        if self.bytes.len() > self.limit {
            self.bytes.drain(..self.bytes.len() - self.limit);
        }
    }

    /// The buffered bytes, oldest first; with `tail`, only the trailing
    /// `tail` bytes.
    pub(crate) fn bytes(&self, tail: Option<usize>) -> Vec<u8> {
        let skip = tail.map_or(0, |tail| self.bytes.len().saturating_sub(tail));
        self.bytes.iter().skip(skip).copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::RawOutput;

    #[test]
    fn keeps_only_the_most_recent_bytes() {
        let mut ring = RawOutput::new(4);
        ring.push(b"abc");
        ring.push(b"def");
        assert_eq!(ring.bytes(None), b"cdef");
    }

    #[test]
    fn oversized_chunk_keeps_its_own_tail() {
        let mut ring = RawOutput::new(4);
        ring.push(b"0123456789");
        assert_eq!(ring.bytes(None), b"6789");
    }

    #[test]
    fn tail_returns_trailing_bytes_and_tolerates_overshoot() {
        let mut ring = RawOutput::new(16);
        ring.push(b"hello");
        assert_eq!(ring.bytes(Some(3)), b"llo");
        assert_eq!(ring.bytes(Some(99)), b"hello");
        assert_eq!(ring.bytes(Some(0)), b"");
    }
}
