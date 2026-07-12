//! Length-prefixed protocol strings.
//!
//! The other primitive (besides `VarInt`s and frames, [`crate::varint`])
//! that every Java Edition packet is built from: a `VarInt` byte length
//! followed by that many bytes of UTF-8.

use std::io::{self, Read};

use crate::varint::{read_varint, write_varint};

/// Appends the length-prefixed UTF-8 encoding of `value` to `out`.
///
/// # Panics
///
/// On a string longer than `i32::MAX` bytes. Callers pass protocol-sized
/// strings (hostnames, usernames, identifiers), which sit far below the
/// format's ceiling; a longer string is a caller bug, not a runtime
/// condition.
pub fn write_string(out: &mut Vec<u8>, value: &str) {
    let length = i32::try_from(value.len()).expect("string length exceeds i32::MAX");
    write_varint(out, length);
    out.extend_from_slice(value.as_bytes());
}

/// Reads one length-prefixed UTF-8 string from `reader`.
///
/// `max_len` bounds the declared byte length so a hostile or corrupt peer
/// cannot make us allocate unbounded memory off a single length prefix.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidData`] on a negative declared length, one past
/// `max_len`, or bytes that are not UTF-8; otherwise whatever the underlying
/// reader fails with (EOF included).
pub fn read_string(reader: &mut impl Read, max_len: usize) -> io::Result<String> {
    let declared = read_varint(reader)?;
    let length = usize::try_from(declared)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "negative string length"))?;
    if length > max_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("string of {length} bytes exceeds limit of {max_len}"),
        ));
    }
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "string is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips() {
        let mut wire = Vec::new();
        write_string(&mut wire, "mc.example.com");
        assert_eq!(wire[0], 14, "single-byte VarInt length prefix");
        let back = read_string(&mut wire.as_slice(), 1024).expect("read");
        assert_eq!(back, "mc.example.com");
    }

    #[test]
    fn empty_string_is_one_zero_byte() {
        let mut wire = Vec::new();
        write_string(&mut wire, "");
        assert_eq!(wire, [0x00]);
        assert_eq!(read_string(&mut wire.as_slice(), 16).expect("read"), "");
    }

    #[test]
    fn rejects_over_limit() {
        let mut wire = Vec::new();
        write_string(&mut wire, "0123456789");
        let err = read_string(&mut wire.as_slice(), 4).expect_err("must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_invalid_utf8() {
        let wire = [0x02, 0xC0, 0x00];
        let err = read_string(&mut wire.as_slice(), 16).expect_err("must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
