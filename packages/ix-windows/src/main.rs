//! `ix-windows`: open one floating, blurred overlay webview window per live MCP
//! resource.
//!
//! A thin wrapper around [`ix_windows::WindowManager`]. It subscribes to the
//! shared dashboard producer sockets ([`dashboard_core::subscribe`]) on a side
//! tokio runtime and forwards each event to the main-thread `tao` event loop,
//! where windows must live. The MCP already publishes every resource onto those
//! sockets as a `resource/<id>` html pane, so this process renders them with no
//! change to the MCP.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use dashboard_core::{ProducerEvent, discovery_dir, subscribe};
use ix_windows::{UserEvent, WindowManager};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};

/// Render live MCP resources as floating, blurred overlay webview windows.
#[derive(Parser)]
#[command(name = "ix-windows", version, about)]
struct Cli {
    /// Directory of producer sockets to watch. Defaults to the ix discovery
    /// directory (`$IX_DASH_DIR`, `$XDG_RUNTIME_DIR/ix-dash`, or
    /// `/tmp/ix-dash-*`), matching the `dashboard` aggregator.
    #[arg(long)]
    dir: Option<PathBuf>,

    /// How often to rescan the directory for new or removed sockets, in
    /// milliseconds.
    #[arg(long, default_value_t = 500)]
    rescan_ms: u64,
}

/// How often to re-check the cursor against hovered windows' frames
/// ([`WindowManager::sweep_hover`]). Only ticks while a close control is
/// revealed; the loop is otherwise fully event-driven.
const HOVER_SWEEP: Duration = Duration::from_millis(200);

fn main() {
    let cli = Cli::parse();
    let dir = cli.dir.unwrap_or_else(discovery_dir);
    let rescan = Duration::from_millis(cli.rescan_ms);

    // The loop carries `UserEvent`s: producer-stream events from the subscriber
    // thread, plus resize reports posted by each window's measuring script.
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Windows must be created and driven on the main thread, but the subscriber
    // is async. Run it on its own tokio runtime in a side thread and forward
    // every producer event into the event loop via the proxy.
    let producer_proxy = proxy.clone();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        runtime.block_on(async move {
            let mut events = subscribe(dir, rescan, &tokio::runtime::Handle::current());
            while let Some(event) = events.recv().await {
                if producer_proxy.send_event(UserEvent::Producer(event)).is_err() {
                    break; // the event loop has exited; stop forwarding.
                }
            }
        });
    });

    let mut manager = WindowManager::new(proxy);
    event_loop.run(move |event, target, control_flow| {
        // Idle until the next OS or producer event; this is a reactive viewer,
        // not an animation loop.
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Producer(ProducerEvent::Snapshot(snapshot))) => {
                manager.apply_snapshot(target, &snapshot);
            }
            Event::UserEvent(UserEvent::Producer(ProducerEvent::Gone { producer })) => {
                manager.producer_gone(&producer);
            }
            Event::UserEvent(UserEvent::Resize {
                window,
                width,
                height,
            }) => {
                manager.resize(window, width, height);
            }
            Event::UserEvent(UserEvent::Drag { window }) => {
                manager.begin_drag(window);
            }
            Event::UserEvent(UserEvent::Close { window }) => {
                manager.window_closed(window);
            }
            Event::WindowEvent {
                window_id,
                event: WindowEvent::CloseRequested,
                ..
            } => {
                manager.window_closed(window_id);
            }
            // The page saw pointer activity: reveal the close control. The OFF
            // edge is the sweep below, not a page event (see `UserEvent::Hover`;
            // tao's CursorEntered/CursorLeft never fire for these never-key
            // background windows, verified empirically).
            Event::UserEvent(UserEvent::Hover { window }) => {
                manager.set_hovered(window, true);
            }
            _ => {}
        }
        // While any close control is revealed, wake on a short timer and clear
        // the ones the cursor has left; otherwise idle indefinitely. Every wake
        // (timer or event) sweeps, so a stale reveal survives at most one tick.
        if manager.any_hovered() {
            manager.sweep_hover();
        }
        *control_flow = if manager.any_hovered() {
            ControlFlow::WaitUntil(Instant::now() + HOVER_SWEEP)
        } else {
            ControlFlow::Wait
        };
    });
}
