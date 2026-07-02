//! Main-thread application state. Every window and the outgoing sender live
//! in a main-thread `thread_local`; the supervisor dispatches events here via
//! the main queue, and `AppKit` delegate/view callbacks call the helpers below
//! directly (they already run on the main thread).
//!
//! Re-entrancy rule: `AppKit` calls that synchronously fire delegate methods
//! (`close`, `makeKeyAndOrderFront`) are deferred until the `APP` borrow is
//! released, because those delegates call straight back into this module.

use std::cell::RefCell;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSScreen, NSWindow};
use objc2_quartz_core::CAMetalDisplayLinkUpdate;
use panes_protocol::{ToGuest, ToHost, WindowId};

use crate::conn::{self, Event, HostInfo, Target};
use crate::render::Renderer;
use crate::window::{PaneWindow, WindowParams};

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

struct App {
    mtm: MainThreadMarker,
    renderer: Renderer,
    windows: HashMap<WindowId, PaneWindow>,
    out: Option<mpsc::Sender<ToGuest>>,
    title_prefix: String,
    quitting: bool,
    /// Per-window ack counters behind the periodic acks/s log: the real-path
    /// equivalent of mock's rate line, and the 120Hz-genlock evidence
    /// (index#1686 M6). An entry resets on each report.
    ack_stats: HashMap<WindowId, AckStat>,
}

struct AckStat {
    count: u32,
    epoch: Instant,
}

/// How often `display_tick` reports a window's ack rate.
const ACK_RATE_REPORT_EVERY: Duration = Duration::from_secs(5);

/// `AppKit` work postponed past the `APP` borrow (see module docs).
#[derive(Default)]
struct Deferred {
    show: Option<Retained<NSWindow>>,
    close: Vec<PaneWindow>,
}

impl Deferred {
    fn run(self, mtm: MainThreadMarker) {
        for window in self.close {
            window.shutdown();
        }
        if let Some(ns) = self.show {
            ns.makeKeyAndOrderFront(None);
            // Unbundled binaries start inactive; activate so the first
            // window actually takes key focus.
            NSApplication::sharedApplication(mtm).activate();
        }
    }
}

pub fn run(target: Target, title_prefix: String) -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("panes-host: must start on the main thread");
        return ExitCode::FAILURE;
    };
    let ns_app = NSApplication::sharedApplication(mtm);
    // Regular: guest windows should feel native, which includes Dock
    // presence and Cmd-Tab participation.
    ns_app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let screen = NSScreen::mainScreen(mtm);
    let max_fps = screen.as_ref().map_or(60, |screen| screen.maximumFramesPerSecond());
    let backing = screen.as_ref().map_or(2.0, |screen| screen.backingScaleFactor());
    eprintln!(
        "panes-host: main screen maximumFramesPerSecond={max_fps} backingScaleFactor={backing}"
    );

    let renderer = match Renderer::new() {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("panes-host: metal setup failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    APP.with(|slot| {
        *slot.borrow_mut() = Some(App {
            mtm,
            renderer,
            windows: HashMap::new(),
            out: None,
            title_prefix,
            quitting: false,
            ack_stats: HashMap::new(),
        });
    });

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let host = HostInfo {
        // Clamped into u32 range explicitly: no real panel exceeds 1000Hz,
        // and the clamp makes the (impossible) fallback a visible policy
        // rather than a silent unwrap_or.
        refresh_mhz: u32::try_from(max_fps.clamp(1, 1000)).expect("clamped to 1..=1000") * 1000,
        scale: (backing.round().max(1.0)) as u32,
    };
    conn::spawn(target, host);

    ns_app.run();
    ExitCode::SUCCESS
}

/// Entry point for supervisor events, always on the main queue.
pub fn on_event(event: Event) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let deferred = with_app(|app| match event {
        Event::Connected(tx) => {
            app.out = Some(tx);
            Deferred::default()
        }
        Event::Disconnected => {
            app.out = None;
            app.ack_stats.clear();
            // Guest windows cannot outlive the guest connection.
            let close = app.windows.drain().map(|(_, window)| window).collect();
            Deferred { show: None, close }
        }
        Event::Msg(msg) => handle_msg(app, msg),
    });
    if let Some(deferred) = deferred {
        deferred.run(mtm);
    }
}

fn handle_msg(app: &mut App, msg: ToHost) -> Deferred {
    match msg {
        // The reader consumes Hello during version negotiation.
        ToHost::Hello { .. } | ToHost::Pong { .. } => Deferred::default(),
        ToHost::WindowNew { id, title, app_id, width, height, scale } => {
            if app.windows.contains_key(&id) {
                eprintln!("panes-host: duplicate WindowNew for {id}, ignoring");
                return Deferred::default();
            }
            let params = WindowParams { id, title, app_id, width, height, scale };
            let window = PaneWindow::new(app.mtm, &app.renderer, &params, &app.title_prefix);
            app.windows.insert(id, window);
            Deferred::default()
        }
        ToHost::WindowTitle { id, title } => {
            if let Some(window) = app.windows.get(&id) {
                window.set_title(&app.title_prefix, &title);
            }
            Deferred::default()
        }
        ToHost::WindowMinMax { id, min, max } => {
            if let Some(window) = app.windows.get(&id) {
                window.set_min_max(min, max);
            }
            Deferred::default()
        }
        ToHost::WindowFrame { id, seq, width, height, full, tiles } => {
            let Some(window) = app.windows.get_mut(&id) else {
                eprintln!("panes-host: frame for unknown window {id}");
                return Deferred::default();
            };
            if !window.apply_frame(&app.renderer, seq, width, height, full, tiles) {
                // Frame the host could not take (zero-size / texture alloc
                // failure): ack immediately anyway. With one-frame-in-flight
                // guest pacing, an ack held hostage to a texture we never
                // made would wedge that window's frame loop forever.
                if let Some(out) = &app.out {
                    let _ = out.send(ToGuest::Ack { id, seq });
                }
                return Deferred::default();
            }
            // First content: order the window in only now, so an empty
            // window never flashes (protocol contract on WindowNew).
            if window.shown {
                Deferred::default()
            } else {
                window.shown = true;
                Deferred { show: Some(window.ns.clone()), close: Vec::new() }
            }
        }
        ToHost::WindowGone { id } => {
            let mut deferred = Deferred::default();
            app.ack_stats.remove(&id);
            if let Some(mut window) = app.windows.remove(&id) {
                window.closing = true;
                deferred.close.push(window);
            }
            deferred
        }
        ToHost::Cursor { .. } => {
            // v1 keeps the host cursor; guest cursor imagery is a follow-up.
            Deferred::default()
        }
    }
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|slot| slot.borrow_mut().as_mut().map(f))
}

/// Queue a message to the guest; silently dropped while disconnected (every
/// caller is reacting to UI events that are meaningless without a guest).
pub fn send(msg: ToGuest) {
    with_app(|app| {
        if let Some(out) = &app.out {
            let _ = out.send(msg);
        }
    });
}

/// Display-link tick for one window: present if dirty, then ack, in that
/// order; the ack is what releases the guest's next frame.
pub fn display_tick(id: WindowId, update: &CAMetalDisplayLinkUpdate) {
    with_app(|app| {
        let Some(window) = app.windows.get_mut(&id) else {
            return;
        };
        if let Some(seq) = window.present(&app.renderer, update)
            && let Some(out) = &app.out
        {
            let _ = out.send(ToGuest::Ack { id, seq });
            let stat = app
                .ack_stats
                .entry(id)
                .or_insert_with(|| AckStat { count: 0, epoch: Instant::now() });
            stat.count += 1;
            let elapsed = stat.epoch.elapsed();
            if elapsed >= ACK_RATE_REPORT_EVERY {
                let rate = f64::from(stat.count) / elapsed.as_secs_f64();
                eprintln!("panes-host: window {id}: {rate:.1} acks/s");
                stat.count = 0;
                stat.epoch = Instant::now();
            }
        }
    });
}

pub fn window_should_close(id: WindowId) -> bool {
    with_app(|app| {
        let Some(window) = app.windows.get(&id) else {
            // Unknown to the guest (already gone): let AppKit close it.
            return true;
        };
        if window.closing {
            return true;
        }
        if let Some(out) = &app.out {
            let _ = out.send(ToGuest::CloseRequest { id });
        }
        // The guest decides: it unmaps and sends WindowGone, which closes
        // the NSWindow for real (the WSLg model: never desync window
        // existence from the compositor).
        false
    })
    .unwrap_or(true)
}

pub fn window_closed(id: WindowId) {
    with_app(|app| {
        // Normally removed already (WindowGone / disconnect); this catches
        // AppKit-initiated closes so state cannot leak.
        app.windows.remove(&id);
    });
}

pub fn window_geometry_changed(id: WindowId) {
    with_app(|app| {
        let Some(window) = app.windows.get_mut(&id) else {
            return;
        };
        let size = window.sync_layer_geometry();
        // Redraw immediately at the new size (stretching stale content)
        // rather than leaving undefined drawable pixels until the guest's
        // matching frame lands.
        window.mark_dirty();
        let activated = window.ns.isKeyWindow();
        if let Some(out) = &app.out {
            let _ = out.send(ToGuest::Configure {
                id,
                width: size.width,
                height: size.height,
                scale: size.scale,
                activated,
            });
        }
    });
}

pub fn window_live_resize(id: WindowId, active: bool) {
    with_app(|app| {
        if let Some(window) = app.windows.get(&id) {
            window.live_resize(active);
        }
    });
    if !active {
        // Final geometry after the user let go.
        window_geometry_changed(id);
    }
}

pub fn window_activation(id: WindowId, activated: bool) {
    if !activated {
        // Held modifiers must not outlive key status: AppKit stops sending
        // flagsChanged after resign-key, so release them guest-side before
        // the deactivated Configure. Outside the with_app borrow because the
        // view sends protocol messages through app state.
        let view = with_app(|app| app.windows.get(&id).map(PaneWindow::view_handle)).flatten();
        if let Some(view) = view {
            view.release_held_modifiers();
        }
    }
    with_app(|app| {
        let Some(window) = app.windows.get(&id) else {
            return;
        };
        let size = window.sync_layer_geometry();
        if let Some(out) = &app.out {
            let _ = out.send(ToGuest::Configure {
                id,
                width: size.width,
                height: size.height,
                scale: size.scale,
                activated,
            });
        }
    });
}

/// Cmd+W (and the red button, via `windowShouldClose`).
pub fn close_requested(id: WindowId) {
    send(ToGuest::CloseRequest { id });
}

/// Cmd+Q: ask the guest to close everything, then exit. The grace period
/// lets the writer thread flush the `CloseRequest`s; we do not wait for
/// `WindowGone` because a hung guest must not pin the host app open.
pub fn request_quit() {
    let already_quitting = with_app(|app| {
        if app.quitting {
            return true;
        }
        app.quitting = true;
        if let Some(out) = &app.out {
            for id in app.windows.keys() {
                let _ = out.send(ToGuest::CloseRequest { id: *id });
            }
        }
        false
    })
    .unwrap_or(false);
    if already_quitting {
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(300));
        std::process::exit(0);
    });
}
