//! The shared score: what the ensemble plays, as a Loro CRDT document.
//!
//! The score holds only *shared* musical state: which instrument module to
//! run (referenced by content hash, bytes travel out of band), the current
//! control values, and schedule-ahead events stamped with shared-timeline
//! frame numbers. State local to one listener (volume, output device) never
//! enters the document, so per-listener locality holds by construction.
//!
//! The CRDT answers *what* the ensemble plays; the shared clock in
//! `audio-clock` answers *when*. Peers gossip compact update deltas
//! (`export_updates` / `import_updates`) and converge to the same score, so
//! every peer renders identical audio from it.

use std::collections::BTreeMap;

use anyhow::{Context as _, Result, anyhow};
use audio_blob::BlobHash;
use loro::{ExportMode, LoroDoc, LoroMap, LoroValue};
use serde::{Deserialize, Serialize};
pub use loro::VersionVector;

/// Root map holding session-wide settings (currently the sample rate).
const SESSION: &str = "session";
/// Root map holding the active instrument publication.
const INSTRUMENT: &str = "instrument";
/// Root list of schedule-ahead [`Event`]s.
const EVENTS: &str = "events";

/// The active instrument module and the shared-timeline frame it takes
/// effect at, as stored in the score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstrumentRef {
    /// Content hash of the WASM module; fetch the bytes from a blob store.
    pub hash: BlobHash,
    /// Shared-timeline frame at which peers switch to this module.
    pub at_frame: u64,
}

#[derive(Deserialize, Serialize)]
struct StoredInstrument {
    hash: String,
    at_frame: i64,
}

/// A scheduled control change.
///
/// At shared-timeline frame `at_frame`, control `control` becomes `value`.
/// Events are appended concurrently by any peer; [`Score::events`] returns
/// them in one deterministic order everywhere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    /// Shared-timeline frame the change applies at.
    pub at_frame: u64,
    /// Instrument control index.
    pub control: u16,
    /// New control value.
    pub value: f32,
}

/// [`Event::sort_key`] result.
///
/// Field order is the derived comparison order: frame, then control, then
/// value bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventKey {
    frame: u64,
    control: u16,
    value_bits: u32,
}

impl Event {
    /// Total order used everywhere so concurrent schedules resolve
    /// identically on every peer.
    #[must_use]
    pub const fn sort_key(&self) -> EventKey {
        EventKey { frame: self.at_frame, control: self.control, value_bits: self.value.to_bits() }
    }
}

/// A control's value at a requested shared-timeline frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlValue {
    /// Instrument control index.
    pub control: u16,
    /// Value in effect at the requested frame.
    pub value: f32,
}

/// The shared score document. Cheap to create; wrap in `Arc<Mutex<_>>` (or
/// the async equivalent) to share between a network node and a renderer.
#[derive(Debug)]
pub struct Score {
    doc: LoroDoc,
}

impl Default for Score {
    fn default() -> Self {
        Self::new()
    }
}

impl Score {
    /// An empty score.
    #[must_use]
    pub fn new() -> Self {
        Self { doc: LoroDoc::new() }
    }

    /// Record the session sample rate.
    ///
    /// # Errors
    /// Fails only on a Loro document error.
    pub fn set_sample_rate(&self, sample_rate: u32) -> Result<()> {
        self.doc
            .get_map(SESSION)
            .insert("sample_rate", i64::from(sample_rate))?;
        self.doc.commit();
        Ok(())
    }

    /// The session sample rate, if one has been recorded.
    #[must_use]
    pub fn sample_rate(&self) -> Option<u32> {
        let value = self.doc.get_map(SESSION).get("sample_rate")?;
        u32::try_from(as_i64(&value.get_deep_value())?).ok()
    }

    /// Point the score at an instrument module, switching at `at_frame`.
    ///
    /// # Errors
    /// Fails on a Loro document error or a frame beyond `i64::MAX`.
    pub fn set_instrument(&self, hash: &BlobHash, at_frame: u64) -> Result<()> {
        let instrument = StoredInstrument {
            hash: hash.to_string(),
            at_frame: frame_to_i64(at_frame)?,
        };
        let instrument = serde_json::to_string(&instrument)?;
        let map = self.doc.get_map(INSTRUMENT);
        map.insert("publication", instrument.as_str())?;
        self.doc.commit();
        Ok(())
    }

    /// The active instrument reference, if one has been published.
    ///
    /// # Errors
    /// Fails when the stored publication is malformed.
    pub fn instrument(&self) -> Result<Option<InstrumentRef>> {
        let map = self.doc.get_map(INSTRUMENT);
        let Some(instrument) = map.get("publication") else {
            return Ok(None);
        };
        let instrument = instrument.get_deep_value();
        let instrument = as_str(&instrument).context("instrument publication is not a string")?;
        let instrument: StoredInstrument = serde_json::from_str(instrument)?;
        let hash = BlobHash::parse_hex(&instrument.hash)?;
        let at_frame = u64::try_from(instrument.at_frame)
            .context("instrument frame must be nonnegative")?;
        Ok(Some(InstrumentRef { hash, at_frame }))
    }

    /// Set a control to a value at the current shared-timeline frame.
    ///
    /// Immediate and scheduled changes share one event log so renderers can
    /// reconstruct control state for any historical range.
    ///
    /// # Errors
    /// Fails on a Loro document error or a frame beyond `i64::MAX`.
    pub fn set_control(&self, control: u16, value: f32, at_frame: u64) -> Result<()> {
        self.schedule(Event { at_frame, control, value })
    }

    /// Control values in effect at `at_frame`, sorted by control index.
    #[must_use]
    pub fn controls_at(&self, at_frame: u64) -> Vec<ControlValue> {
        let mut controls = BTreeMap::new();
        for event in self.events().into_iter().take_while(|event| event.at_frame <= at_frame) {
            controls.insert(event.control, event.value);
        }
        controls
            .into_iter()
            .map(|(control, value)| ControlValue { control, value })
            .collect()
    }

    /// Append a schedule-ahead event.
    ///
    /// # Errors
    /// Fails on a Loro document error or a frame beyond `i64::MAX`.
    pub fn schedule(&self, event: Event) -> Result<()> {
        let list = self.doc.get_list(EVENTS);
        let map = list.insert_container(list.len(), LoroMap::new())?;
        map.insert("at", frame_to_i64(event.at_frame)?)?;
        map.insert("control", i64::from(event.control))?;
        map.insert("value", f64::from(event.value))?;
        self.doc.commit();
        Ok(())
    }

    /// All scheduled events in the deterministic [`Event::sort_key`] order,
    /// identical on every converged peer.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        let value = self.doc.get_list(EVENTS).get_deep_value();
        let LoroValue::List(items) = value else {
            return Vec::new();
        };
        let mut events: Vec<Event> = items.iter().filter_map(event_of).collect();
        events.sort_unstable_by_key(Event::sort_key);
        events
    }

    /// The document version, for delta export bookkeeping.
    #[must_use]
    pub fn version(&self) -> VersionVector {
        self.doc.oplog_vv()
    }

    /// Export the operations another peer at `since` is missing.
    ///
    /// # Errors
    /// Fails only on a Loro encode error.
    pub fn export_updates(&self, since: &VersionVector) -> Result<Vec<u8>> {
        Ok(self.doc.export(ExportMode::updates(since))?)
    }

    /// Export a full snapshot, for a newly joined peer.
    ///
    /// # Errors
    /// Fails only on a Loro encode error.
    pub fn export_snapshot(&self) -> Result<Vec<u8>> {
        Ok(self.doc.export(ExportMode::Snapshot)?)
    }

    /// Import a snapshot or update delta from another peer.
    ///
    /// # Errors
    /// Fails when the bytes are not a valid Loro export.
    pub fn import(&self, bytes: &[u8]) -> Result<()> {
        self.doc.import(bytes)?;
        Ok(())
    }
}

/// Frames travel as `i64` inside the document (Loro's integer scalar).
fn frame_to_i64(frame: u64) -> Result<i64> {
    i64::try_from(frame).map_err(|_| anyhow!("frame {frame} exceeds i64::MAX"))
}

const fn as_i64(value: &LoroValue) -> Option<i64> {
    if let LoroValue::I64(value) = value {
        Some(*value)
    } else {
        None
    }
}

fn as_str(value: &LoroValue) -> Option<&str> {
    if let LoroValue::String(value) = value {
        Some(value)
    } else {
        None
    }
}

/// Decode one event map; `None` skips malformed entries rather than letting
/// one bad peer wedge every renderer.
fn event_of(value: &LoroValue) -> Option<Event> {
    let LoroValue::Map(map) = value else {
        return None;
    };
    let at_frame = u64::try_from(as_i64(map.get("at")?)?).ok()?;
    let control = u16::try_from(as_i64(map.get("control")?)?).ok()?;
    let LoroValue::Double(value) = map.get("value")? else {
        return None;
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "controls are f32 at the instrument ABI; narrowing is the contract"
    )]
    let value = *value as f32;
    Some(Event { at_frame, control, value })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash() -> BlobHash {
        BlobHash::of(b"module bytes")
    }

    #[test]
    fn roundtrips_instrument_controls_and_events() -> Result<()> {
        let score = Score::new();
        score.set_sample_rate(48_000)?;
        score.set_instrument(&hash(), 96_000)?;
        score.set_control(0, 440.0, 0)?;
        score.set_control(1, 0.5, 0)?;
        score.schedule(Event { at_frame: 48_000, control: 0, value: 660.0 })?;

        assert_eq!(score.sample_rate(), Some(48_000));
        let instrument = score.instrument()?.expect("instrument set");
        assert_eq!(instrument.hash, hash());
        assert_eq!(instrument.at_frame, 96_000);
        assert_eq!(
            score.events(),
            vec![
                Event { at_frame: 0, control: 0, value: 440.0 },
                Event { at_frame: 0, control: 1, value: 0.5 },
                Event { at_frame: 48_000, control: 0, value: 660.0 }
            ]
        );
        assert_eq!(
            score.controls_at(0),
            vec![
                ControlValue { control: 0, value: 440.0 },
                ControlValue { control: 1, value: 0.5 }
            ]
        );
        Ok(())
    }

    #[test]
    fn concurrent_edits_converge() -> Result<()> {
        let a = Score::new();
        let b = Score::new();
        a.set_control(0, 1.0, 0)?;
        b.schedule(Event { at_frame: 10, control: 2, value: 0.25 })?;
        b.schedule(Event { at_frame: 5, control: 1, value: 0.75 })?;

        b.import(&a.export_updates(&VersionVector::new())?)?;
        a.import(&b.export_updates(&VersionVector::new())?)?;

        assert_eq!(a.events(), b.events());
        // Deterministic order: sorted by frame regardless of insert order.
        assert_eq!(
            a.events().iter().map(|event| event.at_frame).collect::<Vec<_>>(),
            vec![0, 5, 10]
        );
        Ok(())
    }

    #[test]
    fn concurrent_instrument_publications_remain_atomic() -> Result<()> {
        let a = Score::new();
        let b = Score::new();
        let a_ref = InstrumentRef { hash: BlobHash::of(b"module a"), at_frame: 1_000 };
        let b_ref = InstrumentRef { hash: BlobHash::of(b"module b"), at_frame: 2_000 };
        a.set_instrument(&a_ref.hash, a_ref.at_frame)?;
        b.set_instrument(&b_ref.hash, b_ref.at_frame)?;
        let a_update = a.export_updates(&VersionVector::new())?;
        let b_update = b.export_updates(&VersionVector::new())?;

        a.import(&b_update)?;
        b.import(&a_update)?;

        let converged = a.instrument()?.expect("instrument set");
        assert_eq!(b.instrument()?, Some(converged));
        assert!(converged == a_ref || converged == b_ref);
        Ok(())
    }

    #[test]
    fn rejected_instrument_frame_does_not_edit_the_score() -> Result<()> {
        let score = Score::new();
        let original = hash();
        score.set_instrument(&original, 96_000)?;
        let version = score.version();

        let rejected = BlobHash::of(b"rejected module");
        assert!(score.set_instrument(&rejected, i64::MAX as u64 + 1).is_err());
        assert_eq!(score.version(), version);
        assert_eq!(
            score.instrument()?,
            Some(InstrumentRef { hash: original, at_frame: 96_000 })
        );
        Ok(())
    }

    #[test]
    fn immediate_controls_follow_older_scheduled_events() -> Result<()> {
        let score = Score::new();
        score.schedule(Event { at_frame: 100, control: 1, value: 0.25 })?;
        score.set_control(1, 0.75, 200)?;

        assert_eq!(
            score.events(),
            vec![
                Event { at_frame: 100, control: 1, value: 0.25 },
                Event { at_frame: 200, control: 1, value: 0.75 }
            ]
        );
        assert_eq!(score.controls_at(150), vec![ControlValue { control: 1, value: 0.25 }]);
        assert_eq!(score.controls_at(200), vec![ControlValue { control: 1, value: 0.75 }]);
        Ok(())
    }

    #[test]
    fn delta_export_carries_only_new_operations() -> Result<()> {
        let a = Score::new();
        let b = Score::new();
        a.set_control(0, 1.0, 0)?;
        b.import(&a.export_updates(&b.version())?)?;
        let synced = b.version();

        a.set_control(1, 2.0, 1)?;
        let delta = a.export_updates(&synced)?;
        b.import(&delta)?;
        assert_eq!(a.events(), b.events());
        Ok(())
    }

    #[test]
    fn volume_never_enters_the_score() {
        // Locality by construction: the API simply has no volume surface.
        // Guard the document keys so a future field does not sneak one in.
        let score = Score::new();
        score.set_control(0, 1.0, 0).expect("set control");
        let json = format!("{:?}", score.doc.get_deep_value());
        assert!(!json.to_lowercase().contains("volume"));
    }
}
