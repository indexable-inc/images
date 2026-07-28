//! The process-wide tokio runtime backing async and stream NIFs.

use std::sync::OnceLock;

use tokio::runtime::{Builder, Runtime};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// The shared multi-thread runtime every unibind NIF library spawns onto.
///
/// Built on first use and never shut down: BEAM nodes have no orderly
/// library teardown, and the worker threads ("unibind-ex") park when idle.
///
/// # Panics
///
/// Panics when the runtime cannot be built, which means the OS refused to
/// spawn threads.
#[must_use]
pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .thread_name("unibind-ex")
            // Timers and I/O both on: exported futures use tokio's whole
            // surface, and a driver-less runtime panics at first use (the
            // conformance suite caught `tokio::time::sleep` doing exactly
            // that).
            .enable_all()
            .build()
            .expect("building the unibind tokio runtime failed")
    })
}
