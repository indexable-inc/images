//! Shared LAN timeline math for shared-audio.
//!
//! Peers agree on one timeline ("shared micros" since a session epoch) so a
//! note stamped for frame N fires simultaneously everywhere. The peer with
//! the smallest [`PeerId`] leads; every other peer estimates its offset to
//! the leader from NTP-style four-timestamp pings and converts local
//! monotonic time to shared frames through [`SharedClock`].
//!
//! Everything here is pure state: sockets and ping scheduling live in
//! `audio-net`, which feeds [`PingSample`]s into [`OffsetEstimator`].

use std::collections::VecDeque;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// The one sample rate of the shared timeline. Frame indices everywhere in
/// shared-audio count 48 kHz frames since the session epoch.
pub const SAMPLE_RATE: u32 = 48_000;

/// Identifies a peer. Doubles as the leader-election key: the smallest id
/// in the session leads the clock.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct PeerId(pub u64);

impl PeerId {
    /// A process-unique random id (no RNG dependency: seeds from the
    /// standard library's randomized hasher).
    #[must_use]
    pub fn random() -> Self {
        Self(RandomState::new().build_hasher().finish())
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// One NTP-style measurement: `request_sent` (t0) and `response_received`
/// (t3) are on the local clock; `peer_received` (t1) and `peer_replied`
/// (t2) are on the peer's clock. All monotonic micros.
#[derive(Clone, Copy, Debug)]
pub struct PingSample {
    /// t0: when the request left, local clock.
    pub request_sent: u64,
    /// t1: when the peer saw it, peer clock.
    pub peer_received: u64,
    /// t2: when the peer answered, peer clock.
    pub peer_replied: u64,
    /// t3: when the answer arrived, local clock.
    pub response_received: u64,
}

impl PingSample {
    /// Estimated `peer_clock - local_clock` in micros, assuming symmetric
    /// network delay: `((t1 - t0) + (t2 - t3)) / 2`.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "micros differences between live clocks fit i64 by construction"
    )]
    pub const fn offset_micros(&self) -> i64 {
        let outbound = self.peer_received as i128 - self.request_sent as i128;
        let inbound = self.peer_replied as i128 - self.response_received as i128;
        i128::midpoint(outbound, inbound) as i64
    }

    /// Round-trip time excluding the peer's processing gap.
    #[must_use]
    pub const fn rtt_micros(&self) -> u64 {
        let total = self.response_received.saturating_sub(self.request_sent);
        let remote = self.peer_replied.saturating_sub(self.peer_received);
        total.saturating_sub(remote)
    }
}

/// Sliding-window offset estimator.
///
/// Keeps the most recent samples, discards the high-RTT tail (a delayed
/// packet lies about the offset), and reports the median of what remains.
/// On a LAN this converges within a few pings to single-digit millisecond
/// accuracy, which is what the ear needs to hear "together".
#[derive(Debug, Clone)]
pub struct OffsetEstimator {
    samples: VecDeque<PingSample>,
    capacity: usize,
}

impl OffsetEstimator {
    /// An estimator remembering at most `capacity` recent samples.
    #[must_use]
    pub const fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::new(),
            capacity,
        }
    }

    /// Forget every sample, e.g. after a leader change: offsets measured
    /// against the old leader's clock say nothing about the new one.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Record one measurement.
    pub fn record(&mut self, sample: PingSample) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    /// Number of recorded samples currently in the window.
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether no samples have been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Median offset over the low-RTT half of the window, or `None` before
    /// the first sample.
    #[must_use]
    pub fn estimate(&self) -> Option<i64> {
        let min_rtt = self.samples.iter().map(PingSample::rtt_micros).min()?;
        // Samples much slower than the best round trip saw queueing on one
        // leg and would bias the offset; keep the crisp ones.
        let cutoff = min_rtt.saturating_mul(3) / 2 + 500;
        let mut offsets: Vec<i64> = self
            .samples
            .iter()
            .filter(|sample| sample.rtt_micros() <= cutoff)
            .map(PingSample::offset_micros)
            .collect();
        offsets.sort_unstable();
        offsets.get(offsets.len() / 2).copied()
    }
}

/// Maps local monotonic micros to the shared timeline.
///
/// `offset_micros` converts local micros to leader micros (zero while
/// leading); `epoch_micros` is the session origin in leader micros. Both
/// together give `shared = local + offset - epoch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedClock {
    offset_micros: i64,
    epoch_micros: i64,
}

impl SharedClock {
    /// Start a new session led by this peer: the epoch is "now".
    #[must_use]
    #[expect(
        clippy::cast_possible_wrap,
        reason = "monotonic micros since process start fit i64 for centuries"
    )]
    pub const fn lead(now_local_micros: u64) -> Self {
        Self {
            offset_micros: 0,
            epoch_micros: now_local_micros as i64,
        }
    }

    /// Follow a leader given the estimated offset to it and its advertised
    /// session epoch (in the leader's clock).
    #[must_use]
    pub const fn follow(offset_micros: i64, epoch_micros: i64) -> Self {
        Self {
            offset_micros,
            epoch_micros,
        }
    }

    /// Session epoch in leader micros, as advertised to joining peers.
    #[must_use]
    pub const fn epoch_micros(&self) -> i64 {
        self.epoch_micros
    }

    /// Session epoch translated onto *this* peer's local clock: what this
    /// peer should advertise to others, since they ping our local clock.
    /// While leading (zero offset) it equals [`Self::epoch_micros`].
    #[must_use]
    pub const fn local_epoch_micros(&self) -> i64 {
        self.epoch_micros - self.offset_micros
    }

    /// Micros elapsed on the shared timeline at local instant
    /// `local_micros`. Negative before the epoch.
    #[must_use]
    #[expect(
        clippy::cast_possible_wrap,
        reason = "monotonic micros since process start fit i64 for centuries"
    )]
    pub const fn shared_micros(&self, local_micros: u64) -> i64 {
        local_micros as i64 + self.offset_micros - self.epoch_micros
    }

    /// Absolute frame index on the shared timeline at local instant
    /// `local_micros`.
    #[must_use]
    pub fn frame_at(&self, local_micros: u64, sample_rate: u32) -> i64 {
        micros_to_frames(self.shared_micros(local_micros), sample_rate)
    }

    /// Local monotonic micros at which shared `frame` plays.
    #[must_use]
    pub fn local_micros_of_frame(&self, frame: i64, sample_rate: u32) -> i64 {
        frames_to_micros(frame, sample_rate) + self.epoch_micros - self.offset_micros
    }

    /// Take over leadership without a timeline jump: the new leader picks
    /// the epoch that keeps "shared now" continuous.
    #[must_use]
    pub const fn adopt_lead(&self, now_local_micros: u64) -> Self {
        let shared_now = self.shared_micros(now_local_micros);
        #[expect(
            clippy::cast_possible_wrap,
            reason = "monotonic micros since process start fit i64 for centuries"
        )]
        Self {
            offset_micros: 0,
            epoch_micros: now_local_micros as i64 - shared_now,
        }
    }
}

/// Convert shared micros to a frame index (floor division, so pre-epoch
/// instants map to negative frames without rounding toward zero).
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "frame counts on a live timeline fit i64"
)]
pub fn micros_to_frames(shared_micros: i64, sample_rate: u32) -> i64 {
    let scaled = i128::from(shared_micros) * i128::from(sample_rate);
    scaled.div_euclid(1_000_000) as i64
}

/// Convert a frame index back to shared micros (floor division).
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "micros on a live timeline fit i64"
)]
pub fn frames_to_micros(frame: i64, sample_rate: u32) -> i64 {
    let scaled = i128::from(frame) * 1_000_000;
    scaled.div_euclid(i128::from(sample_rate)) as i64
}

/// Monotonic time source, injectable so tests can drive the clock by hand.
pub trait MonotonicTime: Send + Sync {
    /// Micros since an arbitrary fixed origin, never decreasing.
    fn now_micros(&self) -> u64;
}

/// Real time source: micros since the process created it.
#[derive(Debug, Clone)]
pub struct ProcessTime {
    origin: Instant,
}

impl Default for ProcessTime {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl MonotonicTime for ProcessTime {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "process uptime micros fit u64 for half a million years"
    )]
    fn now_micros(&self) -> u64 {
        self.origin.elapsed().as_micros() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn sample(t0: u64, t1: u64, t2: u64, t3: u64) -> PingSample {
        PingSample {
            request_sent: t0,
            peer_received: t1,
            peer_replied: t2,
            response_received: t3,
        }
    }

    #[test]
    fn symmetric_delay_recovers_exact_offset() {
        // Peer clock runs 5000 us ahead; both legs take 1000 us.
        let ping = sample(10_000, 16_000, 16_100, 12_100);
        assert_eq!(ping.offset_micros(), 5_000);
        assert_eq!(ping.rtt_micros(), 2_000);
    }

    #[test]
    fn estimator_discards_delayed_outliers() {
        let mut estimator = OffsetEstimator::new(8);
        for i in 0..6 {
            let t0 = i * 100_000;
            estimator.record(sample(t0, t0 + 6_000, t0 + 6_050, t0 + 2_050));
        }
        // One sample whose return leg queued for 60 ms: alone it would
        // claim an offset ~30 ms off the truth.
        estimator.record(sample(700_000, 706_000, 706_050, 762_050));
        assert_eq!(estimator.estimate(), Some(5_000));
    }

    #[test]
    fn estimator_empty_and_window() {
        let mut estimator = OffsetEstimator::new(2);
        assert!(estimator.is_empty());
        assert_eq!(estimator.estimate(), None);
        estimator.record(sample(0, 1_000, 1_000, 2_000));
        estimator.record(sample(10, 1_010, 1_010, 2_010));
        estimator.record(sample(20, 1_020, 1_020, 2_020));
        assert_eq!(estimator.len(), 2);
    }

    #[test]
    fn clock_maps_micros_to_frames() {
        let clock = SharedClock::lead(1_000_000);
        assert_eq!(clock.frame_at(1_000_000, SAMPLE_RATE), 0);
        // One second after the epoch = 48000 frames.
        assert_eq!(clock.frame_at(2_000_000, SAMPLE_RATE), 48_000);
        // Half a millisecond = 24 frames.
        assert_eq!(clock.frame_at(1_000_500, SAMPLE_RATE), 24);
    }

    #[test]
    fn follower_agrees_with_leader() {
        // Leader's local clock reads 1_000_000 at the epoch. Follower's
        // clock runs 250_000 us behind the leader's.
        let leader = SharedClock::lead(1_000_000);
        let follower = SharedClock::follow(250_000, leader.epoch_micros());
        // At the same physical instant (leader 3_000_000, follower
        // 2_750_000) both report the same shared frame.
        assert_eq!(
            leader.frame_at(3_000_000, SAMPLE_RATE),
            follower.frame_at(2_750_000, SAMPLE_RATE)
        );
    }

    #[test]
    fn chained_follower_agrees_via_local_epoch() {
        // A leads; B follows A; C only sees B and follows B's *local*
        // epoch, pinging B's local clock.
        let a = SharedClock::lead(1_000_000);
        // A's clock runs 250_000 us ahead of B's.
        let b = SharedClock::follow(250_000, a.epoch_micros());
        // B's clock runs 100_000 us ahead of C's.
        let c = SharedClock::follow(100_000, b.local_epoch_micros());
        // Same physical instant on all three clocks.
        assert_eq!(
            a.frame_at(3_000_000, SAMPLE_RATE),
            b.frame_at(2_750_000, SAMPLE_RATE)
        );
        assert_eq!(
            a.frame_at(3_000_000, SAMPLE_RATE),
            c.frame_at(2_650_000, SAMPLE_RATE)
        );
    }

    #[test]
    fn frame_to_micros_roundtrip() {
        let clock = SharedClock::follow(-3_000, 500_000);
        let local = clock.local_micros_of_frame(96_000, SAMPLE_RATE);
        assert_eq!(local, 2_000_000 + 500_000 + 3_000);
    }

    #[test]
    fn adopt_lead_keeps_timeline_continuous() {
        let follower = SharedClock::follow(250_000, 1_000_000);
        let now_local = 5_000_000;
        let before = follower.shared_micros(now_local);
        let adopted = follower.adopt_lead(now_local);
        assert_eq!(adopted.shared_micros(now_local), before);
        // And it now leads: zero offset.
        assert_eq!(adopted, SharedClock::follow(0, adopted.epoch_micros()));
    }

    #[test]
    fn negative_shared_time_floors() {
        assert_eq!(micros_to_frames(-1, SAMPLE_RATE), -1);
        assert_eq!(frames_to_micros(-1, SAMPLE_RATE), -21);
    }
}
