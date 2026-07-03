//! Jitter buffer between the audio socket thread (producer) and the `CoreAudio`
//! render callback (consumer).
//!
//! The guest ships PCM at its own real-time rate; the device consumes at the
//! host clock. This buffer absorbs transport jitter and the slow drift between
//! the two clocks with three rules, all chosen for game audio (latency over
//! continuity, see index#1686):
//!
//! - **Prime before playing**: output silence until `target` samples are
//!   buffered, so playback starts with a full jitter margin instead of
//!   crackling through a cold start.
//! - **Underrun -> silence and re-prime**: the callback NEVER blocks (a
//!   `CoreAudio` render callback that waits can glitch the whole output
//!   device); missing samples are zeroed and the buffer re-primes, trading
//!   one audible gap for a restored margin.
//! - **Overrun -> drop the OLDEST audio back to `target`**: if the producer
//!   runs ahead (device stall, guest clock fast), keeping everything would
//!   pin latency at the cap forever; dropping the oldest samples snaps
//!   latency back to `target` in one discontinuity and keeps the newest
//!   (most current) audio.
//!
//! A `Mutex<VecDeque>` rather than a lock-free ring on purpose: both sides
//! touch it for a bounded memcpy of a few KiB every 5-10 ms, so the callback
//! waits at worst a few microseconds -- far under a render quantum -- while a
//! hand-rolled atomic SPSC ring would be `unsafe` the house rules require
//! Miri/loom validation for, and no vendored crate provides one.

use std::collections::VecDeque;
use std::sync::Mutex;

/// What one [`JitterBuffer::pop_f32`] call observed; the caller may count or
/// log these, the buffer already handled them (silence written, state moved).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopOutcome {
    /// Still filling toward `target`; the whole output was silence.
    Priming,
    /// The output was fully satisfied from buffered samples.
    Playing,
    /// Buffered audio ran out mid-output: the tail was silence and the buffer
    /// went back to priming.
    Underrun,
}

/// Cumulative producer/consumer incidents, for end-of-connection logging.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub underruns: u64,
    /// Samples discarded by the overrun rule. `usize` because each increment
    /// is a buffer length; wrap-around would take centuries at audio rates.
    pub dropped_samples: usize,
}

struct Inner {
    queue: VecDeque<i16>,
    primed: bool,
    stats: Stats,
}

pub struct JitterBuffer {
    inner: Mutex<Inner>,
    /// Fill level (samples) at which playback starts and to which an overrun
    /// resyncs.
    target: usize,
    /// Fill cap (samples); a push that would exceed it triggers the overrun
    /// rule. Must be > `target`.
    max: usize,
}

impl JitterBuffer {
    /// # Panics
    /// If `target >= max` or `target == 0`: both would make the overrun and
    /// priming rules meaningless, and both are compile-time constants at the
    /// only call site.
    #[must_use]
    pub fn new(target: usize, max: usize) -> Self {
        assert!(target > 0 && target < max, "need 0 < target < max");
        Self {
            inner: Mutex::new(Inner {
                queue: VecDeque::with_capacity(max),
                primed: false,
                stats: Stats::default(),
            }),
            target,
            max,
        }
    }

    /// Producer side: append decoded samples, applying the overrun rule.
    pub fn push(&self, samples: &[i16]) {
        let mut inner = self.inner.lock().expect("audio jitter mutex poisoned");
        let total = inner.queue.len() + samples.len();
        if total > self.max {
            // Bring the post-push fill back to `target`: drop the oldest
            // buffered samples first, then (only if the incoming chunk alone
            // exceeds `target`) the oldest of the incoming chunk.
            let excess = total - self.target;
            let from_queue = excess.min(inner.queue.len());
            inner.queue.drain(..from_queue);
            // `excess - from_queue` <= samples.len() by construction:
            // when from_queue is the whole queue, excess - queue.len()
            // = samples.len() - target < samples.len().
            let from_new = excess - from_queue;
            inner.queue.extend(&samples[from_new..]);
            inner.stats.dropped_samples += excess;
        } else {
            inner.queue.extend(samples);
        }
    }

    /// Consumer side (the render callback): fill `out` with buffered samples
    /// converted to f32 (i16 full scale -> [-1.0, 1.0)), silence where the
    /// buffer cannot, per the priming/underrun rules. Never blocks beyond the
    /// short producer critical section.
    pub fn pop_f32(&self, out: &mut [f32]) -> PopOutcome {
        // Explicit `drop(inner)` on every exit path keeps the producer
        // unblocked while this thread zero-fills (significant_drop_tightening).
        let mut inner = self.inner.lock().expect("audio jitter mutex poisoned");
        if !inner.primed {
            if inner.queue.len() >= self.target {
                inner.primed = true;
            } else {
                drop(inner);
                out.fill(0.0);
                return PopOutcome::Priming;
            }
        }
        let n = inner.queue.len().min(out.len());
        for (slot, sample) in out[..n].iter_mut().zip(inner.queue.drain(..n)) {
            *slot = f32::from(sample) / 32768.0;
        }
        if n < out.len() {
            inner.primed = false;
            inner.stats.underruns += 1;
            drop(inner);
            out[n..].fill(0.0);
            return PopOutcome::Underrun;
        }
        drop(inner);
        PopOutcome::Playing
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        self.inner.lock().expect("audio jitter mutex poisoned").stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(start: i16, len: usize) -> Vec<i16> {
        (0..len).map(|i| start + i16::try_from(i).unwrap()).collect()
    }

    #[test]
    fn primes_with_silence_until_target() {
        let buf = JitterBuffer::new(4, 16);
        buf.push(&ramp(1, 3)); // below target
        let mut out = [1.0f32; 2];
        assert_eq!(buf.pop_f32(&mut out), PopOutcome::Priming);
        // Silence is bit-exact zero, so exact comparison is intended (Vec
        // form because float_cmp rejects strict `==` on float arrays).
        assert_eq!(out.to_vec(), vec![0.0f32; 2]);
        // The buffered samples were NOT consumed while priming.
        buf.push(&ramp(4, 1)); // reaches target
        let mut out = [0.0f32; 4];
        assert_eq!(buf.pop_f32(&mut out), PopOutcome::Playing);
        let expected: Vec<f32> = [1i16, 2, 3, 4].iter().map(|&s| f32::from(s) / 32768.0).collect();
        assert_eq!(out.to_vec(), expected);
    }

    #[test]
    fn underrun_fills_silence_and_reprimes() {
        let buf = JitterBuffer::new(2, 16);
        buf.push(&ramp(1, 3));
        let mut out = [9.0f32; 5];
        assert_eq!(buf.pop_f32(&mut out), PopOutcome::Underrun);
        assert_eq!(&out[3..], &[0.0, 0.0], "missing tail is silence");
        assert_eq!(buf.stats().underruns, 1);
        // Re-primed: the next pop is silence until target is reached again.
        buf.push(&ramp(1, 1));
        let mut out = [9.0f32; 1];
        assert_eq!(buf.pop_f32(&mut out), PopOutcome::Priming);
        assert_eq!(out.to_vec(), vec![0.0f32]);
    }

    #[test]
    fn overrun_drops_oldest_back_to_target() {
        let buf = JitterBuffer::new(4, 8);
        buf.push(&ramp(1, 8)); // exactly max: kept in full
        // 8 + 2 > max: drop the oldest 6 so the post-push fill is target (4),
        // leaving the newest samples [7, 8] ++ [9, 10].
        buf.push(&ramp(9, 2));
        assert_eq!(buf.stats().dropped_samples, 6);
        let mut out = [0.0f32; 4];
        assert_eq!(buf.pop_f32(&mut out), PopOutcome::Playing);
        let expected: Vec<f32> = [7i16, 8, 9, 10].iter().map(|&s| f32::from(s) / 32768.0).collect();
        assert_eq!(out.to_vec(), expected);
    }

    #[test]
    fn giant_push_keeps_only_newest_target_samples() {
        let buf = JitterBuffer::new(4, 8);
        buf.push(&ramp(1, 2));
        // 2 + 20 samples against max 8: everything but the newest 4 goes.
        buf.push(&ramp(100, 20));
        assert_eq!(buf.stats().dropped_samples, 18);
        let mut out = [0.0f32; 4];
        assert_eq!(buf.pop_f32(&mut out), PopOutcome::Playing);
        let expected: Vec<f32> =
            [116i16, 117, 118, 119].iter().map(|&s| f32::from(s) / 32768.0).collect();
        assert_eq!(out.to_vec(), expected);
    }
}
