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

use std::cell::RefCell;
use std::collections::HashSet;

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

/// One line per wheel click, in `wl_pointer` axis units. libinput's convention
/// (15 units per detent) so guest toolkits scroll the expected distance.
const WHEEL_AXIS_PER_LINE: f64 = 15.0;

pub struct ViewIvars {
    id: WindowId,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    /// Modifier keys currently held, keyed by kVK. `flagsChanged` fires for
    /// press and release alike; toggling membership tells them apart without
    /// hardcoding Apple's device-dependent left/right flag bits.
    held_modifiers: RefCell<HashSet<u16>>,
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
            if event.modifierFlags().contains(NSEventModifierFlags::Command) {
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
            self.send_key(code, ButtonState::Pressed);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            self.send_key(event.keyCode(), ButtonState::Released);
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
                if held.insert(code) {
                    ButtonState::Pressed
                } else {
                    held.remove(&code);
                    ButtonState::Released
                }
            };
            self.send_key(code, state);
        }
    }
);

impl PanesView {
    pub fn new(mtm: MainThreadMarker, id: WindowId, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            id,
            tracking: RefCell::new(None),
            held_modifiers: RefCell::new(HashSet::new()),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Focus left this window: release every held modifier guest-side and
    /// forget them. `AppKit` stops delivering `flagsChanged` once the window
    /// resigns key, so a modifier held across a Cmd-Tab would otherwise stay
    /// pressed in the guest forever, and the toggle heuristic would invert
    /// press/release on its next use.
    pub fn release_held_modifiers(&self) {
        let held: Vec<u16> = self.ivars().held_modifiers.borrow_mut().drain().collect();
        for kvk in held {
            self.send_key(kvk, ButtonState::Released);
        }
    }

    /// Event location in surface coordinates (top-left origin, buffer scale).
    fn surface_point(&self, event: &NSEvent) -> NSPoint {
        let local = self.convertPoint_fromView(event.locationInWindow(), None);
        let scale = self.window().map_or(1.0, |window| window.backingScaleFactor());
        NSPoint::new(local.x * scale, local.y * scale)
    }

    fn send_motion(&self, event: &NSEvent) {
        let point = self.surface_point(event);
        app::send(ToGuest::PointerMotion { id: self.ivars().id, x: point.x, y: point.y });
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
        app::send(ToGuest::Key { id: self.ivars().id, keycode, state });
    }
}
