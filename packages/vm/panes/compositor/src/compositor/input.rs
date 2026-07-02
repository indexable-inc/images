//! `ToGuest` input messages -> smithay seat events.
//!
//! Coordinates arrive surface-local (the host tracks which `NSWindow` the
//! event hit), so pointer focus is set explicitly per event with the target
//! surface anchored at the global origin; there is no shared scene the
//! pointer roams across, which is exactly why no window positions exist
//! here.

use panes_protocol::{AxisSource as WireAxisSource, ButtonState as WireButtonState, ToGuest};
use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::input::keyboard::{FilterResult, Keycode};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::{Point, SERIAL_COUNTER};

use super::App;

pub fn handle(app: &mut App, msg: &ToGuest) {
    match msg {
        ToGuest::PointerMotion { id, x, y } => pointer_motion(app, *id, *x, *y),
        ToGuest::PointerButton { id, button, state } => pointer_button(app, *id, *button, *state),
        ToGuest::PointerAxis {
            id,
            source,
            horizontal,
            vertical,
            v120,
            stop,
        } => pointer_axis(app, *id, *source, *horizontal, *vertical, *v120, *stop),
        ToGuest::PointerLeave { id } => pointer_leave(app, *id),
        ToGuest::Key { id, keycode, state } => key(app, *id, *keycode, *state),
        // Non-input messages are dispatched before reaching this module.
        _ => {}
    }
}

fn pointer_motion(app: &mut App, id: panes_protocol::WindowId, x: f64, y: f64) {
    let Some(surface) = app.pane_surface(id) else {
        return;
    };
    let Some(pointer) = app.seat.get_pointer() else {
        return;
    };
    app.pointer_focus = Some(id);
    let time = app.now_ms();
    // Wire coords are drawable pixels (panes-protocol convention); smithay's
    // pointer space is logical, so divide by the host scale, mirroring the
    // xdg configure division in `on_configure`.
    let scale = host_scale(app);
    // Focus is handed over explicitly (second tuple element = the surface's
    // origin in "global" space, which for us is always 0,0), so smithay
    // emits enter/leave pairs as the host moves between windows.
    pointer.motion(
        app,
        Some((surface, Point::default())),
        &MotionEvent {
            location: (x / scale, y / scale).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    pointer.frame(app);
}

/// The host's global `backingScaleFactor` from Hello, as the divisor that
/// takes wire pixel coordinates to logical surface coordinates. Input only
/// flows after Hello, so a missing host (1) is a startup-race fallback, not a
/// silent unit change.
fn host_scale(app: &App) -> f64 {
    f64::from(app.host.as_ref().map_or(1, |host| host.scale.max(1)))
}

fn pointer_button(
    app: &mut App,
    id: panes_protocol::WindowId,
    button: u32,
    state: WireButtonState,
) {
    if app.pane_surface(id).is_none() {
        return;
    }
    let Some(pointer) = app.seat.get_pointer() else {
        return;
    };
    let time = app.now_ms();
    pointer.button(
        app,
        &ButtonEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time,
            button,
            state: match state {
                WireButtonState::Pressed => ButtonState::Pressed,
                WireButtonState::Released => ButtonState::Released,
            },
        },
    );
    pointer.frame(app);
}

fn pointer_axis(
    app: &mut App,
    id: panes_protocol::WindowId,
    source: WireAxisSource,
    horizontal: f64,
    vertical: f64,
    v120: Option<(i32, i32)>,
    stop: bool,
) {
    if app.pane_surface(id).is_none() {
        return;
    }
    let Some(pointer) = app.seat.get_pointer() else {
        return;
    };
    let time = app.now_ms();
    // Finger/Continuous deltas arrive in drawable pixels (the host multiplies
    // precise trackpad deltas by `backingScaleFactor`), so they get the same
    // pixel->logical division as motion. Wheel values are line-based axis
    // units (15 per detent) that never were in pixel space.
    let (horizontal, vertical) = if source == WireAxisSource::Wheel {
        (horizontal, vertical)
    } else {
        let scale = host_scale(app);
        (horizontal / scale, vertical / scale)
    };
    let source = match source {
        WireAxisSource::Wheel => AxisSource::Wheel,
        WireAxisSource::Finger => AxisSource::Finger,
        WireAxisSource::Continuous => AxisSource::Continuous,
    };
    let mut frame = AxisFrame::new(time).source(source);
    // value120 is only defined for wheel steps (wl_pointer v8); smithay
    // derives legacy discrete events from it for older clients.
    if source == AxisSource::Wheel
        && let Some((h120, v120)) = v120
    {
        frame = frame
            .v120(Axis::Horizontal, h120)
            .v120(Axis::Vertical, v120);
    }
    frame = frame
        .value(Axis::Horizontal, horizontal)
        .value(Axis::Vertical, vertical);
    if stop {
        frame = frame.stop(Axis::Horizontal).stop(Axis::Vertical);
    }
    pointer.axis(app, frame);
    pointer.frame(app);
}

fn pointer_leave(app: &mut App, id: panes_protocol::WindowId) {
    if app.pointer_focus != Some(id) {
        return;
    }
    app.pointer_focus = None;
    let Some(pointer) = app.seat.get_pointer() else {
        return;
    };
    let time = app.now_ms();
    pointer.motion(
        app,
        None,
        &MotionEvent {
            location: Point::default(),
            serial: SERIAL_COUNTER.next_serial(),
            time,
        },
    );
    pointer.frame(app);
}

fn key(app: &mut App, id: panes_protocol::WindowId, keycode: u32, state: WireButtonState) {
    let Some(surface) = app.pane_surface(id) else {
        return;
    };
    let Some(keyboard) = app.seat.get_keyboard() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    // The host routes keys by which NSWindow is key; follow it so wl_keyboard
    // focus matches even if no Configure{activated} raced ahead.
    if app.key_focus != Some(id) {
        keyboard.set_focus(app, Some(surface), serial);
        app.key_focus = Some(id);
    }
    let time = app.now_ms();
    // Wire keycodes are evdev (already xkb - 8 per the protocol comment);
    // smithay/xkb expects the +8 offset form, same as its libinput backend.
    let xkb_keycode = Keycode::from(keycode.saturating_add(8));
    keyboard.input::<(), _>(
        app,
        xkb_keycode,
        match state {
            WireButtonState::Pressed => KeyState::Pressed,
            WireButtonState::Released => KeyState::Released,
        },
        serial,
        time,
        |_, _, _| FilterResult::Forward,
    );
}
