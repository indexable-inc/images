//! Interactive driver for a macOS guest: boot it off-screen, then read newline
//! commands from stdin and apply them (synthetic keyboard input + framebuffer
//! screenshots). A caller drives the guest UI (Setup Assistant, launching an
//! app) and verifies rendering without ever touching the host cursor or desktop.
//!
//! Commands (one per line; blank lines and `#` comments are ignored):
//!
//! - `key <name> [count]` press a named key `count` times (default 1)
//! - `down <name>` / `up <name>` hold / release a key (e.g. a modifier)
//! - `type <text>` type the rest of the line as characters
//! - `click <fx> <fy>` left-click at fraction `(fx, fy)` of the display (top-left)
//! - `move <fx> <fy>` move the pointer there without clicking (hover)
//! - `scroll <fx> <fy> <dy_px> [count]` move the pointer there, then post `count`
//!   vertical pixel-scroll events of `dy_px` (drives a scroll-drag)
//! - `cursor` report the last pointer fraction set by `click`/`move`
//! - `size` report the captured framebuffer size in pixels (`ok size <w> <h>`)
//! - `cursor-show <on|off>` draw a marker at the pointer in subsequent `shot`s
//! - `wait <seconds>` sleep (fractional seconds allowed)
//! - `shot <path>` screenshot the guest framebuffer to a PNG
//! - `quit` stop the VM and exit
//!
//! Each command prints a one-line `ok ...` / `err ...` acknowledgement to stdout
//! (flushed), so a caller can drive the guest in lockstep. Names accepted by
//! `key`/`down`/`up` are in [`crate::input::named_key`]. `click`/`move`
//! fractions are clamped-checked to `0..=1`; the pointer is absolute, so the
//! reported `cursor` is the last fraction this driver set, not a guest read-back.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use dashboard_core::{Pane, Publisher, socket_path};
use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSEventType};
use objc2_virtualization::VZVirtualMachineView;

use crate::imp::Error;
use crate::input;
use crate::macguest::{
    DirShare, bgra_to_rgba, capture, downscale_rgba, encode_png_rgba,
    frame_size as guest_frame_size, read_frame_bgra, start_guest_offscreen,
};

/// Gap between a key's down and up, and after its release, so the guest HID
/// stack registers discrete presses instead of coalescing them. The sleeps run
/// on the driver thread, leaving the main queue free to render between events.
const KEY_GAP_DOWN: Duration = Duration::from_millis(8);
const KEY_GAP_AFTER: Duration = Duration::from_millis(24);

/// Pointer-glide tuning: a `move`/`click` sweeps the pointer from its last
/// position to the target through intermediate `mouseMoved` events instead of
/// teleporting (see [`glide`]). `GLIDE_STEP` is the travel (in display fractions)
/// per intermediate step; `GLIDE_GAP` paces them on the driver thread (~60 fps)
/// so the guest renders the motion; the step count is clamped so a long sweep
/// stays smooth and a short one stays quick. A hop under `GLIDE_MIN` just jumps.
///
/// The cap bounds the added latency before a `move`/`click` acks: a full-screen
/// sweep is `GLIDE_MAX_STEPS * GLIDE_GAP` ≈ 384 ms (most moves are far shorter).
const GLIDE_MIN: f64 = 0.012;
const GLIDE_STEP: f64 = 0.03;
const GLIDE_MIN_STEPS: usize = 4;
const GLIDE_MAX_STEPS: usize = 24;
const GLIDE_GAP: Duration = Duration::from_millis(16);

/// Parameters for the interactive driver.
pub struct DriveMacos {
    pub bundle: PathBuf,
    pub shares: Vec<DirShare>,
}

pub fn drive_macos(drive: DriveMacos) -> Result<(), Error> {
    let DriveMacos { bundle, shares } = drive;
    let mtm = MainThreadMarker::new().ok_or(Error::NotMainThread)?;
    let view = start_guest_offscreen(mtm, &bundle, &shares)?;
    drive_view(mtm, view, "macOS guest");
    Ok(())
}

/// Hand an already-started off-screen guest view to the stdin command loop and
/// run the `AppKit` run loop until `quit`/EOF exits the process. Guest-agnostic:
/// shared by the macOS ([`drive_macos`]) and Linux
/// ([`crate::linuxguest::drive_linux`]) drivers, since every command operates on
/// the `VZVirtualMachineView` regardless of guest type. `label` titles the live
/// screen pane this session publishes to the dashboard.
pub(crate) fn drive_view(
    mtm: MainThreadMarker,
    view: Retained<VZVirtualMachineView>,
    label: &'static str,
) {
    // Leak the view (it lives for the process) and hand the raw pointer to the
    // driver thread; the view is not `Send`, so we only re-borrow it on the main
    // queue, where it is valid.
    let view_ptr = Retained::into_raw(view) as usize;
    eprintln!(
        "macos-vm: driving guest; stdin commands: \
         key/down/up/type/click/move/scroll/cursor/size/cursor-show/wait/shot/quit"
    );
    // Stream the guest screen to the local dashboard so the session shows up on
    // the canvas automatically, the same way a terminal producer does.
    spawn_screen_publisher(view_ptr, label);
    std::thread::spawn(move || run_commands(view_ptr));

    // The view needs the `AppKit` run loop to build its layer tree and receive
    // guest frames; the driver thread exits the process on `quit` or EOF.
    NSApplication::sharedApplication(mtm).run();
}

/// How often the dashboard producer samples the guest framebuffer. ~1 fps: a
/// remote-desktop thumbnail does not need to be smooth, and an unchanged frame is
/// skipped anyway, so an idle desktop costs one published frame and then nothing.
const SCREEN_PUBLISH_INTERVAL: Duration = Duration::from_millis(1000);
/// Cap the published frame's width (aspect ratio preserved). A dashboard card is
/// far smaller than a 1920px scanout, and the smaller PNG keeps the stream light.
const SCREEN_MAX_WIDTH: usize = 900;
/// Set this to disable publishing the guest screen to the dashboard (e.g. a
/// lockstep automated driver that does not want the extra main-queue sampling).
/// Any value disables it.
const NO_DASHBOARD_ENV: &str = "IX_MACVM_NO_DASHBOARD";

/// Publish the guest framebuffer to the local dashboard as a live image pane, so
/// a `drive` session appears on the canvas automatically alongside terminals and
/// other producers.
///
/// Best-effort: a runtime/bind failure disables the pane and never touches the
/// VM. The work runs on its own thread because this binary drives an `AppKit`
/// run loop, not an async one; that thread owns a small tokio runtime (for the
/// publisher's socket) and the [`Publisher`], both kept alive for the process by
/// the never-returning sample loop.
///
/// Each tick reads the raw frame on the main queue (the only main-thread work),
/// then converts, downscales, and compares it off the queue: an unchanged frame
/// is dropped before the expensive PNG encode and before any publish, so a static
/// desktop adds no traffic and no aggregator state.
fn spawn_screen_publisher(view_ptr: usize, label: &'static str) {
    if std::env::var_os(NO_DASHBOARD_ENV).is_some() {
        return;
    }
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("macos-vm: dashboard screen disabled (runtime: {error})");
                return;
            }
        };
        let publisher = match Publisher::bind(socket_path(), runtime.handle()) {
            Ok(publisher) => publisher,
            Err(error) => {
                eprintln!("macos-vm: dashboard screen disabled ({error})");
                return;
            }
        };
        eprintln!(
            "macos-vm: publishing guest screen to dashboard ({})",
            publisher.path().display()
        );
        let sink = publisher.sink();
        // Log a persistent read/encode fault once rather than every tick.
        let mut warned_read = false;
        let mut warned_encode = false;
        // The last published downscaled RGBA frame, to skip a static screen.
        let mut last_frame: Option<Vec<u8>> = None;
        loop {
            std::thread::sleep(SCREEN_PUBLISH_INTERVAL);
            // Main-queue work: copy the raw BGRA frame out.
            let (mut bgra, width, height) = match read_bgra_on_main(view_ptr) {
                Ok(frame) => frame,
                // The guest has not rendered a frame yet; the pane appears once it
                // does. Any other read error is logged once, then tolerated.
                Err(Error::NoFramebuffer) => continue,
                Err(error) => {
                    if !warned_read {
                        eprintln!("macos-vm: dashboard screen capture: {error}");
                        warned_read = true;
                    }
                    continue;
                }
            };
            // Off-queue work: convert, downscale, and skip an unchanged frame.
            bgra_to_rgba(&mut bgra);
            let (small, w, h) = match downscale_rgba(bgra, width, height, SCREEN_MAX_WIDTH) {
                Ok(frame) => frame,
                Err(_) => continue,
            };
            if last_frame.as_deref() == Some(small.as_slice()) {
                continue;
            }
            let png = match encode_png_rgba(&small, w as usize, h as usize) {
                Ok(png) => png,
                Err(error) => {
                    if !warned_encode {
                        eprintln!("macos-vm: dashboard screen encode: {error}");
                        warned_encode = true;
                    }
                    continue;
                }
            };
            last_frame = Some(small);
            let mut data_url = String::from("data:image/png;base64,");
            base64::engine::general_purpose::STANDARD.encode_string(&png, &mut data_url);
            let mut pane = Pane::html("screen", label, screen_html(&data_url));
            pane.subtitle = format!("{w}x{h}");
            sink.publish(std::slice::from_ref(&pane));
        }
        // `runtime` and `publisher` are owned here and kept alive by the loop
        // above; the process exits via `quit`/EOF, never by leaving this thread.
    });
}

/// The HTML body for the live screen pane: the captured frame as a data-URL
/// image, scaled to the card width over a black field.
fn screen_html(data_url: &str) -> String {
    format!(
        "<div style=\"margin:0;background:#000;line-height:0\">\
         <img src=\"{data_url}\" alt=\"guest screen\" \
         style=\"display:block;width:100%;height:auto;image-rendering:auto\"/></div>"
    )
}

/// Copy the guest framebuffer (raw BGRA) on the main queue (where the view and
/// its `IOSurface` are valid), mirroring [`shot`] and [`frame_size`]. The
/// BGRA→RGBA conversion is left to the caller so it runs off the main queue.
fn read_bgra_on_main(view_ptr: usize) -> Result<(Vec<u8>, usize, usize), Error> {
    let mut result = Err(Error::NoFramebuffer);
    DispatchQueue::main().exec_sync(|| {
        let view: &VZVirtualMachineView = unsafe { &*(view_ptr as *const VZVirtualMachineView) };
        result = read_frame_bgra(view);
    });
    result
}

/// Mutable driver state carried across commands: the last pointer position this
/// driver set (fractions, top-left), and whether `shot` draws a marker there.
/// The pointer is absolute, so this is the source of truth for `cursor`; the
/// guest is never queried back.
#[derive(Default)]
struct State {
    last_cursor: Option<(f64, f64)>,
    show_cursor: bool,
}

/// Read and execute commands until stdin closes or `quit` exits the process.
fn run_commands(view_ptr: usize) -> ! {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut state = State::default();
    // `lines()` yields each line without its trailing newline; trailing spaces
    // are preserved so `type` can emit them.
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let ack = execute(view_ptr, trimmed, &mut state);
        let _ = writeln!(stdout, "{ack}");
        let _ = stdout.flush();
    }
    std::process::exit(0);
}

/// Parse a `<fx> <fy>` pair of display fractions, validating both are finite and
/// within `0..=1` (the pointer is absolute, so an out-of-range fraction would
/// land off-screen and silently no-op).
fn parse_fraction_pair(
    parts: &mut std::str::SplitWhitespace<'_>,
    verb: &str,
) -> Result<(f64, f64), String> {
    let (Some(fx), Some(fy)) = (parts.next(), parts.next()) else {
        return Err(format!("err {verb} needs two fractions 0..1 (from top-left)"));
    };
    let (Ok(fx), Ok(fy)) = (fx.parse::<f64>(), fy.parse::<f64>()) else {
        return Err(format!("err {verb} fractions must be numbers 0..1 (from top-left)"));
    };
    let in_range = |f: f64| f.is_finite() && (0.0..=1.0).contains(&f);
    if !in_range(fx) || !in_range(fy) {
        return Err(format!("err {verb} fractions must be within 0..1 (from top-left)"));
    }
    Ok((fx, fy))
}

/// Apply a pointer command at display fractions `(fx, fy)` and record the new
/// pointer position. Shared by the `click` and `move` verbs: with `do_click` the
/// pointer is pressed there, otherwise it only moves (hover).
fn pointer_action(view_ptr: usize, fx: f64, fy: f64, do_click: bool, state: &mut State) {
    // Glide the pointer from its last position to the target rather than
    // teleporting: a real pointer sweeps across the screen, and the trail of
    // intermediate moves reliably drives motion-sensitive UI a single jump can
    // miss (a boundary-crossing hover, velocity, and especially drags). The
    // endpoint is set exactly below, so this only shapes the path taken there.
    glide(view_ptr, state.last_cursor, (fx, fy));
    on_main(view_ptr, move |view| {
        let (x, y) = view_point(view, fx, fy);
        if do_click {
            input::click(view, x, y);
        } else {
            input::mouse_move(view, x, y);
        }
    });
    state.last_cursor = Some((fx, fy));
    std::thread::sleep(KEY_GAP_AFTER);
}

/// Sweep the pointer from `start` to `end` (display fractions, top-left) with a
/// short series of eased `mouseMoved` events, so the motion looks human and
/// touches every point in between. Does nothing without a known `start` (the
/// first pointer command of a session simply teleports) or for a hop under
/// [`GLIDE_MIN`] (already effectively on target). The endpoint is left to the
/// caller, which places the pointer there exactly (and may click).
fn glide(view_ptr: usize, start: Option<(f64, f64)>, end: (f64, f64)) {
    let Some((sx, sy)) = start else {
        return;
    };
    let (dx, dy) = (end.0 - sx, end.1 - sy);
    let dist = dx.hypot(dy);
    if dist < GLIDE_MIN {
        return;
    }
    // One step per `GLIDE_STEP` of travel, clamped to the smooth/quick range.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = ((dist / GLIDE_STEP).ceil() as usize).clamp(GLIDE_MIN_STEPS, GLIDE_MAX_STEPS);
    for i in 1..steps {
        #[allow(clippy::cast_precision_loss)]
        let t = i as f64 / steps as f64;
        // Smoothstep ease-in-out: accelerate off the start, decelerate into the
        // target, like a hand moving a pointer (`3 - 2t` via mul_add).
        let e = t * t * 2.0f64.mul_add(-t, 3.0);
        let (fx, fy) = (dx.mul_add(e, sx), dy.mul_add(e, sy));
        on_main(view_ptr, move |view| {
            let (x, y) = view_point(view, fx, fy);
            input::mouse_move(view, x, y);
        });
        std::thread::sleep(GLIDE_GAP);
    }
}

/// Execute one command line, returning its acknowledgement.
fn execute(view_ptr: usize, trimmed: &str, state: &mut State) -> String {
    if let Some(text) = trimmed.strip_prefix("type ") {
        let (typed, skipped) = type_text(view_ptr, text);
        return if skipped == 0 {
            format!("ok type {typed} chars")
        } else {
            format!("ok type {typed} chars ({skipped} unmapped)")
        };
    }
    if let Some(path) = trimmed.strip_prefix("shot ") {
        let path = Path::new(path.trim());
        // Draw the pointer marker only when asked, so the default `shot` stays a
        // faithful capture of the guest framebuffer (existing callers unchanged).
        let cursor = state.show_cursor.then_some(state.last_cursor).flatten();
        return match shot(view_ptr, path, cursor) {
            Ok(bytes) => format!("ok shot {} {bytes}", path.display()),
            Err(error) => format!("err shot {error}"),
        };
    }

    let mut parts = trimmed.split_whitespace();
    let Some(command) = parts.next() else {
        return "ok".to_owned();
    };
    match command {
        "click" | "move" => {
            let (fx, fy) = match parse_fraction_pair(&mut parts, command) {
                Ok(pair) => pair,
                Err(message) => return message,
            };
            pointer_action(view_ptr, fx, fy, command == "click", state);
            format!("ok {command} {fx} {fy}")
        }
        "scroll" => {
            // scroll <fx> <fy> <dy_px> [count]: move the pointer over the target
            // (the absolute device routes scroll to whatever is under it), then post
            // `count` vertical pixel-scroll events of `dy_px`. Drives a scroll-drag.
            let (fx, fy) = match parse_fraction_pair(&mut parts, "scroll") {
                Ok(pair) => pair,
                Err(message) => return message,
            };
            let Some(Ok(dy)) = parts.next().map(str::parse::<f64>) else {
                return "err scroll needs <fx> <fy> <dy_px> [count]".to_owned();
            };
            let count: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            pointer_action(view_ptr, fx, fy, false, state);
            for _ in 0..count {
                on_main(view_ptr, move |view| input::scroll(view, 0.0, dy));
                std::thread::sleep(KEY_GAP_AFTER);
            }
            format!("ok scroll {fx} {fy} {dy} x{count}")
        }
        "cursor" => match state.last_cursor {
            Some((fx, fy)) => format!("ok cursor {fx} {fy}"),
            None => "err cursor unknown (no click/move yet)".to_owned(),
        },
        "size" => match frame_size(view_ptr) {
            Some((w, h)) => format!("ok size {w} {h}"),
            None => "err size guest framebuffer not available yet".to_owned(),
        },
        "cursor-show" => match parts.next() {
            Some("on") => {
                state.show_cursor = true;
                "ok cursor-show on".to_owned()
            }
            Some("off") => {
                state.show_cursor = false;
                "ok cursor-show off".to_owned()
            }
            _ => "err cursor-show needs on|off".to_owned(),
        },
        "key" => {
            let Some(name) = parts.next() else {
                return "err key needs a name".to_owned();
            };
            let Some(code) = input::named_key(name) else {
                return format!("err unknown key {name:?}");
            };
            let count = parts.next().map_or(1, |c| c.parse::<u32>().unwrap_or(1));
            for _ in 0..count {
                press_key(view_ptr, code, false);
            }
            format!("ok key {name} x{count}")
        }
        "down" | "up" => {
            let Some(name) = parts.next() else {
                return format!("err {command} needs a name");
            };
            let Some(code) = input::named_key(name) else {
                return format!("err unknown key {name:?}");
            };
            let event_type = if command == "down" {
                NSEventType::KeyDown
            } else {
                NSEventType::KeyUp
            };
            on_main(view_ptr, move |view| {
                input::send_event(view, event_type, code)
            });
            std::thread::sleep(KEY_GAP_AFTER);
            format!("ok {command} {name}")
        }
        "wait" => {
            // Reject non-positive / non-finite rather than silently acking `ok`
            // without sleeping, which would desync a lockstep caller.
            let secs = match parts.next().map(str::parse::<f64>) {
                Some(Ok(secs)) if secs.is_finite() && secs > 0.0 => secs,
                _ => return "err wait needs a positive, finite number of seconds".to_owned(),
            };
            std::thread::sleep(Duration::from_secs_f64(secs));
            format!("ok wait {secs}")
        }
        "quit" => {
            std::process::exit(0);
        }
        other => format!("err unknown command {other:?}"),
    }
}

/// Type a string, returning `(typed, unmapped)` character counts.
fn type_text(view_ptr: usize, text: &str) -> (usize, usize) {
    let mut typed = 0;
    let mut unmapped = 0;
    for c in text.chars() {
        match input::char_to_stroke(c) {
            Some(stroke) => {
                press_key(view_ptr, stroke.code, stroke.shift);
                typed += 1;
            }
            None => unmapped += 1,
        }
    }
    (typed, unmapped)
}

/// Press a key (down then up), holding Shift around it when requested.
fn press_key(view_ptr: usize, code: u16, shift: bool) {
    on_main(view_ptr, move |view| {
        if shift {
            input::send_event(view, NSEventType::KeyDown, input::SHIFT);
        }
        input::send_event(view, NSEventType::KeyDown, code);
    });
    std::thread::sleep(KEY_GAP_DOWN);
    on_main(view_ptr, move |view| {
        input::send_event(view, NSEventType::KeyUp, code);
        if shift {
            input::send_event(view, NSEventType::KeyUp, input::SHIFT);
        }
    });
    std::thread::sleep(KEY_GAP_AFTER);
}

/// Screenshot the guest framebuffer on the main queue and return bytes written.
/// `cursor` (display fractions, top-left) draws a pointer marker into the PNG
/// when set, so a caller can see where the driver's pointer is.
fn shot(view_ptr: usize, path: &Path, cursor: Option<(f64, f64)>) -> Result<usize, Error> {
    let mut result: Result<usize, Error> = Err(Error::NoFramebuffer);
    // `exec_sync` blocks until the block finishes, so borrowing `result` and
    // `path` across the queue boundary is sound (no `'static` needed).
    DispatchQueue::main().exec_sync(|| {
        let view: &VZVirtualMachineView = unsafe { &*(view_ptr as *const VZVirtualMachineView) };
        result = capture(view, path, cursor);
    });
    result
}

/// The captured framebuffer size in pixels, read on the main queue. `None` until
/// the guest has rendered a frame.
fn frame_size(view_ptr: usize) -> Option<(usize, usize)> {
    let mut size = None;
    DispatchQueue::main().exec_sync(|| {
        let view: &VZVirtualMachineView = unsafe { &*(view_ptr as *const VZVirtualMachineView) };
        size = guest_frame_size(view);
    });
    size
}

/// Map display fractions `(fx, fy)` (top-left origin, `0..=1`) to a point in the
/// view's `AppKit` window coordinates (bottom-left origin). Must run on the main
/// thread (touches the view).
fn view_point(view: &VZVirtualMachineView, fx: f64, fy: f64) -> (f64, f64) {
    let bounds = view.bounds();
    let x = fx * bounds.size.width;
    // Fraction is from the top; AppKit window coordinates are bottom-left origin,
    // so flip y.
    let y = fy.mul_add(-bounds.size.height, bounds.size.height);
    (x, y)
}

/// Run `f` on the main queue with a re-borrow of the leaked view, blocking until
/// it returns. `AppKit` and the VM view must be touched only on the main thread.
fn on_main(view_ptr: usize, f: impl FnOnce(&VZVirtualMachineView) + Send) {
    DispatchQueue::main().exec_sync(move || {
        let view: &VZVirtualMachineView = unsafe { &*(view_ptr as *const VZVirtualMachineView) };
        f(view);
    });
}
