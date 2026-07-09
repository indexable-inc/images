//! Wire pacing, the pure core: frame callbacks fire at SEND, host acks are
//! backpressure only. Kept smithay-free (like [`crate::frame`]) so the
//! arithmetic that decides whether a client may draw is unit-tested on any
//! development host; the event-loop plumbing lives in `compositor`.

/// Backpressure cap on unacked wire frames per window. Frame callbacks fire
/// at SEND, not at ack: a callback held to the ack quantizes the client to
/// `display_hz / n` whenever the host's per-frame turnaround exceeds one
/// display tick (the index#1686 60-acks cap on a 120Hz display). This cap is
/// what still bounds the client when the host stalls: at the cap, sends (and
/// the callbacks fired with them) wait for an ack to free a slot.
pub const MAX_INFLIGHT_FRAMES: u64 = 2;

/// May another frame go onto the wire? Frames in `acked+1..=sent` are in
/// flight; `acked <= sent` is an invariant ([`apply_ack`] clamps).
pub const fn under_inflight_cap(sent: u64, acked: u64) -> bool {
    sent - acked < MAX_INFLIGHT_FRAMES
}

/// Apply a cumulative ack (the host coalesces per display tick and acks only
/// the newest presented seq, so one ack may cover several in-flight frames):
/// the new acked watermark, or `None` for a stale ack. Clamped to `sent` so
/// a buggy host acking a seq it never received cannot widen the in-flight
/// window past [`MAX_INFLIGHT_FRAMES`].
pub const fn apply_ack(sent: u64, acked: u64, ack_seq: u64) -> Option<u64> {
    if ack_seq <= acked {
        return None;
    }
    Some(if ack_seq < sent { ack_seq } else { sent })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_gates_sends() {
        // (sent, acked, may send another frame)
        let table = [
            (0, 0, true),  // nothing in flight
            (1, 0, true),  // one slot free
            (2, 0, false), // at the cap: callbacks wait for an ack
            (3, 1, false), // still at the cap after a partial ack
            (3, 2, true),
            (3, 3, true), // fully drained
        ];
        for (sent, acked, open) in table {
            assert_eq!(under_inflight_cap(sent, acked), open, "sent={sent} acked={acked}");
        }
    }

    #[test]
    fn acks_are_cumulative_stale_ignored_future_clamped() {
        // (sent, acked, ack_seq, new watermark)
        let table = [
            (2, 0, 1, Some(1)), // partial ack frees one slot
            (2, 0, 2, Some(2)), // coalesced ack drains both
            (2, 1, 1, None),    // stale: already covered
            (2, 2, 1, None),    // stale after a watchdog release
            (2, 0, 5, Some(2)), // buggy host acking the future: clamped
        ];
        for (sent, acked, ack_seq, expect) in table {
            assert_eq!(
                apply_ack(sent, acked, ack_seq),
                expect,
                "sent={sent} acked={acked} ack_seq={ack_seq}"
            );
        }
    }
}
