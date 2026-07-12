//! Wire formats: every serialized shape peers exchange, in one place.
//!
//! TCP messages are length-prefixed frames (`u32` little-endian payload
//! length, then a one-byte tag, then the payload). UDP carries fixed-size
//! clock ping packets. Nothing else on this crate hand-assembles bytes;
//! everything routes through these encoders and decoders.

use anyhow::{Context as _, Result, bail, ensure};
use audio_blob::BlobHash;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// Cap on a single frame so a bad peer cannot balloon memory. Instrument
/// modules are small (tens of KiB); 32 MiB leaves generous headroom.
pub const MAX_FRAME_BYTES: u32 = 32 * 1024 * 1024;

/// Session metadata exchanged once per TCP connection, in both directions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    /// The sender's stable peer identity.
    pub peer_id: u64,
    /// UDP port the sender answers clock pings on.
    pub udp_port: u16,
    /// The session epoch on the *sender's* local clock. A follower
    /// translates its leader's epoch (`SharedClock::local_epoch_micros`),
    /// so a peer that can only reach a follower still converges on the
    /// session timeline by pinging that follower's clock.
    pub epoch_micros: i64,
    /// Sample rate the sender's session runs at.
    pub sample_rate: u32,
}

/// One TCP frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Session metadata; first frame in each direction.
    Hello(Hello),
    /// A Loro update delta for the shared score.
    ScoreUpdate(Vec<u8>),
    /// "Send me the blob with this hash."
    BlobRequest(BlobHash),
    /// Blob bytes, preceded by their hash for verification.
    Blob(BlobHash, Vec<u8>),
}

impl PartialEq for Hello {
    fn eq(&self, other: &Self) -> bool {
        self.peer_id == other.peer_id
            && self.udp_port == other.udp_port
            && self.epoch_micros == other.epoch_micros
            && self.sample_rate == other.sample_rate
    }
}

impl Eq for Hello {}

const TAG_HELLO: u8 = 0x01;
const TAG_SCORE_UPDATE: u8 = 0x02;
const TAG_BLOB_REQUEST: u8 = 0x03;
const TAG_BLOB: u8 = 0x04;

impl Message {
    /// Encode into a self-delimiting frame.
    ///
    /// # Errors
    /// Fails when a `Hello` cannot serialize (never in practice).
    pub fn encode(&self) -> Result<Vec<u8>> {
        let (tag, payload): (u8, Vec<u8>) = match self {
            Self::Hello(hello) => (TAG_HELLO, serde_json::to_vec(hello)?),
            Self::ScoreUpdate(update) => (TAG_SCORE_UPDATE, update.clone()),
            Self::BlobRequest(hash) => (TAG_BLOB_REQUEST, hash.as_bytes().to_vec()),
            Self::Blob(hash, bytes) => {
                let mut payload = hash.as_bytes().to_vec();
                payload.extend_from_slice(bytes);
                (TAG_BLOB, payload)
            }
        };
        let length = u32::try_from(payload.len() + 1).context("frame exceeds u32")?;
        ensure!(length <= MAX_FRAME_BYTES, "frame of {length} bytes exceeds cap");
        let mut frame = Vec::with_capacity(payload.len() + 5);
        frame.extend_from_slice(&length.to_le_bytes());
        frame.push(tag);
        frame.extend_from_slice(&payload);
        Ok(frame)
    }

    /// Decode one frame body (tag byte plus payload, length prefix already
    /// consumed).
    ///
    /// # Errors
    /// Fails on an unknown tag or a malformed payload.
    pub fn decode(body: &[u8]) -> Result<Self> {
        let (&tag, payload) = body.split_first().context("empty frame")?;
        Ok(match tag {
            TAG_HELLO => Self::Hello(serde_json::from_slice(payload)?),
            TAG_SCORE_UPDATE => Self::ScoreUpdate(payload.to_vec()),
            TAG_BLOB_REQUEST => Self::BlobRequest(hash_of(payload)?),
            TAG_BLOB => {
                ensure!(payload.len() >= 32, "blob frame shorter than its hash");
                let (hash, bytes) = payload.split_at(32);
                Self::Blob(hash_of(hash)?, bytes.to_vec())
            }
            unknown => bail!("unknown frame tag {unknown:#04x}"),
        })
    }

    /// Write one frame to a stream.
    ///
    /// # Errors
    /// Fails on encode or I/O errors.
    pub async fn write_to<W>(&self, writer: &mut W) -> Result<()>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        writer.write_all(&self.encode()?).await?;
        Ok(())
    }

    /// Read one frame from a stream; `None` on clean end-of-stream.
    ///
    /// # Errors
    /// Fails on I/O errors, an oversized frame, or a malformed payload.
    pub async fn read_from<R>(reader: &mut R) -> Result<Option<Self>>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut length = [0_u8; 4];
        match reader.read_exact(&mut length).await {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let length = u32::from_le_bytes(length);
        ensure!(
            (1..=MAX_FRAME_BYTES).contains(&length),
            "frame length {length} outside 1..={MAX_FRAME_BYTES}"
        );
        let mut body = vec![0_u8; length as usize];
        reader.read_exact(&mut body).await?;
        Ok(Some(Self::decode(&body)?))
    }
}

fn hash_of(bytes: &[u8]) -> Result<BlobHash> {
    let bytes: [u8; 32] = bytes.try_into().context("hash payload is not 32 bytes")?;
    Ok(BlobHash::from_bytes(bytes))
}

/// A clock ping over UDP.
///
/// The request carries the sender's send time; the reply echoes it plus the
/// responder's receive and reply times, the four timestamps NTP-style offset
/// estimation needs (the fourth is stamped by the requester on arrival).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ping {
    /// `t0`: requester's local send time in microseconds.
    Request { sent_micros: u64 },
    /// Echoed `t0` plus responder-local `t1` (received) and `t2` (replied).
    Reply {
        sent_micros: u64,
        received_micros: u64,
        replied_micros: u64,
    },
}

const PING_MAGIC: [u8; 4] = *b"saP1";
const PING_REQUEST: u8 = 0x01;
const PING_REPLY: u8 = 0x02;
/// Fixed packet size: magic + tag + three u64 slots (unused slots zero).
pub const PING_PACKET_BYTES: usize = 4 + 1 + 24;

impl Ping {
    /// Encode into a fixed-size datagram.
    #[must_use]
    pub fn encode(&self) -> [u8; PING_PACKET_BYTES] {
        let mut packet = [0_u8; PING_PACKET_BYTES];
        packet[..4].copy_from_slice(&PING_MAGIC);
        match *self {
            Self::Request { sent_micros } => {
                packet[4] = PING_REQUEST;
                packet[5..13].copy_from_slice(&sent_micros.to_le_bytes());
            }
            Self::Reply { sent_micros, received_micros, replied_micros } => {
                packet[4] = PING_REPLY;
                packet[5..13].copy_from_slice(&sent_micros.to_le_bytes());
                packet[13..21].copy_from_slice(&received_micros.to_le_bytes());
                packet[21..29].copy_from_slice(&replied_micros.to_le_bytes());
            }
        }
        packet
    }

    /// Decode a datagram; `None` for foreign traffic (wrong magic or size),
    /// which a listener silently drops.
    ///
    /// # Errors
    /// Fails on a corrupt packet that carries our magic but a bad tag.
    ///
    /// # Panics
    /// Never: the eight-byte reads sit at fixed offsets inside the
    /// length-checked packet.
    pub fn decode(packet: &[u8]) -> Result<Option<Self>> {
        if packet.len() != PING_PACKET_BYTES || packet[..4] != PING_MAGIC {
            return Ok(None);
        }
        let u64_at = |offset: usize| {
            u64::from_le_bytes(packet[offset..offset + 8].try_into().expect("fixed slice"))
        };
        match packet[4] {
            PING_REQUEST => Ok(Some(Self::Request { sent_micros: u64_at(5) })),
            PING_REPLY => Ok(Some(Self::Reply {
                sent_micros: u64_at(5),
                received_micros: u64_at(13),
                replied_micros: u64_at(21),
            })),
            unknown => bail!("ping packet with unknown tag {unknown:#04x}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn messages_roundtrip() -> Result<()> {
        let hash = BlobHash::of(b"module");
        let messages = [
            Message::Hello(Hello {
                peer_id: 7,
                udp_port: 4242,
                epoch_micros: 1_000_000,
                sample_rate: 48_000,
            }),
            Message::ScoreUpdate(vec![1, 2, 3]),
            Message::BlobRequest(hash),
            Message::Blob(hash, b"module".to_vec()),
        ];
        let mut stream = Vec::new();
        for message in &messages {
            message.write_to(&mut stream).await?;
        }
        let mut reader = stream.as_slice();
        for message in &messages {
            let decoded = Message::read_from(&mut reader).await?.expect("frame present");
            assert_eq!(&decoded, message);
        }
        assert_eq!(Message::read_from(&mut reader).await?, None, "clean EOF");
        Ok(())
    }

    #[test]
    fn pings_roundtrip_and_drop_foreign_traffic() -> Result<()> {
        let request = Ping::Request { sent_micros: 55 };
        let reply = Ping::Reply { sent_micros: 55, received_micros: 60, replied_micros: 61 };
        assert_eq!(Ping::decode(&request.encode())?, Some(request));
        assert_eq!(Ping::decode(&reply.encode())?, Some(reply));
        assert_eq!(Ping::decode(b"not a ping packet")?, None);
        Ok(())
    }

    #[test]
    fn oversized_frames_are_rejected() {
        let bytes = vec![0_u8; MAX_FRAME_BYTES as usize];
        let error = Message::ScoreUpdate(bytes).encode().expect_err("over cap");
        assert!(error.to_string().contains("exceeds"));
    }
}
