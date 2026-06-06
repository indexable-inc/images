// Local-user activity tracking + a shared 1 Hz tick store.
//
// The presence doc carries a `last_active_ms` field that is only
// bumped by real user interactions (mouse, keyboard, scroll, touch).
// Heartbeats deliberately don't touch it (see store.ts), so peers
// whose tab is open but idle drift past the IDLE_MS threshold and
// pick up a zzz indicator across the room.
//
// We need two things to make this feel right:
//   1. A throttled broadcaster so genuine activity bumps the field
//      without flooding the socket on every mousemove.
//   2. A second-resolution `nowTick` store so components computing
//      `now - p.last_active_ms > IDLE_MS` re-derive periodically even
//      when no presence update arrives — otherwise an idle peer would
//      only flip to zzz the next time *something* in the doc changed.

import { readable, type Readable } from 'svelte/store';
import { loadIdentity } from './identity';
import { activeRoomStores } from './store';

export const IDLE_MS = 15_000;

export function isIdle(lastActiveMs: number, now: number): boolean {
  return now - lastActiveMs >= IDLE_MS;
}

// Liveness, derived from how stale a peer's last heartbeat is. The
// heartbeat fires every 12 s (see store.ts), so missing two of them
// (24 s) is the earliest moment we should suspect the peer is gone
// rather than just briefly slow; missing four (48 s) is enough
// certainty to stop drawing them at all. Anyone who hits this state
// either crashed, lost the network, or sent an explicit `online:
// false` on graceful close (see store.ts's beforeunload wiring) —
// either way we fade them out instead of leaving them sitting in the
// list with a "zzz" that never resolves.
export const HEARTBEAT_MS = 12_000;
export const DYING_AFTER_MS = HEARTBEAT_MS * 2;
export const GONE_AFTER_MS = HEARTBEAT_MS * 4;
/** How long an explicit `online: false` stays visible as 'dying'
 *  before being treated as gone. Long enough for the fade-out
 *  animation in callers to read as a deliberate goodbye, short enough
 *  that the slot frees up before the next render pass. */
const OFFLINE_DYING_MS = 3_000;

export type PeerLiveness = 'live' | 'dying' | 'gone';

export function peerLiveness(
  p: { online: boolean; last_seen_ms: number },
  now: number
): PeerLiveness {
  const age = now - p.last_seen_ms;
  if (!p.online) return age < OFFLINE_DYING_MS ? 'dying' : 'gone';
  if (age < DYING_AFTER_MS) return 'live';
  if (age < GONE_AFTER_MS) return 'dying';
  return 'gone';
}

// One-second wall-clock tick. Components that want to react to
// idleness (`now - last_active_ms > IDLE_MS`) should read this in
// their `$derived` so the flag flips at the right wall-clock moment
// even when no presence update arrives.
export const nowTick: Readable<number> = readable(Date.now(), (set) => {
  const id = setInterval(() => set(Date.now()), 1000);
  return () => clearInterval(id);
});

// Throttle window for activity broadcasts. We don't need millisecond
// precision on the wire: the receiver only cares whether we crossed
// the 15s idle threshold, so resending every 5s while the user is
// continuously active is more than enough resolution. The first
// event after a quiet period fires immediately (because lastSent is
// 0 or far in the past), which is what makes "wake-from-idle" feel
// snappy on remote peers.
const ACTIVITY_THROTTLE_MS = 5_000;
let lastSentMs = 0;
let started = false;

function markActiveNow() {
  const now = Date.now();
  if (now - lastSentMs < ACTIVITY_THROTTLE_MS) return;
  lastSentMs = now;
  const self = loadIdentity();
  for (const store of activeRoomStores()) {
    store.doc.setSelf(self, { online: true });
  }
}

/**
 * Wire global input listeners that bump our own `last_active_ms`
 * whenever the user actually does something. Idempotent — safe to
 * call from App mount even if HMR re-runs it.
 *
 * Returns a teardown function (mostly useful for tests).
 */
export function startActivityTracking(): () => void {
  if (started || typeof window === 'undefined') return () => {};
  started = true;

  // Capture phase + passive so we see events even when something
  // calls stopPropagation, and we never block scroll/touch handling.
  const opts: AddEventListenerOptions = { capture: true, passive: true };
  const events = [
    'mousemove',
    'mousedown',
    'keydown',
    'wheel',
    'touchstart',
    'pointerdown',
    'scroll'
  ] as const;

  // Visibility transitions: when the tab comes back to the
  // foreground, that's a real "the user is here" signal even if no
  // input event has fired yet. Bypass the throttle so the zzz on
  // remote peers clears immediately.
  function onVisibility() {
    if (document.visibilityState === 'visible') {
      lastSentMs = 0;
      markActiveNow();
    }
  }

  for (const name of events) {
    window.addEventListener(name, markActiveNow, opts);
  }
  document.addEventListener('visibilitychange', onVisibility);

  return () => {
    for (const name of events) {
      window.removeEventListener(name, markActiveNow, opts);
    }
    document.removeEventListener('visibilitychange', onVisibility);
    started = false;
  };
}
