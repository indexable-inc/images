//! `ReplayMod` `.mcpr` writer.
//!
//! A replay is a zip with two mandatory entries. `recording.tmcpr` is the
//! clientbound packet stream: per packet, a big-endian u32 timestamp
//! (milliseconds since recording start), a big-endian u32 byte length, then
//! the raw packet (`VarInt` id + body — decompressed, decrypted).
//! `metaData.json` describes the stream (`ReplayStudio`'s `ReplayMetaData`,
//! file format version 14). `ReplayStudio` parses the stream starting in the
//! LOGIN state and derives transitions from the packets themselves
//! (`LoginSuccess` → configuration, `FinishConfiguration` → play), so a
//! recording should begin at `LoginSuccess`; anything the server never sent
//! simply does not exist in the replay.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

/// Everything `metaData.json` needs beyond what the recorder observes.
pub struct ReplayInfo {
    /// `host:port` the session was recorded from.
    pub server_name: String,
    /// Display name of the Minecraft version, e.g. `"26.2"`.
    pub mc_version: String,
    /// Numeric protocol version, e.g. 776. Mandatory since file format 13.
    pub protocol_version: i32,
}

/// Accumulates timestamped clientbound packets and writes the archive.
pub struct Recorder {
    /// Set by the first recorded packet; every timestamp is relative to it.
    epoch: Option<Instant>,
    /// The `recording.tmcpr` byte stream, built incrementally.
    stream: Vec<u8>,
    packets: usize,
    last_offset: Duration,
}

impl Recorder {
    pub const fn new() -> Self {
        Self {
            epoch: None,
            stream: Vec::new(),
            packets: 0,
            last_offset: Duration::ZERO,
        }
    }

    /// Appends one packet (logical form: `VarInt` id + body), stamped with
    /// the current time. The u32 fields saturate rather than fail: the wire
    /// framing caps packets far below 4 GiB, and a ~50-day recording has
    /// bigger problems than a pinned timestamp.
    pub fn record(&mut self, packet: &[u8]) {
        let epoch = *self.epoch.get_or_insert_with(Instant::now);
        self.last_offset = epoch.elapsed();
        let millis = saturating_millis(self.last_offset);
        // The wire framing caps packets far below 4 GiB; pinning is the
        // contract, not a silent default.
        #[allow(clippy::fallible_int_fallback)]
        let length = u32::try_from(packet.len()).unwrap_or(u32::MAX);
        self.stream.extend_from_slice(&millis.to_be_bytes());
        self.stream.extend_from_slice(&length.to_be_bytes());
        self.stream.extend_from_slice(packet);
        self.packets += 1;
    }

    pub const fn is_empty(&self) -> bool {
        self.packets == 0
    }

    pub const fn packets(&self) -> usize {
        self.packets
    }

    /// Timestamp offset of the most recent packet — the replay's duration.
    pub const fn duration(&self) -> Duration {
        self.last_offset
    }

    /// Writes the `.mcpr` archive.
    ///
    /// # Errors
    ///
    /// Filesystem and zip-encoding failures, annotated with `path`.
    pub fn write(&self, path: &Path, info: &ReplayInfo) -> anyhow::Result<()> {
        let file = File::create(path)
            .with_context(|| format!("creating replay {}", path.display()))?;
        let mut archive = ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        archive.start_file("metaData.json", options)?;
        archive.write_all(&serde_json::to_vec(&self.metadata(info))?)?;

        archive.start_file("recording.tmcpr", options)?;
        archive.write_all(&self.stream)?;

        archive
            .finish()
            .with_context(|| format!("finishing replay {}", path.display()))?;
        Ok(())
    }

    fn metadata(&self, info: &ReplayInfo) -> serde_json::Value {
        let duration = saturating_millis(self.duration());
        let since_epoch = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or_default();
        // Pinning is the contract: a clock past year 584_556_019 beats a
        // panic or a missing mandatory field.
        #[allow(clippy::fallible_int_fallback)]
        let date = u64::try_from(since_epoch).unwrap_or(u64::MAX);
        serde_json::json!({
            "singleplayer": false,
            "serverName": info.server_name,
            "duration": duration,
            "date": date,
            "mcversion": info.mc_version,
            "fileFormat": "MCPR",
            "fileFormatVersion": 14,
            "protocol": info.protocol_version,
            "generator": concat!("mc-bot ", env!("CARGO_PKG_VERSION")),
            // Entity id of the recording player; unknown without parsing
            // JoinGame, and ReplayMod treats -1 as unset.
            "selfId": -1,
            // Quick-access cache of visible player uuids; optional, and the
            // bot alone in a server sees none.
            "players": [],
        })
    }
}

/// A duration as whole milliseconds in the format's u32 fields, pinned to
/// `u32::MAX`.
// Clamping is the contract (see `Recorder::record`): a ~50-day recording has
// bigger problems than a pinned timestamp.
#[allow(clippy::fallible_int_fallback)]
fn saturating_millis(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use zip::ZipArchive;

    use super::*;

    fn read_entry(archive: &mut ZipArchive<File>, name: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        archive
            .by_name(name)
            .expect(name)
            .read_to_end(&mut bytes)
            .expect("read entry");
        bytes
    }

    #[test]
    fn writes_replay_archive() {
        let mut recorder = Recorder::new();
        recorder.record(&[0x02, 1, 2, 3]);
        recorder.record(&[0x03]);
        assert_eq!(recorder.packets(), 2);

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.mcpr");
        let info = ReplayInfo {
            server_name: "127.0.0.1:25565".to_owned(),
            mc_version: "26.2".to_owned(),
            protocol_version: 776,
        };
        recorder.write(&path, &info).expect("write");

        let mut archive = ZipArchive::new(File::open(&path).expect("open")).expect("zip");
        let meta: serde_json::Value =
            serde_json::from_slice(&read_entry(&mut archive, "metaData.json")).expect("json");
        assert_eq!(meta["fileFormat"], "MCPR");
        assert_eq!(meta["fileFormatVersion"], 14);
        assert_eq!(meta["protocol"], 776);
        assert_eq!(meta["mcversion"], "26.2");
        assert_eq!(meta["serverName"], "127.0.0.1:25565");

        let stream = read_entry(&mut archive, "recording.tmcpr");
        // Entry 1: timestamp 0 (first packet defines the epoch), length 4.
        assert_eq!(&stream[..4], &0u32.to_be_bytes());
        assert_eq!(&stream[4..8], &4u32.to_be_bytes());
        assert_eq!(&stream[8..12], &[0x02, 1, 2, 3]);
        // Entry 2 follows immediately; its timestamp is near-zero but only
        // its framing is asserted, not the clock.
        assert_eq!(&stream[16..20], &1u32.to_be_bytes());
        assert_eq!(&stream[20..], &[0x03]);
    }

    #[test]
    fn empty_recorder_reports_empty() {
        let recorder = Recorder::new();
        assert!(recorder.is_empty());
        assert_eq!(recorder.duration(), Duration::ZERO);
    }
}
