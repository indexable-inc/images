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
//! - `wait <seconds>` sleep (fractional seconds allowed)
//! - `shot <path>` screenshot the guest framebuffer to a PNG
//! - `quit` stop the VM and exit
//!
//! Each command prints a one-line `ok ...` / `err ...` acknowledgement to stdout
//! (flushed), so a caller can drive the guest in lockstep. Names accepted by
//! `key`/`down`/`up` are in [`crate::input::named_key`].

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSEventType};
use objc2_virtualization::VZVirtualMachineView;

use crate::imp::Error;
use crate::input;
use crate::macguest::{DirShare, capture, start_guest_offscreen};

/// Gap between a key's down and up, and after its release, so the guest HID
/// stack registers discrete presses instead of coalescing them. The sleeps run
/// on the driver thread, leaving the main queue free to render between events.
const KEY_GAP_DOWN: Duration = Duration::from_millis(8);
const KEY_GAP_AFTER: Duration = Duration::from_millis(24);

/// Parameters for the interactive driver.
pub struct DriveMacos {
    pub bundle: PathBuf,
    pub shares: Vec<DirShare>,
}

pub fn drive_macos(drive: DriveMacos) -> Result<(), Error> {
    let DriveMacos { bundle, shares } = drive;
    let mtm = MainThreadMarker::new().ok_or(Error::NotMainThread)?;
    let view = start_guest_offscreen(mtm, &bundle, &shares)?;
    // Leak the view (it lives for the process) and hand the raw pointer to the
    // driver thread; the view is not `Send`, so we only re-borrow it on the main
    // queue, where it is valid.
    let view_ptr = Retained::into_raw(view) as usize;
    eprintln!("macos-vm: driving guest; stdin commands: key/down/up/type/wait/shot/quit");
    std::thread::spawn(move || run_commands(view_ptr));

    // The view needs the `AppKit` run loop to build its layer tree and receive
    // guest frames; the driver thread exits the process on `quit` or EOF.
    NSApplication::sharedApplication(mtm).run();
    Ok(())
}

/// Read and execute commands until stdin closes or `quit` exits the process.
fn run_commands(view_ptr: usize) -> ! {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    // `lines()` yields each line without its trailing newline; trailing spaces
    // are preserved so `type` can emit them.
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let ack = execute(view_ptr, trimmed);
        let _ = writeln!(stdout, "{ack}");
        let _ = stdout.flush();
    }
    std::process::exit(0);
}

/// Execute one command line, returning its acknowledgement.
fn execute(view_ptr: usize, trimmed: &str) -> String {
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
        return match shot(view_ptr, path) {
            Ok(bytes) => format!("ok shot {} {bytes}", path.display()),
            Err(error) => format!("err shot {error}"),
        };
    }

    let mut parts = trimmed.split_whitespace();
    let Some(command) = parts.next() else {
        return "ok".to_owned();
    };
    match command {
        "click" => {
            let fx = parts.next().and_then(|s| s.parse::<f64>().ok());
            let fy = parts.next().and_then(|s| s.parse::<f64>().ok());
            let (Some(fx), Some(fy)) = (fx, fy) else {
                return "err click needs two fractions 0..1 (from top-left)".to_owned();
            };
            on_main(view_ptr, move |view| {
                let bounds = view.bounds();
                let x = fx * bounds.size.width;
                // Fraction is from the top; AppKit window coordinates are
                // bottom-left origin, so flip y.
                let y = fy.mul_add(-bounds.size.height, bounds.size.height);
                input::click(view, x, y);
            });
            std::thread::sleep(KEY_GAP_AFTER);
            format!("ok click {fx} {fy}")
        }
        "key" => {
            let Some(name) = parts.next() else {
                return "err key needs a name".to_owned();
            };
            let Some(code) = input::named_key(name) else {
                return format!("err unknown key {name:?}");
            };
            let count = parts.next().and_then(|c| c.parse::<u32>().ok()).unwrap_or(1);
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
            on_main(view_ptr, move |view| input::send_event(view, event_type, code));
            std::thread::sleep(KEY_GAP_AFTER);
            format!("ok {command} {name}")
        }
        "wait" => {
            let Some(secs) = parts.next().and_then(|s| s.parse::<f64>().ok()) else {
                return "err wait needs seconds".to_owned();
            };
            if secs.is_finite() && secs > 0.0 {
                std::thread::sleep(Duration::from_secs_f64(secs));
            }
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
fn shot(view_ptr: usize, path: &Path) -> Result<usize, Error> {
    let mut result: Result<usize, Error> = Err(Error::NoFramebuffer);
    // `exec_sync` blocks until the block finishes, so borrowing `result` and
    // `path` across the queue boundary is sound (no `'static` needed).
    DispatchQueue::main().exec_sync(|| {
        let view: &VZVirtualMachineView = unsafe { &*(view_ptr as *const VZVirtualMachineView) };
        result = capture(view, path);
    });
    result
}

/// Run `f` on the main queue with a re-borrow of the leaked view, blocking until
/// it returns. `AppKit` and the VM view must be touched only on the main thread.
fn on_main(view_ptr: usize, f: impl FnOnce(&VZVirtualMachineView) + Send) {
    DispatchQueue::main().exec_sync(move || {
        let view: &VZVirtualMachineView = unsafe { &*(view_ptr as *const VZVirtualMachineView) };
        f(view);
    });
}
