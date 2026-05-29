//! A read-only web dashboard for live PTY terminals.
//!
//! The dashboard renders whatever sits in a [`Hub`]'s Loro document and streams
//! changes to browsers over Server-Sent Events. Two frame sources drive a hub:
//!
//! * [`serve`] polls one [`TuiManager`] in this process (the single-process
//!   view) and applies its terminals under one scope.
//! * the standalone aggregator (`tui-dashboard`) reads many producer sockets
//!   ([`crate::publish`]) and applies each producer under its own scope.
//!
//! Both paths share [`serve_hub`], the router, the page, and the SSE stream, so
//! there is one owner for the browser-facing surface. Loro is only the view-sync
//! layer: the PTYs stay in their owning process, browsers never write back, so
//! the doc has a single editor per scope and conflict resolution never runs; the
//! CRDT buys cheap incremental text diffs and a late joiner catching up from one
//! snapshot.
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

mod hub;
mod server;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::{Result, TuiManager};

pub use hub::Hub;
pub use server::{Dashboard, serve_hub};

/// The scope the in-process dashboard files its terminals under. A single
/// process has one frame source, so one scope keeps every terminal namespaced
/// consistently with the multi-producer aggregator.
const LOCAL_SCOPE: &str = "local";

/// Base64 for the SSE wire. One spelling shared by the snapshot and update
/// encoders in [`server`].
pub(crate) fn b64(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

/// Serve a read-only dashboard of `manager`'s live terminals on `addr`.
///
/// Pass `127.0.0.1:0` to bind an ephemeral port and read it back from
/// [`Dashboard::addr`]. `poll` is the viewport sampling interval; every tick
/// that changes a terminal produces one CRDT update broadcast to all clients.
///
/// The server and poll loop run on the manager's own runtime, not the ambient
/// one, so a pure-Rust caller can `runtime.block_on(serve(..))` from a
/// temporary runtime and the returned dashboard keeps running after it drops.
pub async fn serve(
    manager: &Arc<TuiManager>,
    addr: SocketAddr,
    poll: Duration,
) -> Result<Dashboard> {
    let runtime = manager.runtime_handle();
    let hub = Hub::new();
    let (mut dashboard, mut stop_rx) = serve_hub(hub.clone(), addr, &runtime).await?;

    let manager = manager.clone();
    let poller = runtime.spawn(async move {
        loop {
            let frames = crate::frame::collect_frames(&manager).await;
            hub.apply_scope(LOCAL_SCOPE, &frames);
            tokio::select! {
                () = tokio::time::sleep(poll) => {}
                _ = stop_rx.wait_for(|stop| *stop) => break,
            }
        }
    });
    dashboard.push_task(poller);

    Ok(dashboard)
}
