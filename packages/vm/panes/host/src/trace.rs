//! Env-gated latency instrumentation: `PANES_TRACE=1` emits one parseable
//! stderr line per forwarded input event, ingested frame, and present, for
//! the mouse-look latency work (index#1686). Timestamps are
//! `CACurrentMediaTime()` seconds -- the same mach clock `NSEvent.timestamp`
//! and `CAMetalDisplayLinkUpdate` targets use, and the domain macOS Python's
//! `time.monotonic()` reads -- so an external probe on the same machine
//! correlates its clock with these lines with no conversion.
//!
//! Off (the default), each call site costs one atomic load and no
//! formatting, cheap enough to live in the 120Hz paths permanently.

use std::sync::atomic::{AtomicU8, Ordering};

use objc2_quartz_core::CACurrentMediaTime;

/// 0 = unresolved, 1 = off, 2 = on. A plain atomic (not `OnceLock`) keeps
/// the hot-path read a single relaxed load.
static STATE: AtomicU8 = AtomicU8::new(0);

pub fn enabled() -> bool {
    match STATE.load(Ordering::Relaxed) {
        0 => {
            let on = std::env::var_os("PANES_TRACE").is_some_and(|value| value != "0");
            STATE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
        state => state == 2,
    }
}

/// Seconds on the `NSEvent.timestamp` clock (mach time since boot).
pub fn now() -> f64 {
    CACurrentMediaTime()
}
