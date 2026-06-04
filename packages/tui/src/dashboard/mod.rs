//! The in-process dashboard: poll one [`TuiManager`] and render its terminals.
//!
//! [`serve`] binds the engine-free dashboard server from `dashboard-core` and
//! drives it from a poll loop over a single [`TuiManager`] in this process,
//! filing every terminal as a pane under one scope. The browser-facing surface
//! (the [`Hub`] Loro document, the router, the SSE stream, the [`Dashboard`]
//! handle) lives in `dashboard-core`; this module only owns the bridge from a
//! live manager to that surface.
//!
//! The standalone aggregator (`dashboard`) drives the same surface from many
//! producer sockets instead, so the only engine-bound frame source stays here.
//!
//! ```no_run
//! use std::sync::Arc;
//! use std::time::Duration;
//! use tui::TuiManager;
//!
//! # async fn run() -> Result<(), tui::Error> {
//! let manager = Arc::new(TuiManager::new());
//! let dashboard = tui::serve(&manager, "127.0.0.1:0".parse().unwrap(), Duration::from_millis(100)).await?;
//! println!("open {}", dashboard.url());
//! # Ok(())
//! # }
//! ```

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use dashboard_core::{Dashboard, Hub, serve_hub};

use crate::{Error, Result, TuiManager};

/// The scope the in-process dashboard files its terminals under. A single
/// process has one frame source, so one scope keeps every terminal namespaced
/// consistently with the multi-producer aggregator.
const LOCAL_SCOPE: &str = "local";

/// Serve a read-only dashboard of `manager`'s live terminals on `addr`.
///
/// Pass `127.0.0.1:0` to bind an ephemeral port and read it back from
/// [`Dashboard::addr`]. `poll` is the viewport sampling interval; every tick
/// that changes a terminal produces one CRDT update broadcast to all clients.
///
/// The server and poll loop run on the manager's own runtime, not the ambient
/// one, so a pure-Rust caller can `runtime.block_on(serve(..))` from a
/// temporary runtime and the returned dashboard keeps running after it drops.
///
/// # Errors
///
/// Returns [`Error::Dashboard`] when the server cannot bind `addr`.
pub async fn serve(
    manager: &Arc<TuiManager>,
    addr: SocketAddr,
    poll: Duration,
) -> Result<Dashboard> {
    let runtime = manager.runtime_handle();
    let hub = Hub::new();
    // The in-process dashboard streams a live manager; persisted recordings are
    // the standalone aggregator's job, so it serves the replay routes empty.
    let served = serve_hub(hub.clone(), addr, None, &runtime)
        .await
        .map_err(|source| Error::Dashboard {
            message: source.to_string(),
        })?;
    let mut dashboard = served.dashboard;
    let mut stop_rx = served.shutdown;

    let manager = manager.clone();
    let poller = runtime.spawn(async move {
        loop {
            let panes = crate::frame::collect_panes(&manager).await;
            hub.apply_scope(LOCAL_SCOPE, &panes);
            tokio::select! {
                () = tokio::time::sleep(poll) => {}
                _ = stop_rx.wait_for(|stop| *stop) => break,
            }
        }
    });
    dashboard.push_task(poller);

    Ok(dashboard)
}
