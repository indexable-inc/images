use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use panes_protocol::{AxisSource, ToGuest, WindowId};

/// Outgoing input queue bound. Coalesced acks, absolute motion, relative
/// motion, and scroll deltas occupy one slot per post-barrier segment, so they
/// cannot fill the FIFO by themselves. The bound limits discrete input and
/// configure/close bursts when the peer stops draining; reconnecting is safer
/// than letting those messages grow unbounded and arrive seconds late.
pub const SEND_QUEUE_BOUND: usize = 512;

pub struct SendQueue {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

struct State {
    entries: VecDeque<Entry>,
    open: bool,
    on_close: Option<Box<dyn FnOnce() + Send>>,
}

struct Entry {
    key: Option<CoalesceKey>,
    msg: ToGuest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoalesceKey {
    PointerMotion { id: WindowId },
    PointerRelative { id: WindowId },
    PointerAxis { id: WindowId, source: AxisSource },
    Ack { id: WindowId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendQueueError {
    Full,
    Disconnected,
}

pub struct SendQueueReceiver {
    shared: Arc<Shared>,
}

pub fn channel() -> (SendQueue, SendQueueReceiver) {
    channel_with_on_close(|| {})
}

pub fn channel_with_on_close(on_close: impl FnOnce() + Send + 'static) -> (SendQueue, SendQueueReceiver) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            entries: VecDeque::new(),
            open: true,
            on_close: Some(Box::new(on_close)),
        }),
        ready: Condvar::new(),
    });
    (SendQueue { shared: Arc::clone(&shared) }, SendQueueReceiver { shared })
}

impl SendQueue {
    pub fn send(&self, msg: ToGuest) -> Result<(), SendQueueError> {
        let key = CoalesceKey::for_msg(&msg);
        let mut state = self.shared.state.lock().map_err(|_| SendQueueError::Disconnected)?;
        if !state.open {
            return Err(SendQueueError::Disconnected);
        }

        // Coalesce only within the segment after the last discrete
        // (non-coalescable) entry: a motion queued after a click must not
        // rewrite a slot the guest reads the click position from, or the click
        // lands at coordinates the user reached later. Scanning from the back
        // stops at the first barrier, which also keeps the scan short in
        // motion-dominated bursts.
        if let Some(key) = key {
            for entry in state.entries.iter_mut().rev() {
                if entry.key.is_none() {
                    break;
                }
                if entry.key == Some(key) {
                    coalesce(&mut entry.msg, msg);
                    return Ok(());
                }
            }
        }

        if state.entries.len() == SEND_QUEUE_BOUND {
            let on_close = close_locked(&mut state);
            self.shared.ready.notify_one();
            drop(state);
            run_on_close(on_close);
            return Err(SendQueueError::Full);
        }

        state.entries.push_back(Entry { key, msg });
        self.shared.ready.notify_one();
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.shared.state.lock().is_ok_and(|state| state.open)
    }
}

impl Drop for SendQueue {
    fn drop(&mut self) {
        let on_close = self.shared.state.lock().ok().and_then(|mut state| close_locked(&mut state));
        self.shared.ready.notify_all();
        run_on_close(on_close);
    }
}

impl SendQueueReceiver {
    pub fn recv(&self) -> Option<ToGuest> {
        let mut state = self.shared.state.lock().ok()?;
        loop {
            if let Some(entry) = state.entries.pop_front() {
                return Some(entry.msg);
            }
            if !state.open {
                return None;
            }
            state = self.shared.ready.wait(state).ok()?;
        }
    }

    pub fn try_recv(&self) -> Option<ToGuest> {
        let mut state = self.shared.state.lock().ok()?;
        state.entries.pop_front().map(|entry| entry.msg)
    }
}

impl Drop for SendQueueReceiver {
    fn drop(&mut self) {
        let on_close = self.shared.state.lock().ok().and_then(|mut state| close_locked(&mut state));
        self.shared.ready.notify_all();
        run_on_close(on_close);
    }
}

impl CoalesceKey {
    fn for_msg(msg: &ToGuest) -> Option<Self> {
        match msg {
            ToGuest::PointerMotion { id, .. } => Some(Self::PointerMotion { id: *id }),
            ToGuest::PointerRelative { id, .. } => Some(Self::PointerRelative { id: *id }),
            // A stop terminates a scroll segment in the compositor, so it is a
            // discrete barrier: later deltas must not merge into or replace it.
            ToGuest::PointerAxis { id, source, stop: false, .. } => {
                Some(Self::PointerAxis { id: *id, source: *source })
            }
            ToGuest::Ack { id, .. } => Some(Self::Ack { id: *id }),
            ToGuest::PointerAxis { stop: true, .. }
            | ToGuest::Hello { .. }
            | ToGuest::Configure { .. }
            | ToGuest::CloseRequest { .. }
            | ToGuest::PointerButton { .. }
            | ToGuest::PointerLeave { .. }
            | ToGuest::Key { .. }
            | ToGuest::Ping { .. }
            | ToGuest::KeyRepeat { .. } => None,
        }
    }
}

fn close_locked(state: &mut State) -> Option<Box<dyn FnOnce() + Send>> {
    state.open = false;
    state.on_close.take()
}

fn run_on_close(on_close: Option<Box<dyn FnOnce() + Send>>) {
    if let Some(on_close) = on_close {
        on_close();
    }
}

fn coalesce(queued: &mut ToGuest, next: ToGuest) {
    match (queued, next) {
        (ToGuest::PointerRelative { dx, dy, .. }, ToGuest::PointerRelative { dx: next_dx, dy: next_dy, .. }) => {
            *dx += next_dx;
            *dy += next_dy;
        }
        (
            ToGuest::PointerAxis { horizontal, vertical, v120, .. },
            ToGuest::PointerAxis {
                horizontal: next_horizontal,
                vertical: next_vertical,
                v120: next_v120,
                ..
            },
        ) => {
            *horizontal += next_horizontal;
            *vertical += next_vertical;
            // The compositor emits value120 only for `Some`; `None` carries no
            // wheel-step information to add, so preserving the present side is
            // the lossless merge. When both sides have wheel steps, sum each
            // axis component.
            *v120 = match (*v120, next_v120) {
                (Some((h, v)), Some((next_h, next_v))) => Some((h + next_h, v + next_v)),
                (Some(value), None) | (None, Some(value)) => Some(value),
                (None, None) => None,
            };
        }
        (ToGuest::Ack { seq, .. }, ToGuest::Ack { seq: next_seq, .. }) => {
            // Ack seqs are cumulative. Keep the larger seq here instead of
            // relying on today's per-window enqueue monotonicity in window.rs.
            if next_seq >= *seq {
                *seq = next_seq;
            }
        }
        (queued, next) => *queued = next,
    }
}

#[cfg(test)]
mod tests {
    use panes_protocol::ButtonState;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    fn motion(id: WindowId, x: f64) -> ToGuest {
        ToGuest::PointerMotion { id, x, y: x + 1.0 }
    }

    fn click(id: WindowId, button: u32) -> ToGuest {
        ToGuest::PointerButton { id, button, state: ButtonState::Pressed }
    }

    fn key(id: WindowId, keycode: u32) -> ToGuest {
        ToGuest::Key { id, keycode, state: ButtonState::Pressed }
    }

    fn relative(id: WindowId, dx: f64, dy: f64) -> ToGuest {
        ToGuest::PointerRelative { id, dx, dy }
    }

    fn axis(
        id: WindowId,
        source: AxisSource,
        horizontal: f64,
        vertical: f64,
        v120: Option<(i32, i32)>,
        stop: bool,
    ) -> ToGuest {
        ToGuest::PointerAxis { id, source, horizontal, vertical, v120, stop }
    }

    #[test]
    fn motion_coalesces_to_latest_while_length_stays_bounded() {
        let (queue, rx) = channel();
        for x in 0..1000 {
            assert_eq!(queue.send(motion(7, f64::from(x))), Ok(()));
        }

        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerMotion { id: 7, x, y }) if x == 999.0 && y == 1000.0
        ));
        assert!(rx.try_recv().is_none());
    }

    #[test]
    fn motion_keys_are_per_window_and_per_axis_source() {
        let (queue, rx) = channel();
        assert_eq!(queue.send(motion(1, 1.0)), Ok(()));
        assert_eq!(queue.send(motion(2, 2.0)), Ok(()));
        assert_eq!(
            queue.send(ToGuest::PointerAxis {
                id: 1,
                source: AxisSource::Wheel,
                horizontal: 1.0,
                vertical: 0.0,
                v120: Some((120, 0)),
                stop: false,
            }),
            Ok(())
        );
        assert_eq!(
            queue.send(ToGuest::PointerAxis {
                id: 1,
                source: AxisSource::Finger,
                horizontal: 2.0,
                vertical: 0.0,
                v120: None,
                stop: false,
            }),
            Ok(())
        );
        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerMotion { id: 1, x, .. }) if x == 1.0
        ));
        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerMotion { id: 2, x, .. }) if x == 2.0
        ));
        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerAxis { id: 1, source: AxisSource::Wheel, horizontal, .. })
                if horizontal == 1.0
        ));
        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerAxis { id: 1, source: AxisSource::Finger, horizontal, .. })
                if horizontal == 2.0
        ));
        assert!(rx.try_recv().is_none());
    }

    #[test]
    fn relative_deltas_sum_while_length_stays_bounded() {
        let (queue, rx) = channel();
        assert_eq!(queue.send(relative(7, 1.5, -2.0)), Ok(()));
        assert_eq!(queue.send(relative(7, 2.0, 3.5)), Ok(()));

        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerRelative { id: 7, dx, dy }) if dx == 3.5 && dy == 1.5
        ));
        assert!(rx.try_recv().is_none());
    }

    #[test]
    fn axis_deltas_sum_and_v120_merges_losslessly() {
        let (queue, rx) = channel();
        assert_eq!(queue.send(axis(7, AxisSource::Wheel, 1.0, 2.0, Some((120, 0)), false)), Ok(()));
        assert_eq!(queue.send(axis(7, AxisSource::Wheel, 3.0, 4.0, None, false)), Ok(()));
        assert_eq!(queue.send(axis(7, AxisSource::Wheel, 5.0, 6.0, Some((0, -120)), false)), Ok(()));

        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerAxis {
                id: 7,
                source: AxisSource::Wheel,
                horizontal,
                vertical,
                v120: Some((120, -120)),
                stop: false,
            }) if horizontal == 9.0 && vertical == 12.0
        ));
        assert!(rx.try_recv().is_none());
    }

    #[test]
    fn axis_stop_is_a_discrete_barrier() {
        let (queue, rx) = channel();
        assert_eq!(queue.send(axis(7, AxisSource::Finger, 1.0, 2.0, None, false)), Ok(()));
        assert_eq!(queue.send(axis(7, AxisSource::Finger, 0.0, 0.0, None, true)), Ok(()));
        assert_eq!(queue.send(axis(7, AxisSource::Finger, 3.0, 4.0, None, false)), Ok(()));
        assert_eq!(queue.send(axis(7, AxisSource::Finger, 5.0, 6.0, None, false)), Ok(()));

        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerAxis { horizontal, vertical, stop: false, .. })
                if horizontal == 1.0 && vertical == 2.0
        ));
        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerAxis { horizontal, vertical, stop: true, .. })
                if horizontal == 0.0 && vertical == 0.0
        ));
        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerAxis { horizontal, vertical, stop: false, .. })
                if horizontal == 8.0 && vertical == 10.0
        ));
        assert!(rx.try_recv().is_none());
    }

    #[test]
    fn ack_coalescing_keeps_highest_seq() {
        let (queue, rx) = channel();
        assert_eq!(queue.send(ToGuest::Ack { id: 7, seq: 10 }), Ok(()));
        assert_eq!(queue.send(ToGuest::Ack { id: 7, seq: 8 }), Ok(()));
        assert_eq!(queue.send(ToGuest::Ack { id: 7, seq: 12 }), Ok(()));

        assert!(matches!(rx.try_recv(), Some(ToGuest::Ack { id: 7, seq: 12 })));
        assert!(rx.try_recv().is_none());
    }

    #[test]
    fn discrete_events_preserve_order_and_are_not_dropped() {
        let (queue, rx) = channel();
        assert_eq!(queue.send(key(1, 30)), Ok(()));
        assert_eq!(queue.send(click(1, 0x110)), Ok(()));
        assert_eq!(queue.send(ToGuest::CloseRequest { id: 1 }), Ok(()));

        assert!(matches!(rx.try_recv(), Some(ToGuest::Key { keycode: 30, .. })));
        assert!(matches!(rx.try_recv(), Some(ToGuest::PointerButton { button: 0x110, .. })));
        assert!(matches!(rx.try_recv(), Some(ToGuest::CloseRequest { id: 1 })));
        assert!(rx.try_recv().is_none());
    }

    #[test]
    fn full_fifo_signals_broken_connection_for_discrete_input() {
        let (queue, rx) = channel();
        for keycode in 0..SEND_QUEUE_BOUND {
            assert_eq!(queue.send(key(1, keycode as u32)), Ok(()));
        }

        assert_eq!(queue.send(click(1, 0x110)), Err(SendQueueError::Full));
        assert_eq!(queue.send(key(1, 31)), Err(SendQueueError::Disconnected));
        assert!(rx.recv().is_some());
    }

    #[test]
    fn full_fifo_runs_close_hook_once() {
        let closed = Arc::new(AtomicUsize::new(0));
        let hook_closed = Arc::clone(&closed);
        let (queue, _rx) = channel_with_on_close(move || {
            hook_closed.fetch_add(1, Ordering::SeqCst);
        });
        for keycode in 0..SEND_QUEUE_BOUND {
            assert_eq!(queue.send(key(1, keycode as u32)), Ok(()));
        }

        assert_eq!(queue.send(click(1, 0x110)), Err(SendQueueError::Full));
        assert_eq!(queue.send(click(1, 0x111)), Err(SendQueueError::Disconnected));
        drop(queue);
        assert_eq!(closed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn motion_after_click_does_not_rewrite_the_click_position() {
        let (queue, rx) = channel();
        assert_eq!(queue.send(motion(1, 1.0)), Ok(()));
        assert_eq!(queue.send(click(1, 0x110)), Ok(()));
        assert_eq!(queue.send(motion(1, 9.0)), Ok(()));
        assert_eq!(queue.send(motion(1, 12.0)), Ok(()));

        // The click barrier splits coalescing: the pre-click position survives
        // untouched, and only the post-click motions collapse together.
        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerMotion { id: 1, x, .. }) if x == 1.0
        ));
        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerButton { button: 0x110, .. })
        ));
        assert!(matches!(
            rx.try_recv(),
            Some(ToGuest::PointerMotion { id: 1, x, .. }) if x == 12.0
        ));
        assert!(rx.try_recv().is_none());
    }
}
