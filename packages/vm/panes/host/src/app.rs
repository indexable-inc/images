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
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSCursor, NSEvent,
    NSScreen, NSWindow,
};
use dispatch2::DispatchQueue;
use objc2_core_graphics::{CGAssociateMouseAndMouseCursorPosition, CGError};
use objc2_foundation::{NSNotification, NSObjectProtocol};
use objc2_metal::MTLDrawable as _;
use objc2_quartz_core::CAMetalDisplayLinkUpdate;
use panes_protocol::{MINOR_KEY_REPEAT, MINOR_POINTER_LOCK, ToGuest, ToHost, WindowId};

use crate::conn::{self, Event, Target};
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
    /// `--native-titlebar`: stock macOS chrome instead of the default
    /// hidden-titlebar style (see `window::apply_hidden_titlebar`).
    native_titlebar: bool,
    quitting: bool,
    /// Per-window ack counters behind the periodic acks/s log: the real-path
    /// equivalent of mock's rate line, and the 120Hz-genlock evidence
    /// (index#1686 M6). An entry resets on each report.
    ack_stats: HashMap<WindowId, AckStat>,
    /// Guest protocol minor from its Hello; gates `PointerRelative` (a 1.1
    /// message postcard-decodes to an error on a 1.0 guest). 0 until known.
    peer_minor: u16,
    /// The engaged cursor capture, at most one. `Option` assignment is the
    /// whole engage/release mechanism: dropping the old value restores the
    /// cursor (see [`CursorCapture`]).
    capture: Option<CursorCapture>,
    /// Shared with the connection supervisor, which reads it for each
    /// (re)connect's Hello; rewritten by [`screens_changed`] so the facts
    /// track the live screen table.
    host_info: Arc<conn::HostInfo>,
}

/// An engaged pointer capture: cursor hidden and dissociated from mouse
/// movement so `NSEvent` deltas drive the guest's relative pointer while the
/// real cursor stays parked. The un-capture lives in `Drop`, so no path can
/// release the window without also giving the user their cursor back (the
/// failure mode to be paranoid about).
struct CursorCapture {
    id: WindowId,
}

impl CursorCapture {
    fn engage(id: WindowId) -> Self {
        eprintln!("panes-host: window {id}: pointer captured (cursor hidden, relative deltas)");
        // Hide/unhide nest globally, so exactly one hide per capture,
        // balanced by the one unhide in Drop.
        NSCursor::hide();
        let err = CGAssociateMouseAndMouseCursorPosition(false);
        if err != CGError::Success {
            eprintln!("panes-host: window {id}: cursor dissociation failed: {err:?}");
        }
        // Coalescing merges queued mouse-moved events (summed deltas, so no
        // motion is lost) but delays delivery to the queue drain; measured
        // at ~21% of a 1kHz delta stream merged under frame load
        // (index#1686). Mouse-look wants every delta as fresh as the queue
        // can hand it over, so coalescing is off exactly while captured;
        // absolute-cursor mode keeps the AppKit default.
        NSEvent::setMouseCoalescingEnabled(false);
        Self { id }
    }
}

impl Drop for CursorCapture {
    fn drop(&mut self) {
        eprintln!("panes-host: window {}: pointer capture released", self.id);
        NSEvent::setMouseCoalescingEnabled(true);
        let err = CGAssociateMouseAndMouseCursorPosition(true);
        if err != CGError::Success {
            eprintln!("panes-host: window {}: cursor re-association failed: {err:?}", self.id);
        }
        // NSCursor.hide nests, and the engage-side re-hides
        // (`reassert_capture_cursor`) may have raised the count past one:
        // whether the OS-forced show on right-mouse-down decrements the
        // counter is undocumented, so a single unhide here could strand the
        // cursor hidden system-wide, the worst failure this struct exists to
        // prevent. Unhide until the cursor is actually visible; bounded as
        // paranoia against a wedged visibility query, and stopping at
        // visible avoids driving the counter needlessly negative.
        #[allow(deprecated)] // CGCursorIsVisible: see reassert_capture_cursor.
        for _ in 0..16 {
            NSCursor::unhide();
            if objc2_core_graphics::CGCursorIsVisible() {
                break;
            }
        }
    }
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

pub fn run(target: Target, title_prefix: String, native_titlebar: bool) -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("panes-host: must start on the main thread");
        return ExitCode::FAILURE;
    };
    let ns_app = NSApplication::sharedApplication(mtm);
    // Regular: guest windows should feel native, which includes Dock
    // presence and Cmd-Tab participation.
    ns_app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    log_screens(mtm);
    let facts = read_screen_facts(mtm);
    let host_info = Arc::new(conn::HostInfo {
        refresh_mhz: AtomicU32::new(facts.refresh_mhz),
        scale: AtomicU32::new(facts.scale),
    });

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
            native_titlebar,
            quitting: false,
            ack_stats: HashMap::new(),
            peer_minor: 0,
            capture: None,
            host_info: host_info.clone(),
        });
    });

    // App activation delegate: a deactivated app must never hold the user's
    // cursor hostage, so capture is released on resign-active and re-engaged
    // (if the guest still wants it) on reactivation. Also the screen-change
    // hook (see `screens_changed`).
    let delegate = AppDelegate::new(mtm);
    ns_app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    conn::spawn(target, host_info);

    ns_app.run();
    ExitCode::SUCCESS
}

/// One line per attached screen with the facts presentation depends on
/// (backing scale, max refresh, frame, which one is main); logged at startup
/// and again on every screen-parameters change. Replaces a one-shot startup
/// summary that kept being read as current truth hours later: a live 1x/60Hz
/// incident was misdiagnosed as a bad `NSScreen` read when the host had
/// simply been launched while a 1x external display was the main screen
/// (index#1686).
fn log_screens(mtm: MainThreadMarker) {
    // Frame equality marks the main screen: screens never overlap exactly,
    // and it avoids leaning on NSScreen instance identity across the two
    // accessors.
    let main_frame = NSScreen::mainScreen(mtm).map(|screen| screen.frame());
    for screen in NSScreen::screens(mtm) {
        let frame = screen.frame();
        eprintln!(
            "panes-host: screen {}x{} at ({}, {}): backingScaleFactor={} \
             maximumFramesPerSecond={}{}",
            frame.size.width,
            frame.size.height,
            frame.origin.x,
            frame.origin.y,
            screen.backingScaleFactor(),
            screen.maximumFramesPerSecond(),
            if main_frame == Some(frame) { " (main)" } else { "" },
        );
    }
}

/// Snapshot of the screen-derived facts [`ToGuest::Hello`] advertises.
struct ScreenFacts {
    refresh_mhz: u32,
    scale: u32,
}

/// Facts [`ToGuest::Hello`] advertises, from the current screen table:
/// refresh from the main screen, scale = the HIGHEST `backingScaleFactor` of
/// any attached display, not the main screen's. A host launched (or
/// reconnecting) while a 1x display is frontmost must not pin every guest
/// client to 1x buffers stretched over Retina drawables (seen live with
/// GLFW/Minecraft, index#1686); windows that land on a lower-scale screen
/// are corrected per window by their Configure. Fallback 2.0 (headless / no
/// screens) errs toward sharp.
fn read_screen_facts(mtm: MainThreadMarker) -> ScreenFacts {
    let max_fps =
        NSScreen::mainScreen(mtm).map_or(60, |screen| screen.maximumFramesPerSecond());
    let backing = NSScreen::screens(mtm)
        .iter()
        .map(|screen| screen.backingScaleFactor())
        .fold(0.0_f64, f64::max);
    let backing = if backing > 0.0 { backing } else { 2.0 };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    ScreenFacts {
        // Clamped into u32 range explicitly: no real panel exceeds 1000Hz,
        // and the clamp makes the (impossible) fallback a visible policy
        // rather than a silent unwrap_or.
        refresh_mhz: u32::try_from(max_fps.clamp(1, 1000)).expect("clamped to 1..=1000") * 1000,
        scale: (backing.round().max(1.0)) as u32,
    }
}

/// `applicationDidChangeScreenParameters:`: displays attach, detach, or
/// change mode/scale in place, with no reliable per-window signal
/// (`windowDidChangeScreen:` only fires when a window lands on a different
/// screen object; an in-place mode switch fires nothing window-level). Seen
/// live (index#1686): a window left on the built-in panel after a 1x/60Hz
/// external detached kept ticking at ~60 acks/s. Re-derive everything
/// screen-dependent: refresh the Hello facts for future reconnects, re-pin
/// every window's stream rate, and re-sync layer geometry so the guest gets
/// a Configure with the window's current backing scale.
fn screens_changed() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    log_screens(mtm);
    let facts = read_screen_facts(mtm);
    with_app(|app| {
        // Relaxed matches the reader (conn.rs): independent u32 facts.
        app.host_info.refresh_mhz.store(facts.refresh_mhz, Ordering::Relaxed);
        app.host_info.scale.store(facts.scale, Ordering::Relaxed);
        for (id, window) in &mut app.windows {
            window.refresh_stream_rate(mtm);
            let size = window.sync_layer_geometry();
            window.mark_dirty();
            let activated = window.ns.isKeyWindow();
            if let Some(out) = &app.out {
                let _ = out.send(ToGuest::Configure {
                    id: *id,
                    width: size.width,
                    height: size.height,
                    scale: size.scale,
                    activated,
                });
            }
        }
    });
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
        Event::Hello { minor } => {
            app.peer_minor = minor;
            send_key_repeat(app);
            Deferred::default()
        }
        Event::Disconnected => {
            app.out = None;
            app.ack_stats.clear();
            app.peer_minor = 0;
            // The capture cannot outlive the lock-holding guest. peer_minor
            // is 0 now, so reconciling releases it through the one shared
            // path (the view leaves relative mode too) while the windows
            // still exist, before they are torn down below.
            sync_capture(app);
            // Guest windows cannot outlive the guest connection.
            let close = app.windows.drain().map(|(_, window)| window).collect();
            Deferred { show: None, close }
        }
        Event::Msg { msg, recv } => handle_msg(app, msg, recv),
    });
    if let Some(deferred) = deferred {
        deferred.run(mtm);
    }
}

fn handle_msg(app: &mut App, msg: ToHost, recv: f64) -> Deferred {
    match msg {
        // The reader consumes Hello during version negotiation.
        ToHost::Hello { .. } | ToHost::Pong { .. } => Deferred::default(),
        ToHost::WindowNew { id, title, app_id, width, height, scale } => {
            if app.windows.contains_key(&id) {
                eprintln!("panes-host: duplicate WindowNew for {id}, ignoring");
                return Deferred::default();
            }
            let params = WindowParams { id, title, app_id, width, height, scale };
            let window = PaneWindow::new(
                app.mtm,
                &app.renderer,
                &params,
                &app.title_prefix,
                app.native_titlebar,
            );
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
            let trace_bytes = crate::trace::enabled()
                .then(|| (tiles.len(), tiles.iter().map(|tile| tile.payload.len()).sum::<usize>()));
            let Some(window) = app.windows.get_mut(&id) else {
                eprintln!("panes-host: frame for unknown window {id}");
                return Deferred::default();
            };
            let applied = window.apply_frame(&app.renderer, seq, width, height, full, tiles);
            if let Some((tiles, bytes)) = trace_bytes {
                // recv is stamped on the reader thread right after the wire
                // decode (see conn::read_loop), so recv..done spans the
                // main-queue wait plus the damage-log ingest: the time a
                // frame occupies or waits on the main thread that input
                // events share.
                eprintln!(
                    "panes-trace frame id={id} seq={seq} recv={recv:.6} done={:.6} \
                     tiles={tiles} bytes={bytes}",
                    crate::trace::now(),
                );
            }
            if !applied {
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
            // A locked window that vanished must release the capture.
            sync_capture(app);
            deferred
        }
        ToHost::Cursor { .. } => {
            // v1 keeps the host cursor; guest cursor imagery is a follow-up.
            Deferred::default()
        }
        ToHost::PointerLock { id, locked } => {
            if let Some(window) = app.windows.get_mut(&id) {
                window.wants_lock = locked;
            } else {
                eprintln!("panes-host: pointer lock for unknown window {id}");
            }
            sync_capture(app);
            Deferred::default()
        }
        ToHost::WindowScale { id, scale } => {
            if let Some(window) = app.windows.get_mut(&id) {
                window.set_guest_scale(scale);
            }
            Deferred::default()
        }
    }
}

/// Tell the guest the user's actual macOS key-repeat timing (System
/// Settings, via `NSEvent`'s class getters) so `wl_keyboard.repeat_info`
/// matches the host exactly. The host drops `isARepeat` keyDowns, so this
/// advertisement is the guest's only repeat authority. Sent once per
/// connection (a mid-session System Settings change applies on reconnect);
/// gated on the guest speaking 1.2, postcard cannot skip an unknown variant.
fn send_key_repeat(app: &App) {
    if app.peer_minor < MINOR_KEY_REPEAT {
        return;
    }
    let Some(out) = &app.out else {
        return;
    };
    let msg = ToGuest::KeyRepeat {
        delay_ms: whole_ms(NSEvent::keyRepeatDelay()),
        interval_ms: whole_ms(NSEvent::keyRepeatInterval()),
    };
    eprintln!("panes-host: key repeat: {msg:?}");
    let _ = out.send(msg);
}

/// Seconds (`NSTimeInterval`) to whole milliseconds. `as` saturates: a
/// negative or NaN interval becomes 0 (the guest disables repeat for it, see
/// `panes_protocol::wl_repeat_info`), and macOS "Key Repeat: Off" reports a
/// minutes-long interval that stays finite here.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn whole_ms(seconds: f64) -> u32 {
    (seconds * 1000.0).round() as u32
}

/// Re-hide the cursor when macOS revealed it behind an engaged capture's
/// back, now and once more after `AppKit` finishes the current event.
/// `AppKit` spontaneously unhides a hidden cursor on paths of its own -- the
/// right-mouse-down menu-preparation path is the one guests actually hit
/// (holding right-click in a pointer-locked game showed the cursor); GLFW
/// catalogs more (screenshot mode, dock hover: glfw#2648, glfw#2656). Each
/// such show may or may not decrement the `NSCursor.hide` nesting counter
/// (undocumented), so the guard only re-hides while the cursor is actually
/// visible, and the capture's `Drop` symmetrically unhides until visible:
/// correct under either counter semantic, and release can never strand a
/// hidden cursor.
/// The deferred second look exists because the OS unhide lands during event
/// processing, ordered unpredictably against the view handler that calls
/// this; the back of the main queue is reliably after both.
pub fn reassert_capture_cursor() {
    fn rehide_if_visible(app: &mut App) {
        if app.capture.is_none() {
            return;
        }
        // CGCursorIsVisible is deprecated without a replacement, and there
        // is no event for "the OS unhid your cursor": visibility can only
        // be polled. GLFW ships the same call for the same reason.
        #[allow(deprecated)]
        if objc2_core_graphics::CGCursorIsVisible() {
            eprintln!("panes-host: OS unhid the cursor while captured; re-hiding");
            NSCursor::hide();
        }
    }
    with_app(rehide_if_visible);
    DispatchQueue::main().exec_async(|| {
        with_app(rehide_if_visible);
    });
}

/// Reconcile the engaged cursor capture with what the guest wants and what
/// the user is looking at: capture exactly when a lock-holding window is the
/// key window of the active app (and the guest speaks 1.1, so the
/// `PointerRelative` stream it implies is legal to send). Idempotent, called
/// from every transition that can change an input: `PointerLock`, key-window
/// changes, window close, app (de)activation, disconnect.
fn sync_capture(app: &mut App) {
    let desired = if app.peer_minor >= MINOR_POINTER_LOCK
        && NSApplication::sharedApplication(app.mtm).isActive()
    {
        app.windows
            .iter()
            .find(|(_, window)| window.wants_lock && window.ns.isKeyWindow())
            .map(|(id, _)| *id)
    } else {
        None
    };
    if app.capture.as_ref().map(|capture| capture.id) == desired {
        return;
    }
    if let Some(capture) = app.capture.take() {
        // The window may already be gone (WindowGone/close paths).
        if let Some(window) = app.windows.get(&capture.id) {
            window.view_handle().set_relative(false);
        }
        // `capture` drops here: cursor re-associated and unhidden.
    }
    if let Some(id) = desired
        && let Some(window) = app.windows.get(&id)
    {
        window.view_handle().set_relative(true);
        app.capture = Some(CursorCapture::engage(id));
        // macOS ignores a hide issued while the cursor hovers the dock
        // (glfw#2656: refocus-over-dock), so double-check once this engage's
        // event settles. Deferred: we are inside the APP borrow here.
        DispatchQueue::main().exec_async(|| {
            reassert_capture_cursor();
        });
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
            if crate::trace::enabled() {
                // `target - now` is how far ahead of glass this tick runs
                // (index#1686 latency work); did/dc expose the drawable pool
                // (drawableID cycling and the layer's claimed cap).
                eprintln!(
                    "panes-trace present id={id} seq={seq} now={:.6} target={:.6} did={} dc={}",
                    crate::trace::now(),
                    update.targetPresentationTimestamp(),
                    update.drawable().drawableID(),
                    window.max_drawable_count(),
                );
            }
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
        // Never leave the cursor dissociated for a window that closed.
        sync_capture(app);
    });
}

/// Occlusion change: a fully covered window keeps presenting and acking at
/// the full display rate on its own (`CAMetalDisplayLink` does not stop for
/// occlusion), so downshift it to ack-only ticks; see
/// [`PaneWindow::set_occluded`].
pub fn window_occlusion_changed(id: WindowId) {
    with_app(|app| {
        let Some(window) = app.windows.get_mut(&id) else {
            return;
        };
        // A window not yet ordered in reports "not visible"; that is not
        // occlusion, just the pre-show state.
        if !window.shown {
            return;
        }
        let visible = window.occlusion_visible();
        eprintln!(
            "panes-host: window {id}: {}",
            if visible { "visible; presents resume" } else { "occluded; presents paused" }
        );
        window.set_occluded(!visible);
    });
}

/// The window landed on a different display (`windowDidChangeScreen:`): the
/// pinned streaming tick range must chase that panel's rate, or a window
/// dragged from a 60Hz external onto the 120Hz panel would stay capped at
/// 60 for its lifetime (and one dragged the other way would tick uselessly
/// fast).
pub fn window_screen_changed(id: WindowId) {
    with_app(|app| {
        if let Some(window) = app.windows.get_mut(&id) {
            window.refresh_stream_rate(app.mtm);
        }
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
        // Held keys must not outlive key status: AppKit stops sending
        // flagsChanged and keyUp after resign-key, so release them
        // guest-side before the deactivated Configure. Outside the with_app
        // borrow because the view sends protocol messages through app state.
        let view = with_app(|app| app.windows.get(&id).map(PaneWindow::view_handle)).flatten();
        if let Some(view) = view {
            view.release_held_keys();
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
        // Key-window changes gate the cursor capture: resign releases it,
        // become re-engages it when the guest still holds the lock.
        sync_capture(app);
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
        // The exit below skips Drop; give the cursor back explicitly.
        app.capture = None;
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

define_class!(
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PanesAppDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidResignActive:))]
        fn application_did_resign_active(&self, _notification: &NSNotification) {
            // Belt and braces on top of windowDidResignKey: whatever order
            // AppKit resigns things in, deactivation always ends with the
            // cursor released.
            with_app(sync_capture);
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn application_did_become_active(&self, _notification: &NSNotification) {
            with_app(sync_capture);
        }

        #[unsafe(method(applicationDidChangeScreenParameters:))]
        fn application_did_change_screen_parameters(&self, _notification: &NSNotification) {
            screens_changed();
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { objc2::msg_send![super(this), init] }
    }
}
