//! The per-window content view: hosts the `CAMetalLayer` and translates every
//! `AppKit` input event into protocol messages in surface coordinates.
//!
//! Coordinates: the view overrides `isFlipped` so its local origin is
//! top-left like the guest surface; points are multiplied by the window's
//! `backingScaleFactor` because the protocol carries buffer-scale positions.
//!
//! Scroll signs: `scrollingDelta*` is positive when content should move
//! right/down (scroll wheel up), while Wayland's axis is positive for a
//! downward scroll, so both axes are negated on the way out.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSEventPhase, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect};
use panes_protocol::{AxisSource, ButtonState, ToGuest, WindowId};

use crate::app;
use crate::keymap;

/// evdev button codes (input-event-codes.h), what `wl_pointer.button` carries.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const BTN_SIDE: u32 = 0x113;
const BTN_EXTRA: u32 = 0x114;

/// kVK codes for the host-local Cmd shortcuts and the caps-lock special case.
const KVK_ANSI_Q: u16 = 0x0C;
const KVK_ANSI_W: u16 = 0x0D;
const KVK_CAPS_LOCK: u16 = 0x39;

/// kVK codes for the Cmd editing chords translated to their Linux
/// equivalents (`HIToolbox` Events.h), plus the modifier keys themselves.
const KVK_ANSI_A: u16 = 0x00;
const KVK_ANSI_Z: u16 = 0x06;
const KVK_ANSI_X: u16 = 0x07;
const KVK_ANSI_C: u16 = 0x08;
const KVK_ANSI_V: u16 = 0x09;
const KVK_DELETE: u16 = 0x33; // backspace
const KVK_LEFT_ARROW: u16 = 0x7B;
const KVK_RIGHT_ARROW: u16 = 0x7C;
const KVK_SHIFT: u16 = 0x38;
const KVK_RIGHT_SHIFT: u16 = 0x3C;
const KVK_CONTROL: u16 = 0x3B;
const KVK_RIGHT_CONTROL: u16 = 0x3E;
const KVK_OPTION: u16 = 0x3A;
const KVK_RIGHT_OPTION: u16 = 0x3D;
const KVK_COMMAND: u16 = 0x37;
const KVK_RIGHT_COMMAND: u16 = 0x36;
const KVK_FUNCTION: u16 = 0x3F;

/// evdev keycodes (input-event-codes.h) for translation targets that have no
/// kVK on the Mac keyboard, plus the synthetic chord modifier.
const KEY_LEFTCTRL: u32 = 29;
const KEY_HOME: u32 = 102;
const KEY_END: u32 = 107;

/// One line per wheel click, in `wl_pointer` axis units. libinput's convention
/// (15 units per detent) so guest toolkits scroll the expected distance.
const WHEEL_AXIS_PER_LINE: f64 = 15.0;

/// What actually went out on the wire for one key press, so its release
/// mirrors the press exactly no matter which modifiers are down by then (a
/// user may release Cmd before or after the chorded key, and a translated
/// press must never be un-pressed as its untranslated self).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ForwardedKey {
    /// evdev keycode sent in the press.
    keycode: u32,
    /// The press was wrapped in a synthetic left-ctrl (Cmd chord
    /// translation); the release drops the ctrl once no other forwarded key
    /// still needs it.
    ctrl: bool,
    /// Pressed while Cmd was down: released defensively when Cmd goes up
    /// (`release_cmd_chords`), because `AppKit` swallows chorded keyUps at
    /// the `NSApplication` layer and the un-swallow monitor
    /// (`app::install_key_up_monitor`) is behavior we do not control.
    chorded: bool,
}

/// How one Cmd chord is presented to the guest (see [`translate_chord`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Translation {
    keycode: u32,
    ctrl: bool,
}

pub struct ViewIvars {
    id: WindowId,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    /// Modifier keys currently held, keyed by kVK. Press vs release is
    /// decided against the event's device-independent class flag (see
    /// `flags_changed`); membership exists to catch the release of a key
    /// whose class stays down (two Cmds held, one released) and to release
    /// everything on focus loss.
    held_modifiers: RefCell<HashSet<u16>>,
    /// Non-modifier keys whose press was forwarded, keyed by kVK, with what
    /// was forwarded. Jobs: releasing on focus loss (a keyUp after Cmd-Tab
    /// never reaches this view, so a key held across it would stay pressed
    /// guest-side and auto-repeat forever), gating keyUp forwarding to keys
    /// the guest actually saw pressed (the host-consumed Cmd-W/Q must not
    /// leak a stray release), and releasing exactly what the press sent
    /// (chord translation).
    held_keys: RefCell<HashMap<u16, ForwardedKey>>,
    /// Translate macOS editing chords to their Linux equivalents
    /// (`translate_chord`) instead of forwarding raw Super chords; from
    /// `app::RunOptions` (`--no-chord-translation` clears it).
    chord_translation: bool,
    /// The synthetic left-ctrl for translated chords is currently pressed
    /// guest-side. Explicit state, not inferred from `held_keys`: the guard
    /// against a physically-held ctrl (see `key_down`) means a ctrl-wrapped
    /// entry does not always imply we pressed one.
    synthetic_ctrl: Cell<bool>,
    /// Pointer capture engaged for this window (`app::sync_capture`): motion
    /// goes out as `PointerRelative` deltas, and the absolute re-anchor
    /// before buttons/scrolls is skipped (the frozen cursor position is
    /// meaningless while dissociated).
    relative: Cell<bool>,
    /// Identity (timestamp, eventNumber) of the last relative-forwarded
    /// event. `AppKit` delivers each mouseMoved TWICE here -- once to the
    /// first responder (`acceptsMouseMovedEvents`) and once to the tracking
    /// area's owner (`MouseMoved` option), the same view (measured live: 4
    /// posted moves, 8 arrivals). Absolute coordinates absorb the duplicate;
    /// summed deltas would double mouse-look sensitivity, so duplicates are
    /// dropped by identity.
    last_relative: Cell<(f64, isize)>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PanesHostView"]
    #[ivars = ViewIvars]
    pub struct PanesView;

    impl PanesView {
        // Top-left origin so view points map to surface coordinates directly.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        // The click that activates the window also reaches the guest, the
        // VM-window convention (Parallels/VMware do this): without it AppKit
        // swallows the first mouseDown on an inactive window, so the user's
        // first click into a guest app would do nothing.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            let ivars = self.ivars();
            if let Some(old) = ivars.tracking.borrow_mut().take() {
                self.removeTrackingArea(&old);
            }
            // InVisibleRect keeps the area glued to the view through resizes
            // so we never track a stale rect between updateTrackingAreas
            // calls (AppKit only invokes this lazily).
            let options = NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::ActiveInKeyWindow
                | NSTrackingAreaOptions::InVisibleRect;
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    self.bounds(),
                    options,
                    Some::<&AnyObject>(self.as_ref()),
                    None,
                )
            };
            self.addTrackingArea(&area);
            *ivars.tracking.borrow_mut() = Some(area);
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.send_motion(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.send_motion(event);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            self.send_motion(event);
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            self.send_motion(event);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            app::send(ToGuest::PointerLeave { id: self.ivars().id });
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.send_button(event, ButtonState::Pressed);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.send_button(event, ButtonState::Released);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.send_button(event, ButtonState::Pressed);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            self.send_button(event, ButtonState::Released);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            self.send_button(event, ButtonState::Pressed);
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            self.send_button(event, ButtonState::Released);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.send_scroll(event);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            // Guests auto-repeat themselves from wl_keyboard.repeat_info;
            // forwarding host repeats would double them.
            if event.isARepeat() {
                return;
            }
            let code = event.keyCode();
            let cmd = event.modifierFlags().contains(NSEventModifierFlags::Command);
            if cmd {
                // App-level shortcuts stay host-side, like any native app.
                match code {
                    KVK_ANSI_W => {
                        app::close_requested(self.ivars().id);
                        return;
                    }
                    KVK_ANSI_Q => {
                        app::request_quit();
                        return;
                    }
                    _ => {}
                }
            }
            let forwarded = if cmd && self.ivars().chord_translation {
                let Some(translation) = translate_chord(code) else {
                    // An unmapped Cmd chord does nothing, like a native app
                    // that lacks the shortcut; forwarding the bare key would
                    // type a stray character instead (Super itself is not
                    // forwarded in translation mode).
                    return;
                };
                ForwardedKey { keycode: translation.keycode, ctrl: translation.ctrl, chorded: true }
            } else {
                let Some(keycode) = keymap::evdev_from_kvk(code) else {
                    eprintln!("panes-host: no evdev mapping for kVK {code:#x}");
                    return;
                };
                ForwardedKey { keycode, ctrl: false, chorded: cmd }
            };
            // One synthetic ctrl serves however many translated chords are
            // down; engage it only for the first, and not at all while the
            // user physically holds ctrl (same evdev keycode: a second press
            // would later release ctrl out from under their real hold).
            if forwarded.ctrl && !self.ivars().synthetic_ctrl.get() && !self.real_ctrl_held() {
                self.ivars().synthetic_ctrl.set(true);
                self.send_keycode(KEY_LEFTCTRL, ButtonState::Pressed);
            }
            self.ivars().held_keys.borrow_mut().insert(code, forwarded);
            self.send_keycode(forwarded.keycode, ButtonState::Pressed);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            // Only keys whose press was forwarded: a release for a key the
            // guest never saw pressed (host-consumed shortcut, focus gained
            // mid-hold, chord already swept by release_cmd_chords) would be
            // noise.
            self.release_forwarded(event.keyCode());
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            let code = event.keyCode();
            // Caps lock reports one flagsChanged per toggle, not per
            // press/release; synthesize a full press so the guest's LED/state
            // machine advances.
            if code == KVK_CAPS_LOCK {
                self.send_key(code, ButtonState::Pressed);
                self.send_key(code, ButtonState::Released);
                return;
            }
            let state = {
                let mut held = self.ivars().held_modifiers.borrow_mut();
                let transition = modifier_class(code).map_or_else(
                    // Unmapped modifier key: membership toggling is the only
                    // signal left (no class flag to consult).
                    || {
                        Some(if held.contains(&code) {
                            ButtonState::Released
                        } else {
                            ButtonState::Pressed
                        })
                    },
                    |class| {
                        modifier_transition(
                            held.contains(&code),
                            event.modifierFlags().contains(class),
                        )
                    },
                );
                // None: release of a key pressed before this window had key
                // focus (Cmd-Tab back with the modifier still down); the
                // guest never saw the press. The old membership toggle
                // guessed "press" here, latching the modifier in the guest's
                // xkb state so every letter became a dead chord and text
                // input stopped until the next focus loss.
                let Some(state) = transition else {
                    eprintln!(
                        "panes-host: ignoring release of modifier kVK {code:#x} pressed \
                         before focus"
                    );
                    return;
                };
                match state {
                    ButtonState::Pressed => {
                        held.insert(code);
                    }
                    ButtonState::Released => {
                        held.remove(&code);
                    }
                }
                state
            };
            let cmd_key = code == KVK_COMMAND || code == KVK_RIGHT_COMMAND;
            // Cmd went fully up: defensively release everything pressed
            // inside the chord (see release_cmd_chords).
            if cmd_key && !event.modifierFlags().contains(NSEventModifierFlags::Command) {
                self.release_cmd_chords();
            }
            // In translation mode Super never reaches the guest: chords are
            // presented as their Linux equivalents, and a bare Super press
            // would turn them back into Super chords.
            if cmd_key && self.ivars().chord_translation {
                return;
            }
            self.send_key(code, state);
        }
    }
);

/// The device-independent `NSEventModifierFlags` class bit a modifier key
/// contributes to (`HIToolbox` Events.h kVK codes).
const fn modifier_class(kvk: u16) -> Option<NSEventModifierFlags> {
    match kvk {
        KVK_SHIFT | KVK_RIGHT_SHIFT => Some(NSEventModifierFlags::Shift),
        KVK_CONTROL | KVK_RIGHT_CONTROL => Some(NSEventModifierFlags::Control),
        KVK_OPTION | KVK_RIGHT_OPTION => Some(NSEventModifierFlags::Option),
        KVK_COMMAND | KVK_RIGHT_COMMAND => Some(NSEventModifierFlags::Command),
        KVK_FUNCTION => Some(NSEventModifierFlags::Function),
        _ => None,
    }
}

/// What a `flagsChanged` for one modifier key means, given whether we saw
/// its press (`held`) and whether its modifier class is down after the event
/// (`class_down`). `None` = forward nothing.
///
/// A key we saw press can only be releasing (a second press cannot arrive
/// without an intervening release, even with its sibling holding the class
/// flag down). A key we did not see press is a press when its class is down;
/// when the class is up it is the release of a press this window never saw
/// (modifier held across a focus change), which must be dropped, not
/// toggle-guessed into a press.
const fn modifier_transition(held: bool, class_down: bool) -> Option<ButtonState> {
    match (held, class_down) {
        (true, _) => Some(ButtonState::Released),
        (false, true) => Some(ButtonState::Pressed),
        (false, false) => None,
    }
}

/// macOS editing chords -> what a Linux app expects, so guest apps respond
/// to Mac muscle memory (on by default; `--no-chord-translation` restores
/// raw Super chords): Cmd+A/C/V/X/Z -> Ctrl+same, Cmd+Backspace ->
/// Ctrl+Backspace (delete word), Cmd+Left/Right -> Home/End. Shift rides
/// along natively (Cmd+Shift+Z -> Ctrl+Shift+Z).
fn translate_chord(kvk: u16) -> Option<Translation> {
    match kvk {
        KVK_ANSI_A | KVK_ANSI_C | KVK_ANSI_V | KVK_ANSI_X | KVK_ANSI_Z | KVK_DELETE => {
            Some(Translation { keycode: keymap::evdev_from_kvk(kvk)?, ctrl: true })
        }
        KVK_LEFT_ARROW => Some(Translation { keycode: KEY_HOME, ctrl: false }),
        KVK_RIGHT_ARROW => Some(Translation { keycode: KEY_END, ctrl: false }),
        _ => None,
    }
}

impl PanesView {
    pub fn new(
        mtm: MainThreadMarker,
        id: WindowId,
        frame: NSRect,
        chord_translation: bool,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            id,
            tracking: RefCell::new(None),
            held_modifiers: RefCell::new(HashSet::new()),
            held_keys: RefCell::new(HashMap::new()),
            chord_translation,
            synthetic_ctrl: Cell::new(false),
            relative: Cell::new(false),
            last_relative: Cell::new((f64::NEG_INFINITY, 0)),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Switch the motion path between absolute coordinates and relative
    /// deltas; owned by `app::sync_capture`, always in step with the cursor
    /// capture.
    pub fn set_relative(&self, relative: bool) {
        self.ivars().relative.set(relative);
    }

    /// Focus left this window: release every held key guest-side and forget
    /// them. `AppKit` stops delivering `flagsChanged` and `keyUp` once the
    /// window resigns key, so anything held across a Cmd-Tab would otherwise
    /// stay pressed in the guest forever (a stuck regular key auto-repeats
    /// forever via `wl_keyboard.repeat_info`).
    pub fn release_held_keys(&self) {
        let modifiers: Vec<u16> = self.ivars().held_modifiers.borrow_mut().drain().collect();
        for kvk in modifiers {
            // Tracked for chord classification but never forwarded in
            // translation mode (see flags_changed): releasing it here would
            // hand the guest a Super release for a press it never saw.
            if self.ivars().chord_translation && (kvk == KVK_COMMAND || kvk == KVK_RIGHT_COMMAND)
            {
                continue;
            }
            self.send_key(kvk, ButtonState::Released);
        }
        let keys: Vec<u16> = self.ivars().held_keys.borrow().keys().copied().collect();
        for kvk in keys {
            self.release_forwarded(kvk);
        }
    }

    /// Release exactly what the press for `kvk` forwarded (see
    /// [`ForwardedKey`]); a no-op for keys the guest never saw pressed.
    fn release_forwarded(&self, kvk: u16) {
        let (key, ctrl_still_needed) = {
            let mut held = self.ivars().held_keys.borrow_mut();
            let Some(key) = held.remove(&kvk) else {
                return;
            };
            (key, held.values().any(|other| other.ctrl))
        };
        self.send_keycode(key.keycode, ButtonState::Released);
        // Only release a ctrl we pressed (`synthetic_ctrl`): a chord that
        // rode a physically-held ctrl must leave that ctrl to the user's own
        // `flagsChanged` release.
        if key.ctrl && !ctrl_still_needed && self.ivars().synthetic_ctrl.get() {
            self.ivars().synthetic_ctrl.set(false);
            self.send_keycode(KEY_LEFTCTRL, ButtonState::Released);
        }
    }

    /// A physical ctrl key's forwarded press is still outstanding.
    fn real_ctrl_held(&self) -> bool {
        let held = self.ivars().held_modifiers.borrow();
        held.contains(&KVK_CONTROL) || held.contains(&KVK_RIGHT_CONTROL)
    }

    /// Cmd went fully up: release every key that went down inside the chord.
    /// Their real keyUps are normally re-delivered by the un-swallow monitor
    /// (`app::install_key_up_monitor`), but that leans on `AppKit` dispatch
    /// details; this sweep guarantees no chorded key outlives its chord (a
    /// latched Backspace auto-repeats guest-side forever). A chord key still
    /// physically held after Cmd-up stops repeating early, the safe side of
    /// the trade.
    fn release_cmd_chords(&self) {
        let chorded: Vec<u16> = self
            .ivars()
            .held_keys
            .borrow()
            .iter()
            .filter(|(_, key)| key.chorded)
            .map(|(kvk, _)| *kvk)
            .collect();
        for kvk in chorded {
            self.release_forwarded(kvk);
        }
    }

    /// Event location in surface coordinates (top-left origin, buffer scale).
    fn surface_point(&self, event: &NSEvent) -> NSPoint {
        let local = self.convertPoint_fromView(event.locationInWindow(), None);
        let scale = self.window().map_or(1.0, |window| window.backingScaleFactor());
        NSPoint::new(local.x * scale, local.y * scale)
    }

    fn send_motion(&self, event: &NSEvent) {
        if self.ivars().relative.get() {
            self.send_relative(event);
            return;
        }
        let point = self.surface_point(event);
        app::send(ToGuest::PointerMotion { id: self.ivars().id, x: point.x, y: point.y });
    }

    /// Motion while captured: `NSEvent` deltas keep flowing after
    /// `CGAssociateMouseAndMouseCursorPosition(false)` froze the cursor
    /// (that is the whole point of dissociation). Scaled to buffer pixels
    /// like every other pointer coordinate; signs already match Wayland
    /// (positive = right/down, same as the flipped view).
    fn send_relative(&self, event: &NSEvent) {
        // Drop the tracking-area duplicate of this event (see
        // `ViewIvars::last_relative`); distinct events never share a
        // (timestamp, eventNumber) identity.
        let identity = (event.timestamp(), event.eventNumber());
        if self.ivars().last_relative.get() == identity {
            return;
        }
        self.ivars().last_relative.set(identity);
        let scale = self.window().map_or(1.0, |window| window.backingScaleFactor());
        let (dx, dy) = (event.deltaX() * scale, event.deltaY() * scale);
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        if crate::trace::enabled() {
            // `now - ev` is the HID-to-handler delay: event-queue wait plus
            // AppKit coalescing, the main-thread contention under test.
            eprintln!(
                "panes-trace input id={} ev={:.6} now={:.6} dx={dx} dy={dy}",
                self.ivars().id,
                event.timestamp(),
                crate::trace::now(),
            );
        }
        app::send(ToGuest::PointerRelative { id: self.ivars().id, dx, dy });
    }

    fn send_button(&self, event: &NSEvent, state: ButtonState) {
        let button = match event.buttonNumber() {
            0 => BTN_LEFT,
            1 => BTN_RIGHT,
            2 => BTN_MIDDLE,
            3 => BTN_SIDE,
            4 => BTN_EXTRA,
            other => {
                eprintln!("panes-host: ignoring mouse button {other}");
                return;
            }
        };
        // wl_pointer.button carries no position; re-anchor the pointer first
        // so a click after focus change lands where the user clicked.
        self.send_motion(event);
        // AppKit unhides a captured (hidden) cursor on its right-mouse-down
        // menu-preparation path; re-hide around every button so holding
        // right-click in a pointer-locked game never shows the cursor.
        if self.ivars().relative.get() {
            app::reassert_capture_cursor();
        }
        app::send(ToGuest::PointerButton { id: self.ivars().id, button, state });
    }

    fn send_scroll(&self, event: &NSEvent) {
        let id = self.ivars().id;
        // wl_pointer.axis carries no position either: the first event over a
        // window can be a scroll (two-finger scroll without a prior click or
        // move), so re-anchor pointer focus first, like the button path.
        self.send_motion(event);
        let momentum = event.momentumPhase();
        let phase = event.phase();
        // Finger-up for an ordinary trackpad gesture arrives as
        // `phase() == Ended`; `momentumPhase()` only covers the later inertial
        // tail. Both close a scroll segment with wl_pointer axis_stop, so
        // kinetic scrolling in the guest halts with the gesture.
        if momentum.contains(NSEventPhase::Ended)
            || momentum.contains(NSEventPhase::Cancelled)
            || phase.contains(NSEventPhase::Ended)
            || phase.contains(NSEventPhase::Cancelled)
        {
            app::send(ToGuest::PointerAxis {
                id,
                source: AxisSource::Finger,
                horizontal: 0.0,
                vertical: 0.0,
                v120: None,
                stop: true,
            });
            return;
        }
        let dx = event.scrollingDeltaX();
        let dy = event.scrollingDeltaY();
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let msg = if event.hasPreciseScrollingDeltas() {
            // Trackpad: pixel deltas, scaled to buffer pixels like motion.
            let scale = self.window().map_or(1.0, |window| window.backingScaleFactor());
            ToGuest::PointerAxis {
                id,
                source: AxisSource::Finger,
                horizontal: -dx * scale,
                vertical: -dy * scale,
                v120: None,
                stop: false,
            }
        } else {
            // Wheel: line deltas; v120 is wl_pointer v8 "value120" (120 per
            // detent), axis value uses the libinput 15-units-per-detent rule.
            #[allow(clippy::cast_possible_truncation)]
            let v120 = ((-dx * 120.0) as i32, (-dy * 120.0) as i32);
            ToGuest::PointerAxis {
                id,
                source: AxisSource::Wheel,
                horizontal: -dx * WHEEL_AXIS_PER_LINE,
                vertical: -dy * WHEEL_AXIS_PER_LINE,
                v120: Some(v120),
                stop: false,
            }
        };
        app::send(msg);
    }

    fn send_key(&self, kvk: u16, state: ButtonState) {
        let Some(keycode) = keymap::evdev_from_kvk(kvk) else {
            eprintln!("panes-host: no evdev mapping for kVK {kvk:#x}");
            return;
        };
        self.send_keycode(keycode, state);
    }

    fn send_keycode(&self, keycode: u32, state: ButtonState) {
        app::send(ToGuest::Key { id: self.ivars().id, keycode, state });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_release_after_focus_gain_is_dropped() {
        // The latch bug: modifier physically released after Cmd-Tab back,
        // press never seen -> must forward nothing, not a phantom press.
        assert_eq!(modifier_transition(false, false), None);
    }

    #[test]
    fn modifier_press_and_release_round_trip() {
        assert_eq!(modifier_transition(false, true), Some(ButtonState::Pressed));
        assert_eq!(modifier_transition(true, false), Some(ButtonState::Released));
    }

    #[test]
    fn modifier_sibling_release_keeps_class_down() {
        // Left and right Cmd held, one releases: class flag still set, but a
        // key we saw press can only be releasing.
        assert_eq!(modifier_transition(true, true), Some(ButtonState::Released));
    }

    #[test]
    fn modifier_classes_cover_both_sides() {
        for (left, right, class) in [
            (KVK_SHIFT, KVK_RIGHT_SHIFT, NSEventModifierFlags::Shift),
            (KVK_CONTROL, KVK_RIGHT_CONTROL, NSEventModifierFlags::Control),
            (KVK_OPTION, KVK_RIGHT_OPTION, NSEventModifierFlags::Option),
            (KVK_COMMAND, KVK_RIGHT_COMMAND, NSEventModifierFlags::Command),
        ] {
            assert_eq!(modifier_class(left), Some(class));
            assert_eq!(modifier_class(right), Some(class));
        }
        assert_eq!(modifier_class(KVK_FUNCTION), Some(NSEventModifierFlags::Function));
        assert_eq!(modifier_class(KVK_ANSI_A), None);
    }

    #[test]
    fn chord_translation_table() {
        // Ctrl-wrapped: same physical key, synthetic ctrl.
        for kvk in [KVK_ANSI_A, KVK_ANSI_C, KVK_ANSI_V, KVK_ANSI_X, KVK_ANSI_Z, KVK_DELETE] {
            let translation = translate_chord(kvk).expect("mapped chord");
            assert!(translation.ctrl);
            assert_eq!(Some(translation.keycode), keymap::evdev_from_kvk(kvk));
        }
        // Line motion: different key, no ctrl.
        assert_eq!(
            translate_chord(KVK_LEFT_ARROW),
            Some(Translation { keycode: KEY_HOME, ctrl: false })
        );
        assert_eq!(
            translate_chord(KVK_RIGHT_ARROW),
            Some(Translation { keycode: KEY_END, ctrl: false })
        );
        // Everything else is swallowed, never typed raw.
        assert_eq!(translate_chord(KVK_ANSI_W), None);
        assert_eq!(translate_chord(0x0F), None); // R
    }
}
