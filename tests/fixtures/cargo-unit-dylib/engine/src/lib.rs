//! Process-global state plus a symbol absorbed from a native static archive.

use std::sync::atomic::{AtomicU64, Ordering};

/// Stands in for a real engine's process-global registry (flecs's component
/// index pool is the case this fixture was written for). Two copies of this
/// static in one process is the failure the dylib prevents.
static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn bump() -> u64 {
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

pub fn count() -> u64 {
    COUNTER.load(Ordering::SeqCst)
}

unsafe extern "C" {
    /// Defined in the static archive `build.rs` compiles and links.
    fn cargo_unit_probe() -> core::ffi::c_int;
}

/// Calling it is what pulls the archive member into the link at all; an
/// unreferenced archive member is never extracted, so an unused probe would
/// make the export assertion vacuous.
pub fn probe() -> i32 {
    unsafe { cargo_unit_probe() }
}
