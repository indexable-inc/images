//! BEAM-side support for unibind's rustler backend.
//!
//! The glue that `unibind-backend-ex` generates leans on three pieces that
//! cannot live in generated code: one process-wide tokio [`runtime`] shared
//! by every unibind NIF library in the node, the [`spawn_reply`] plumbing
//! that runs an `async fn` and messages the calling process, the
//! [`spawn_stream`] plumbing that drives a [`unibind_runtime::UniStream`]
//! under consumer demand, and the [`Bytes`] wire newtype that puts binary
//! payloads on the BEAM as binaries. Aside from [`ensure_sigchld_default`], which a NIF
//! that spawns OS child processes calls by hand before its first spawn, user
//! crates never name this crate in their own code (streams are `UniStream<T>`
//! from `unibind-runtime`, shared with every backend); the rest is called by
//! generated code.
//!
//! # Wire protocol
//!
//! Every async call and stream carries a caller-created reference so
//! replies never collide:
//!
//! - async: one `{:unibind, ref, {:ok, value} | {:error, error}}` message.
//! - stream: one `{:unibind_stream, ref, {:item, value}}` per item, then
//!   `{:unibind_stream, ref, :done}`. Items are only produced under demand:
//!   the consumer grants credits through the generated `unibind_demand`
//!   NIF, one credit per item.
//!
//! Both spawns monitor the calling process and abort the task when it
//! exits, so a crashed caller never leaks a future or a producer. Aborted
//! work is dropped, which the user's `Drop` impls observe as cancellation.

mod atoms;
mod bytes;
mod reply;
mod runtime;
mod stream;

pub use bytes::Bytes;
pub use reply::{InFlight, Never, spawn_reply};
pub use runtime::runtime;
pub use stream::{StreamHandle, grant, map_stream, spawn_stream};

/// `Display` and `Error` for a `#[unibind::error]` enum whose every variant
/// carries a single `message` field.
///
/// unibind renders the exception a caller sees from the variant's `Display`
/// text, so every binding has to write one; when the message is already the
/// only field, that impl is the same eleven lines in every crate. Naming the
/// variants here is the whole declaration:
///
/// ```ignore
/// unibind_ex_runtime::message_error!(TuiError {
///     Spawn, NotFound, Io, BadKey, Timeout,
/// });
/// ```
///
/// A binding whose messages are computed rather than stored writes its own
/// `Display` instead; this only covers the pass-through case.
#[macro_export]
macro_rules! message_error {
    ($error:ident { $($variant:ident),+ $(,)? }) => {
        impl ::std::fmt::Display for $error {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                match self {
                    $(Self::$variant { message })|+ => ::std::write!(formatter, "{message}"),
                }
            }
        }

        impl ::std::error::Error for $error {}
    };
}

/// Restore `SIGCHLD` to its default disposition, once per process.
///
/// The BEAM's main VM process ignores `SIGCHLD` (its ports fork from
/// `erl_child_setup`, so the VM expects to own no children), and `SIG_IGN`
/// auto-reaps a NIF's own child processes before it can `waitpid` their exit
/// statuses (`ECHILD`). A NIF that spawns OS child processes and reaps them
/// itself calls this before its first spawn: with `erl_child_setup` in the
/// picture the VM process has no other children to reap.
pub fn ensure_sigchld_default() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: signal(2) with a standard disposition; no handler code runs.
        unsafe {
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
        }
    });
}
