//! BEAM-side support for unibind's rustler backend.
//!
//! The glue that `unibind-backend-ex` generates leans on three pieces that
//! cannot live in generated code: one process-wide tokio [`runtime`] shared
//! by every unibind NIF library in the node, the [`spawn_reply`] plumbing
//! that runs an `async fn` and messages the calling process, and the
//! [`spawn_stream`] plumbing that drives a [`unibind_runtime::UniStream`]
//! under consumer demand. User crates never name this crate in their own
//! code (streams are `UniStream<T>` from `unibind-runtime`, shared with
//! every backend); everything here is called by generated code.
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
mod reply;
mod runtime;
mod stream;

pub use reply::{spawn_reply, InFlight, Never};
pub use runtime::runtime;
pub use stream::{grant, spawn_stream, StreamHandle};
