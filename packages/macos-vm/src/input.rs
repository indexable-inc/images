//! Synthetic keyboard input to a macOS guest with no host event system and no
//! host cursor: we build `NSEvent` key events and hand them straight to the
//! `VZVirtualMachineView`, which forwards them to the guest's USB keyboard
//! device. Modifiers are sent as their own key events (we never set `AppKit`
//! modifier flags); the guest keyboard device reads the HID key reports. Key-code
//! map and technique from github.com/thecrypticace/vzautomation.

use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::{NSPoint, NSString};
use objc2_virtualization::VZVirtualMachineView;

/// `kVK_Shift`: held around a keystroke to type its shifted character.
pub const SHIFT: u16 = 0x38;

/// One key in a keystroke: a Carbon virtual key code and whether Shift is held.
#[derive(Clone, Copy)]
pub struct KeyStroke {
    pub code: u16,
    pub shift: bool,
}

/// Send a single key-down or key-up to the guest. Must run on the main thread
/// (the VM view's thread). A null event (`AppKit` declined to build one) is a
/// no-op rather than a panic.
pub fn send_event(view: &VZVirtualMachineView, event_type: NSEventType, keycode: u16) {
    let empty = NSString::from_str("");
    let window_number = view.window().map_or(0, |window| window.windowNumber());
    let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        event_type,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        window_number,
        None,
        &empty,
        &empty,
        false,
        keycode,
    );
    let Some(event) = event else { return };
    if event_type == NSEventType::KeyDown {
        view.keyDown(&event);
    } else {
        view.keyUp(&event);
    }
}

/// Click the guest at view point `(x, y)` in `AppKit` window coordinates
/// (bottom-left origin). Sends a left mouse down then up; the screen-coordinate
/// pointing device maps the location to the guest's absolute cursor, so only the
/// guest cursor moves. Must run on the main thread.
pub fn click(view: &VZVirtualMachineView, x: f64, y: f64) {
    send_mouse(view, NSEventType::LeftMouseDown, x, y, 1.0);
    send_mouse(view, NSEventType::LeftMouseUp, x, y, 0.0);
}

fn send_mouse(view: &VZVirtualMachineView, event_type: NSEventType, x: f64, y: f64, pressure: f32) {
    let window_number = view.window().map_or(0, |window| window.windowNumber());
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        event_type,
        NSPoint::new(x, y),
        NSEventModifierFlags::empty(),
        0.0,
        window_number,
        None,
        0,
        1,
        pressure,
    );
    let Some(event) = event else { return };
    if event_type == NSEventType::LeftMouseDown {
        view.mouseDown(&event);
    } else {
        view.mouseUp(&event);
    }
}

/// Carbon virtual key code for a named key (navigation, editing, modifiers).
/// Modifiers are returned too so the driver can hold them with `down`/`up`.
pub fn named_key(name: &str) -> Option<u16> {
    let code = match name {
        "return" | "enter" => 0x24,
        "tab" => 0x30,
        "space" => 0x31,
        "delete" | "backspace" => 0x33,
        "escape" | "esc" => 0x35,
        "forward-delete" => 0x75,
        "up" => 0x7E,
        "down" => 0x7D,
        "left" => 0x7B,
        "right" => 0x7C,
        "home" => 0x73,
        "end" => 0x77,
        "pageup" => 0x74,
        "pagedown" => 0x79,
        "f1" => 0x7A,
        "f2" => 0x78,
        "f3" => 0x63,
        "f4" => 0x76,
        "f5" => 0x60,
        "f6" => 0x61,
        "f7" => 0x62,
        "f8" => 0x64,
        "f9" => 0x65,
        "f10" => 0x6D,
        "f11" => 0x67,
        "f12" => 0x6F,
        // Modifiers, held via `down <mod>` / released via `up <mod>`.
        "command" | "cmd" => 0x37,
        "shift" => 0x38,
        "option" | "alt" => 0x3A,
        "control" | "ctrl" => 0x3B,
        "function" | "fn" => 0x3F,
        _ => return None,
    };
    Some(code)
}

/// Key code for an ASCII letter `a`..=`z`.
const fn letter_code(lower: char) -> Option<u16> {
    let code = match lower {
        'a' => 0x00,
        'b' => 0x0B,
        'c' => 0x08,
        'd' => 0x02,
        'e' => 0x0E,
        'f' => 0x03,
        'g' => 0x05,
        'h' => 0x04,
        'i' => 0x22,
        'j' => 0x26,
        'k' => 0x28,
        'l' => 0x25,
        'm' => 0x2E,
        'n' => 0x2D,
        'o' => 0x1F,
        'p' => 0x23,
        'q' => 0x0C,
        'r' => 0x0F,
        's' => 0x01,
        't' => 0x11,
        'u' => 0x20,
        'v' => 0x09,
        'w' => 0x0D,
        'x' => 0x07,
        'y' => 0x10,
        'z' => 0x06,
        _ => return None,
    };
    Some(code)
}

/// The keystroke that types a single character, or `None` if unmapped. Covers
/// printable ASCII (letters, digits, and the US-layout symbols), enough to type
/// usernames, passwords, and shell commands.
pub fn char_to_stroke(c: char) -> Option<KeyStroke> {
    if c.is_ascii_alphabetic() {
        let code = letter_code(c.to_ascii_lowercase())?;
        return Some(KeyStroke { code, shift: c.is_ascii_uppercase() });
    }
    let (code, shift) = match c {
        '1' => (0x12, false),
        '!' => (0x12, true),
        '2' => (0x13, false),
        '@' => (0x13, true),
        '3' => (0x14, false),
        '#' => (0x14, true),
        '4' => (0x15, false),
        '$' => (0x15, true),
        '5' => (0x17, false),
        '%' => (0x17, true),
        '6' => (0x16, false),
        '^' => (0x16, true),
        '7' => (0x1A, false),
        '&' => (0x1A, true),
        '8' => (0x1C, false),
        '*' => (0x1C, true),
        '9' => (0x19, false),
        '(' => (0x19, true),
        '0' => (0x1D, false),
        ')' => (0x1D, true),
        ' ' => (0x31, false),
        '\t' => (0x30, false),
        '\n' => (0x24, false),
        '-' => (0x1B, false),
        '_' => (0x1B, true),
        '=' => (0x18, false),
        '+' => (0x18, true),
        '[' => (0x21, false),
        '{' => (0x21, true),
        ']' => (0x1E, false),
        '}' => (0x1E, true),
        '\\' => (0x2A, false),
        '|' => (0x2A, true),
        ';' => (0x29, false),
        ':' => (0x29, true),
        '\'' => (0x27, false),
        '"' => (0x27, true),
        ',' => (0x2B, false),
        '<' => (0x2B, true),
        '.' => (0x2F, false),
        '>' => (0x2F, true),
        '/' => (0x2C, false),
        '?' => (0x2C, true),
        '`' => (0x32, false),
        '~' => (0x32, true),
        _ => return None,
    };
    Some(KeyStroke { code, shift })
}
