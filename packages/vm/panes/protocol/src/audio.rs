//! Audio side-channel: the guest's mixed PCM output, played on the macOS
//! host (index#1686).
//!
//! A SECOND vsock port ([`VSOCK_PORT`], 7102) carries the same
//! `[u32 LE len][postcard]` framing as the window stream (via
//! [`crate::write_msg`] / [`crate::read_msg_bounded`]) but its own message
//! enums and its own version pair: the window protocol on port 7100 stays
//! untouched at 1.x, and a peer built without the audio half simply never
//! connects this port. Same handshake discipline as the window stream: both
//! sides send their Hello immediately on connect (no speak-first ordering),
//! each validates the peer major before anything else and hangs up on
//! mismatch; the append-only postcard variant rules from the crate root apply
//! here too.
//!
//! Design constraints this encodes:
//! - The guest is the clock: `PipeWire`'s null sink drives capture, so PCM
//!   flows at a steady real-time rate (48 kHz s16le stereo is ~188 KiB/s,
//!   negligible next to the frame stream). The host paces nothing back; its
//!   jitter buffer absorbs transport jitter and clock drift (see
//!   `panes-host`). No acks, no timestamps: game audio wants the lowest
//!   latency the buffer allows, not A/V sync (index#1686).
//! - The stream format rides in the guest's Hello and is fixed for the
//!   connection's lifetime; a format change is a reconnect. That keeps the
//!   host side free of mid-stream renegotiation state.

use serde::{Deserialize, Serialize};

/// Peers refuse a mismatched major and hang up (see the crate-root rules on
/// postcard's unknown-variant intolerance; new variants are append-only and
/// gated on the peer's advertised minor).
pub const VERSION_MAJOR: u16 = 1;
pub const VERSION_MINOR: u16 = 0;

/// Guest vsock port the audio daemon listens on. Distinct from the window
/// stream's 7100 so the window protocol needs no change at all; 7101 is taken
/// by smoke tests.
pub const VSOCK_PORT: u32 = 7102;

/// Cap one audio message at 256 KiB.
///
/// The daemon sends PCM in ~10 ms chunks (a few KiB), so this fits any
/// legitimate frame with two orders of magnitude of headroom while keeping
/// what a hostile length prefix can make a reader allocate far below the
/// window stream's 64 MB [`crate::MAX_FRAME`].
pub const MAX_FRAME: usize = 256 * 1024;

/// Encoding of one PCM sample on the wire. Explicitly little-endian: both
/// current peers are LE, but the wire format must not depend on that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SampleFormat {
    /// Signed 16-bit little-endian, interleaved by channel.
    S16le,
}

impl SampleFormat {
    #[must_use]
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::S16le => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToHost {
    /// Sent once, immediately on connect. `rate`/`channels`/`format` describe
    /// every following [`ToHost::Pcm`] payload for the connection's lifetime.
    Hello {
        major: u16,
        minor: u16,
        /// Sample rate in Hz (48000 in the shipped guest).
        rate: u32,
        /// Interleaved channel count (2 in the shipped guest).
        channels: u16,
        format: SampleFormat,
    },
    /// One chunk of the guest mix, in the Hello's format. Chunk size is the
    /// sender's choice (whatever the capture socket produced, ~10 ms in
    /// practice); the payload must be a whole number of interleaved sample
    /// frames, i.e. a multiple of `channels * format.bytes_per_sample()`.
    Pcm {
        #[serde(with = "serde_bytes")]
        payload: Vec<u8>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToGuest {
    /// Sent once, immediately on connect.
    Hello { major: u16, minor: u16 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WireError, read_msg_bounded, write_msg};

    #[test]
    fn hello_roundtrips() {
        let mut buf = Vec::new();
        let msg = ToHost::Hello {
            major: VERSION_MAJOR,
            minor: VERSION_MINOR,
            rate: 48000,
            channels: 2,
            format: SampleFormat::S16le,
        };
        write_msg(&mut buf, &msg).unwrap();
        let back: ToHost = read_msg_bounded(&mut buf.as_slice(), MAX_FRAME).unwrap();
        let ToHost::Hello { major: 1, minor: 0, rate: 48000, channels: 2, format } = back else {
            panic!("wrong variant");
        };
        assert_eq!(format, SampleFormat::S16le);
    }

    #[test]
    fn pcm_roundtrips() {
        let mut buf = Vec::new();
        let payload: Vec<u8> = (0..=255).collect();
        write_msg(&mut buf, &ToHost::Pcm { payload: payload.clone() }).unwrap();
        let back: ToHost = read_msg_bounded(&mut buf.as_slice(), MAX_FRAME).unwrap();
        let ToHost::Pcm { payload: got } = back else {
            panic!("wrong variant");
        };
        assert_eq!(got, payload);
    }

    #[test]
    fn bounded_read_rejects_prefix_past_audio_cap() {
        // A prefix legal for the window stream (< 64 MB) must still be
        // rejected by an audio reader without allocating.
        let mut buf = Vec::new();
        buf.extend_from_slice(&u32::try_from(MAX_FRAME + 1).expect("fits u32").to_le_bytes());
        let err = read_msg_bounded::<ToHost>(&mut buf.as_slice(), MAX_FRAME).unwrap_err();
        assert!(matches!(err, WireError::TooLarge(n) if n == MAX_FRAME + 1));
    }

    #[test]
    fn to_guest_hello_roundtrips() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &ToGuest::Hello { major: 1, minor: 0 }).unwrap();
        let back: ToGuest = read_msg_bounded(&mut buf.as_slice(), MAX_FRAME).unwrap();
        assert!(matches!(back, ToGuest::Hello { major: 1, minor: 0 }));
    }
}
