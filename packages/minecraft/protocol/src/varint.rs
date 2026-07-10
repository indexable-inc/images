//! Minecraft protocol `VarInt`: the 7-bits-per-byte, little-endian-group,
//! continuation-bit integer every Java Edition packet frame starts with.
//!
//! The wire format caps `VarInt`s at 5 bytes (35 payload bits for an i32);
//! a longer run is a protocol violation, not a bigger number.

use std::io::{self, Read, Write};

/// Longest legal `VarInt` encoding of an i32.
pub const MAX_VARINT_LEN: usize = 5;

/// Appends the `VarInt` encoding of `value` to `out`.
pub fn write_varint(out: &mut Vec<u8>, value: i32) {
    // The encoding operates on the two's-complement bit pattern, so negative
    // values always take the full 5 bytes rather than looping forever.
    let mut rest = value.cast_unsigned();
    loop {
        // Masked to 7 bits, so the truncation keeps every set bit.
        #[allow(clippy::cast_possible_truncation)]
        let byte = (rest & 0x7F) as u8;
        rest >>= 7;
        if rest == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Reads one `VarInt` from `reader`.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidData`] when a sixth continuation byte appears;
/// otherwise whatever the underlying reader fails with (EOF included).
pub fn read_varint(reader: &mut impl Read) -> io::Result<i32> {
    let mut value: u32 = 0;
    for shift in 0..MAX_VARINT_LEN {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte)?;
        value |= u32::from(byte[0] & 0x7F) << (7 * shift);
        if byte[0] & 0x80 == 0 {
            return Ok(value.cast_signed());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "VarInt exceeds 5 bytes",
    ))
}

/// Writes `payload` as one length-prefixed packet frame: `VarInt` length, then
/// the payload (which itself starts with the `VarInt` packet id).
///
/// # Errors
///
/// [`io::ErrorKind::InvalidData`] on a payload longer than `i32::MAX` bytes;
/// otherwise whatever the underlying writer fails with.
pub fn write_frame(writer: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    let mut frame = Vec::with_capacity(payload.len() + MAX_VARINT_LEN);
    write_varint(
        &mut frame,
        i32::try_from(payload.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "packet too long"))?,
    );
    frame.extend_from_slice(payload);
    writer.write_all(&frame)
}

/// Reads one length-prefixed packet frame, returning the payload bytes.
///
/// `max_len` bounds the declared length so a hostile or corrupt peer cannot
/// make us allocate unbounded memory off a single frame header.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidData`] on a negative declared length or one past
/// `max_len`; otherwise whatever the underlying reader fails with.
pub fn read_frame(reader: &mut impl Read, max_len: usize) -> io::Result<Vec<u8>> {
    let len = read_varint(reader)?;
    let len = usize::try_from(len)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "negative frame length"))?;
    if len > max_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds limit of {max_len}"),
        ));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn roundtrip(value: i32, expected: &[u8]) {
        let mut out = Vec::new();
        write_varint(&mut out, value);
        assert_eq!(out, expected, "encoding of {value}");
        let decoded = read_varint(&mut out.as_slice()).expect("decode");
        assert_eq!(decoded, value, "roundtrip of {value}");
    }

    #[test]
    fn known_vectors() {
        // The canonical table from the protocol documentation.
        roundtrip(0, &[0x00]);
        roundtrip(1, &[0x01]);
        roundtrip(127, &[0x7F]);
        roundtrip(128, &[0x80, 0x01]);
        roundtrip(255, &[0xFF, 0x01]);
        roundtrip(25565, &[0xDD, 0xC7, 0x01]);
        roundtrip(2_097_151, &[0xFF, 0xFF, 0x7F]);
        roundtrip(i32::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]);
        roundtrip(-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
        roundtrip(i32::MIN, &[0x80, 0x80, 0x80, 0x80, 0x08]);
    }

    #[test]
    fn rejects_overlong() {
        let overlong = [0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        let err = read_varint(&mut overlong.as_slice()).expect_err("must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn frame_roundtrip() {
        let payload = b"\x00hello".to_vec();
        let mut wire = Vec::new();
        write_frame(&mut wire, &payload).expect("write");
        let back = read_frame(&mut wire.as_slice(), 1024).expect("read");
        assert_eq!(back, payload);
    }

    #[test]
    fn frame_length_limit_holds() {
        let mut wire = Vec::new();
        write_varint(&mut wire, 1 << 20);
        let err = read_frame(&mut wire.as_slice(), 1024).expect_err("must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
