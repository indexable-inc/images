//! Conformance surface for the unibind Elixir backend (phase 5, #1995).
//!
//! Every export exists so the ExUnit suite in `mix/` can prove one boundary
//! behavior from Elixir: async NIFs reply `{:unibind, ref, {:ok, _}}`, a
//! caller exiting mid-call drops the in-flight future (observable through
//! `cancelled_count`), the BEAM garbage collector runs `Drop` on resources
//! (`dropped_sessions`), and streams only produce under granted demand. The
//! statics are the observable side of behaviors that would otherwise be
//! invisible across the boundary.

/// The exported boundary. The module name names the generated Elixir
/// namespace (`UnibindConformance`) and the OTP app (`:unibind_conformance`).
///
/// The surface itself lives in `src/surface/`, one file per concern, and
/// `parts` is the order they lower in -- which is the order the generated
/// Elixir declares them in, so it is written here rather than inferred.
/// Every `.rs` file in that directory has to appear in the list; one that
/// does not is a compile error naming it.
#[unibind::export(
    backends(ex),
    parts = [
        "src/surface/data.rs",
        "src/surface/echo.rs",
        "src/surface/sessions.rs",
    ]
)]
mod _unibind_conformance {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use unibind_runtime::UniStream;
}
