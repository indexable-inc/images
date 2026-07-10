//! Packet framing above the compression boundary.
//!
//! Until the server's login `SetCompression` arrives, a frame is `VarInt`
//! length + payload ([`mc_protocol::varint`]). Afterwards the payload grows a
//! second `VarInt` — the uncompressed size, `0` meaning "sent as-is" (only
//! legal below the negotiated threshold) — and everything past it is one
//! zlib stream.

use std::io::{self, Read, Write};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use mc_protocol::varint::{read_frame, read_varint, write_frame, write_varint};

/// Ceiling on a single packet, compressed or not. The vanilla protocol caps
/// frames at 2^21 - 1 bytes (the widest three-byte `VarInt`); configuration
/// registry data is the biggest thing a server sends and sits well below it.
pub const MAX_PACKET: usize = (1 << 21) - 1;

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// A packet transport in either compression mode. Packets are always handed
/// in and out in their logical form: `VarInt` packet id + body, uncompressed.
pub struct Framed<S> {
    transport: S,
    /// `Some` once the server's login `SetCompression` arrives.
    threshold: Option<usize>,
}

impl<S: Read + Write> Framed<S> {
    pub const fn new(transport: S) -> Self {
        Self {
            transport,
            threshold: None,
        }
    }

    /// The underlying transport, for out-of-band control like socket
    /// timeouts.
    pub const fn transport(&self) -> &S {
        &self.transport
    }

    /// Switches every subsequent [`Self::send`]/[`Self::recv`] to compressed
    /// framing. Called when the server's `SetCompression` arrives — that
    /// packet itself is still plain-framed.
    pub const fn enable_compression(&mut self, threshold: usize) {
        self.threshold = Some(threshold);
    }

    /// Sends one packet.
    ///
    /// # Errors
    ///
    /// Whatever the transport write fails with;
    /// [`io::ErrorKind::InvalidData`] on a packet longer than the frame
    /// format can carry.
    pub fn send(&mut self, packet: &[u8]) -> io::Result<()> {
        let Some(threshold) = self.threshold else {
            return write_frame(&mut self.transport, packet);
        };
        let mut payload = Vec::with_capacity(packet.len() + 1);
        if packet.len() < threshold {
            write_varint(&mut payload, 0);
            payload.extend_from_slice(packet);
        } else {
            let uncompressed = i32::try_from(packet.len())
                .map_err(|_| invalid("packet too long to frame".to_owned()))?;
            write_varint(&mut payload, uncompressed);
            let mut encoder = ZlibEncoder::new(payload, Compression::default());
            encoder.write_all(packet)?;
            payload = encoder.finish()?;
        }
        write_frame(&mut self.transport, &payload)
    }

    /// Receives one packet, undoing compressed framing when enabled.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidData`] on frames or declared sizes past
    /// [`MAX_PACKET`], or a zlib stream that does not inflate to its declared
    /// size; otherwise whatever the transport read fails with.
    pub fn recv(&mut self) -> io::Result<Vec<u8>> {
        let payload = read_frame(&mut self.transport, MAX_PACKET)?;
        if self.threshold.is_none() {
            return Ok(payload);
        }
        let mut rest = payload.as_slice();
        let declared = read_varint(&mut rest)?;
        if declared == 0 {
            return Ok(rest.to_vec());
        }
        let expected = usize::try_from(declared)
            .map_err(|_| invalid("negative uncompressed length".to_owned()))?;
        if expected > MAX_PACKET {
            return Err(invalid(format!(
                "uncompressed packet of {expected} bytes exceeds limit of {MAX_PACKET}"
            )));
        }
        let mut packet = Vec::with_capacity(expected);
        // +1 so inflating past the declared size is observable (and rejected)
        // instead of silently truncated; the bound keeps a hostile peer from
        // zip-bombing us regardless of what the prefix claims.
        let cap = u64::try_from(expected + 1)
            .map_err(|_| invalid("uncompressed length overflows".to_owned()))?;
        ZlibDecoder::new(rest).take(cap).read_to_end(&mut packet)?;
        if packet.len() != expected {
            return Err(invalid(format!(
                "packet inflated to {} bytes, expected {expected}",
                packet.len()
            )));
        }
        Ok(packet)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// In-memory transport: reads from a fixed script, appends writes.
    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Duplex {
        fn new(input: Vec<u8>) -> Self {
            Self {
                input: Cursor::new(input),
                output: Vec::new(),
            }
        }
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn zlib(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).expect("compress");
        encoder.finish().expect("finish")
    }

    #[test]
    fn plain_roundtrip() {
        let mut sender = Framed::new(Duplex::new(Vec::new()));
        sender.send(&[0x42, 1, 2, 3]).expect("send");
        let wire = sender.transport.output.clone();
        assert_eq!(wire, [4, 0x42, 1, 2, 3], "VarInt length + payload");

        let mut receiver = Framed::new(Duplex::new(wire));
        assert_eq!(receiver.recv().expect("recv"), [0x42, 1, 2, 3]);
    }

    #[test]
    fn compressed_send_below_threshold_is_marked_uncompressed() {
        let mut framed = Framed::new(Duplex::new(Vec::new()));
        framed.enable_compression(256);
        framed.send(&[0x42, 1, 2, 3]).expect("send");
        // Frame: length 5, then data-length 0 (uncompressed), then payload.
        assert_eq!(framed.transport.output, [5, 0, 0x42, 1, 2, 3]);
    }

    #[test]
    fn compressed_roundtrip_above_threshold() {
        let packet: Vec<u8> = std::iter::once(0x42)
            .chain(std::iter::repeat_n(7, 511))
            .collect();
        let mut sender = Framed::new(Duplex::new(Vec::new()));
        sender.enable_compression(256);
        sender.send(&packet).expect("send");

        let mut receiver = Framed::new(Duplex::new(sender.transport.output.clone()));
        receiver.enable_compression(256);
        assert_eq!(receiver.recv().expect("recv"), packet);
    }

    #[test]
    fn rejects_inflation_size_mismatch() {
        // Declares 8 uncompressed bytes but the stream holds 4.
        let mut payload = Vec::new();
        write_varint(&mut payload, 8);
        payload.extend_from_slice(&zlib(&[1, 2, 3, 4]));
        let mut wire = Vec::new();
        write_frame(&mut wire, &payload).expect("frame");

        let mut receiver = Framed::new(Duplex::new(wire));
        receiver.enable_compression(256);
        let err = receiver.recv().expect_err("must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_oversized_declaration() {
        let mut payload = Vec::new();
        write_varint(&mut payload, 1 << 22);
        payload.extend_from_slice(&zlib(&[0]));
        let mut wire = Vec::new();
        write_frame(&mut wire, &payload).expect("frame");

        let mut receiver = Framed::new(Duplex::new(wire));
        receiver.enable_compression(256);
        let err = receiver.recv().expect_err("must reject");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
