//! Producer side: expose this process's terminals as panes over a unix socket so
//! the out-of-process aggregator can render them.
//!
//! The socket, the wire serialization, and the fan-out all live in
//! [`dashboard_core::Publisher`]; this module is the one adapter that needs the
//! PTY engine. [`publish`] binds that publisher, then spawns a poll loop on the
//! manager's runtime that samples the manager into terminal panes
//! ([`collect_panes`](crate::frame::collect_panes)) and pushes them through the
//! publisher's sink each tick. The poll task is attached to the publisher, so
//! stopping or dropping it winds the loop down with the socket.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use dashboard_core::Publisher;

use crate::frame::collect_panes;
use crate::{Error, Result, TuiManager};

/// Publish `manager`'s terminals as panes on the unix socket at `path`.
///
/// `path` is usually [`socket_path`](crate::socket_path). `poll` is the sampling
/// interval; every tick pushes the current terminal panes to all connected
/// readers. The poll and accept loops run on the manager's runtime, so the
/// producer survives a temporary caller runtime being dropped.
///
/// # Errors
///
/// Returns [`Error::Publish`] when the discovery directory cannot be created or
/// the socket cannot be bound.
pub async fn publish(manager: &Arc<TuiManager>, path: PathBuf, poll: Duration) -> Result<Publisher> {
    let runtime = manager.runtime_handle();
    let mut publisher = Publisher::bind(path, &runtime).map_err(|source| Error::Publish {
        message: source.to_string(),
    })?;

    // Seed the first snapshot before returning so a reader that connects
    // immediately sees the current terminals without waiting a poll interval.
    publisher.publish(&collect_panes(manager).await);

    let sink = publisher.sink();
    let manager = manager.clone();
    let poller = runtime.spawn(async move {
        loop {
            tokio::time::sleep(poll).await;
            sink.publish(&collect_panes(&manager).await);
        }
    });
    publisher.push_task(poller);

    Ok(publisher)
}
