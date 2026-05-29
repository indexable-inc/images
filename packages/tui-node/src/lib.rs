//! Node.js (N-API) bindings for the `tui` PTY-backed terminal manager.
//!
//! This is a thin binding: every method delegates to the `tui` crate, which
//! owns the behavior. I/O methods are async (they return a `Promise` and run on
//! the tui actor without blocking the Node event loop); identity and instant
//! state are synchronous getters.

#![allow(
    clippy::missing_const_for_fn,
    reason = "napi methods are dispatched through the generated addon vtable and cannot be const"
)]
#![allow(
    clippy::must_use_candidate,
    reason = "these values are consumed across the JS boundary, not by Rust callers"
)]
#![allow(
    clippy::missing_errors_doc,
    reason = "fallible methods surface as rejected JS promises; errors are documented in index.d.ts"
)]
#![allow(
    clippy::cast_possible_truncation,
    reason = "JS numbers cross in as u32; terminal dimensions and scrollback fit u16/u32 in practice"
)]
#![allow(
    clippy::use_self,
    reason = "napi-derive resolves exported return types by their concrete name"
)]

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use napi::bindgen_prelude::*;
use napi_derive::napi;

/// One process-wide manager owning the tokio runtime that drives every PTY,
/// mirrored on the Python binding so both share the same lifecycle semantics.
static MANAGER: OnceLock<Arc<tui::TuiManager>> = OnceLock::new();

fn manager() -> Arc<tui::TuiManager> {
    MANAGER
        .get_or_init(|| Arc::new(tui::TuiManager::new()))
        .clone()
}

fn err(source: impl std::fmt::Display) -> Error {
    Error::from_reason(source.to_string())
}

/// JS numbers arrive as `u32`; terminal dimensions and ports are `u16`. Reject
/// out-of-range values instead of silently wrapping them.
fn narrow_u16(name: &str, value: u32) -> Result<u16> {
    u16::try_from(value)
        .map_err(|_| Error::from_reason(format!("{name} must be in 0..=65535, got {value}")))
}

/// Spawn-time terminal configuration. Unset fields fall back to the core
/// defaults (80x24, 10,000 lines of scrollback).
#[napi(object)]
pub struct SpawnOptions {
    pub rows: Option<u32>,
    pub cols: Option<u32>,
    pub scrollback_lines: Option<u32>,
}

/// Scrollback history plus the visible viewport, read together.
#[napi(object)]
pub struct FullOutput {
    pub scrollback: Vec<String>,
    pub viewport: Vec<String>,
}

/// A single spawned PTY-backed process.
#[napi]
pub struct Tui {
    inner: tui::TuiInstance,
}

#[napi]
impl Tui {
    /// Spawn `command` on a fresh PTY and start tracking it.
    #[napi(constructor)]
    pub fn new(
        command: String,
        args: Option<Vec<String>>,
        options: Option<SpawnOptions>,
    ) -> Result<Self> {
        let mut config = tui::SpawnConfig::default();
        if let Some(options) = options {
            if let Some(rows) = options.rows {
                config.rows = narrow_u16("rows", rows)?;
            }
            if let Some(cols) = options.cols {
                config.cols = narrow_u16("cols", cols)?;
            }
            if let Some(scrollback) = options.scrollback_lines {
                config.scrollback_lines = scrollback as usize;
            }
        }
        let inner = manager()
            .spawn(command, args.unwrap_or_default(), config)
            .map_err(err)?;
        Ok(Self { inner })
    }

    /// Every instance the process is currently tracking.
    #[napi]
    pub fn list_all() -> Vec<Tui> {
        manager()
            .list()
            .into_iter()
            .map(|inner| Self { inner })
            .collect()
    }

    // -- identity / shape -------------------------------------------------

    #[napi(getter)]
    pub fn id(&self) -> String {
        self.inner.id.to_string()
    }

    #[napi(getter)]
    pub fn command(&self) -> String {
        self.inner.command.clone()
    }

    #[napi(getter)]
    pub fn args(&self) -> Vec<String> {
        self.inner.args.clone()
    }

    #[napi(getter)]
    pub fn rows(&self) -> u32 {
        u32::from(self.inner.rows())
    }

    #[napi(getter)]
    pub fn cols(&self) -> u32 {
        u32::from(self.inner.cols())
    }

    #[napi(getter)]
    pub fn scrollback_limit(&self) -> u32 {
        self.inner.scrollback_limit as u32
    }

    // -- I/O (async: runs on the actor, never blocks the event loop) ------

    /// Send `data` to the PTY exactly as given.
    #[napi]
    pub async fn write(&self, data: String) -> Result<()> {
        self.inner.write_async(&data).await.map_err(err)
    }

    /// The current viewport, one string per visible row.
    #[napi]
    pub async fn read_viewport(&self) -> Result<Vec<String>> {
        self.inner.read_viewport_async().await.map_err(err)
    }

    /// Lines that have scrolled above the viewport, oldest first.
    #[napi]
    pub async fn read_scrollback(&self) -> Result<Vec<String>> {
        self.inner.read_scrollback_async().await.map_err(err)
    }

    /// Scrollback and viewport read together.
    #[napi]
    pub async fn read_full(&self) -> Result<FullOutput> {
        let full = self.inner.read_full_async().await.map_err(err)?;
        Ok(FullOutput {
            scrollback: full.scrollback,
            viewport: full.viewport,
        })
    }

    /// Read the viewport, waiting up to `timeoutMs` for first content.
    #[napi]
    pub async fn read_blocking(&self, timeout_ms: u32) -> Result<Vec<String>> {
        self.inner
            .read_blocking_async(Duration::from_millis(u64::from(timeout_ms)))
            .await
            .map_err(err)
    }

    // -- lifecycle --------------------------------------------------------

    /// Whether the child process is still running.
    #[napi]
    pub fn is_alive(&self) -> bool {
        self.inner.is_alive()
    }

    /// The exit code, or `null` while running or if terminated by a signal.
    #[napi]
    pub fn exit_code(&self) -> Option<i32> {
        match self.inner.exit_state() {
            tui::ExitState::Exited(code) => code,
            tui::ExitState::Running => None,
        }
    }

    /// Resolve once the child exits, returning its exit code (`null` if it was
    /// terminated by a signal).
    #[napi]
    pub async fn wait(&self) -> Result<Option<i32>> {
        Ok(match self.inner.wait_async().await {
            tui::ExitState::Exited(code) => code,
            tui::ExitState::Running => None,
        })
    }

    /// Force-terminate the child with `SIGKILL`. A no-op if already exited.
    #[napi]
    pub async fn kill(&self) -> Result<()> {
        self.inner.kill_async().await.map_err(err)
    }

    /// Resize the terminal (delivers `SIGWINCH` to the child).
    #[napi]
    pub async fn resize(&self, rows: u32, cols: u32) -> Result<()> {
        let rows = narrow_u16("rows", rows)?;
        let cols = narrow_u16("cols", cols)?;
        self.inner.resize_async(rows, cols).await.map_err(err)
    }

    /// Force-kill the child and stop tracking it, dropping it from `listAll`
    /// and the dashboard.
    #[napi]
    pub fn close(&self) {
        let _ = self.inner.kill();
        let _ = manager().remove(&self.inner.id);
    }
}

/// Handle to a running web dashboard. Stop with `stop()`.
#[napi]
pub struct Dashboard {
    url_value: String,
    addr_value: String,
    inner: Mutex<Option<tui::Dashboard>>,
}

#[napi]
impl Dashboard {
    #[napi(getter)]
    pub fn url(&self) -> String {
        self.url_value.clone()
    }

    #[napi(getter)]
    pub fn addr(&self) -> String {
        self.addr_value.clone()
    }

    /// Stop the server and its poll loop. Idempotent.
    #[napi]
    pub async fn stop(&self) {
        // Take the handle out from under the lock before the await point so the
        // guard never crosses it; the async wind-down then runs on the runtime.
        let taken = self.inner.lock().ok().and_then(|mut guard| guard.take());
        if let Some(mut dashboard) = taken {
            dashboard.stop().await;
        }
    }
}

/// Start the Loro-backed web dashboard for every live terminal in this process.
///
/// `host` must be an IP literal (a hostname is not resolved). Pass `port = 0`
/// for an ephemeral port, read back from `Dashboard.url`. `pollMs` is the
/// viewport sampling interval in milliseconds.
// napi's async-export macro generates a `NapiRefContainer` whose layout ends in
// a zero-sized array; clippy::nursery's `trailing_empty_array` fires on that
// generated type, not on our code, so allow it at the export site.
#[allow(clippy::trailing_empty_array)]
#[napi]
pub async fn serve(
    host: Option<String>,
    port: Option<u32>,
    poll_ms: Option<u32>,
) -> Result<Dashboard> {
    let host = host.unwrap_or_else(|| "127.0.0.1".to_owned());
    let port = narrow_u16("port", port.unwrap_or(8080))?;
    // Parse the host as a bare IP so IPv4 and IPv6 literals both work (an
    // IPv6 address has its own colons, so `format!("{host}:{port}")` would be
    // ambiguous). Hostnames are not resolved and fail closed here.
    let ip: IpAddr = host
        .parse()
        .map_err(|source| err(format!("invalid host {host}: {source}")))?;
    let addr = SocketAddr::new(ip, port);

    // Clamp the poll interval to at least 1ms so a `0` does not spin the
    // dashboard's sample loop, matching the Python wrapper.
    let poll = Duration::from_millis(u64::from(poll_ms.unwrap_or(100)).max(1));
    let dashboard = tui::serve(&manager(), addr, poll).await.map_err(err)?;

    Ok(Dashboard {
        url_value: dashboard.url(),
        addr_value: dashboard.addr().to_string(),
        inner: Mutex::new(Some(dashboard)),
    })
}
