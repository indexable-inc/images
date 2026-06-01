//! Press/drag/click disambiguation for a draggable overlay window, plus
//! two-finger scroll-drag.
//!
//! An overlay window is both draggable (grab it to move, the OS owns the drag via
//! `Window::drag_window`) and clickable (a stationary press-release triggers an
//! action). [`DragClick`] watches the winit cursor and left-button events and
//! tells the caller when to hand off to the native drag and whether a release was
//! a click. No cursor polling, no hit-test plumbing.
//!
//! A two-finger trackpad scroll over the overlay should also move it, so the user
//! can nudge a window without pressing: [`scroll_drag_delta`] turns a winit scroll
//! delta into the logical-point translation the caller adds to the window's
//! position.

use winit::dpi::PhysicalPosition;
use winit::event::MouseScrollDelta;

/// Logical points to move per `LineDelta` notch, for a notched mouse wheel (a
/// trackpad reports precise `PixelDelta` instead). One notch nudges the window a
/// readable step rather than a single point.
const LINE_POINTS: f64 = 16.0;

/// Convert a winit scroll delta into the logical-point `(dx, dy)` translation to
/// add to a window's position so a two-finger trackpad scroll drags the window
/// like a grab.
///
/// A trackpad reports a precise [`MouseScrollDelta::PixelDelta`] in physical
/// pixels (winit builds it as `scrollingDelta * scale_factor`), so dividing by
/// `scale_factor` recovers the logical points the window position is measured in;
/// a notched wheel reports [`MouseScrollDelta::LineDelta`] in lines, scaled by
/// [`LINE_POINTS`].
///
/// The sign is **negated**: a winit scroll delta points the way the *content*
/// would scroll, so moving the window the opposite way makes it track the scroll
/// gesture, grab-and-move. This also inherits the user's scroll-direction
/// preference (so it always matches "the way I normally scroll"). Verified
/// against the user's trackpad: the un-negated version moved the window the wrong
/// way.
pub fn scroll_drag_delta(delta: MouseScrollDelta, scale_factor: f64) -> (f64, f64) {
    match delta {
        MouseScrollDelta::PixelDelta(p) => (-p.x / scale_factor, -p.y / scale_factor),
        MouseScrollDelta::LineDelta(x, y) => (-(x as f64) * LINE_POINTS, -(y as f64) * LINE_POINTS),
    }
}

/// Tracks one window's left-button gesture.
pub struct DragClick {
    /// Pointer travel (physical px) past which a press becomes a drag.
    threshold: f64,
    /// Last known cursor position within the window.
    cursor: Option<PhysicalPosition<f64>>,
    /// The left button is down.
    pressing: bool,
    /// The press has crossed the threshold and the native drag has taken over.
    dragging: bool,
    /// Anchor the drag distance is measured from (cursor at press, or the first
    /// move after a press that had no cursor sample yet).
    press: Option<PhysicalPosition<f64>>,
}

impl DragClick {
    pub fn new(threshold: f64) -> Self {
        Self {
            threshold,
            cursor: None,
            pressing: false,
            dragging: false,
            press: None,
        }
    }

    /// Record a cursor move. Returns `true` exactly once, when the press has just
    /// crossed the threshold and the caller should start the native window drag.
    pub fn cursor_moved(&mut self, pos: PhysicalPosition<f64>) -> bool {
        self.cursor = Some(pos);
        if self.pressing && !self.dragging {
            let origin = *self.press.get_or_insert(pos);
            let dx = pos.x - origin.x;
            let dy = pos.y - origin.y;
            if (dx * dx + dy * dy).sqrt() >= self.threshold {
                self.dragging = true;
                return true;
            }
        }
        false
    }

    /// Record a left-button press. Defers the drag decision to [`Self::cursor_moved`]
    /// so a stationary press stays a click whose release is still delivered.
    pub fn pressed(&mut self) {
        self.pressing = true;
        self.press = self.cursor;
        self.dragging = false;
    }

    /// Record a left-button release. Returns `true` if the gesture was a click
    /// (the pointer never crossed the threshold), then resets for the next press.
    pub fn released(&mut self) -> bool {
        let clicked = self.pressing && !self.dragging;
        self.pressing = false;
        self.press = None;
        self.dragging = false;
        clicked
    }

    /// Whether the native drag is currently in progress (for cursor feedback).
    pub fn dragging(&self) -> bool {
        self.dragging
    }

    /// The last known cursor position within the window.
    pub fn cursor(&self) -> Option<PhysicalPosition<f64>> {
        self.cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f64, y: f64) -> PhysicalPosition<f64> {
        PhysicalPosition::new(x, y)
    }

    #[test]
    fn stationary_press_release_is_a_click() {
        let mut g = DragClick::new(5.0);
        g.cursor_moved(at(10.0, 10.0));
        g.pressed();
        // A sub-threshold jitter does not start a drag.
        assert!(!g.cursor_moved(at(12.0, 11.0)));
        assert!(g.released(), "small movement still counts as a click");
    }

    #[test]
    fn dragging_past_threshold_is_not_a_click() {
        let mut g = DragClick::new(5.0);
        g.cursor_moved(at(10.0, 10.0));
        g.pressed();
        assert!(g.cursor_moved(at(20.0, 10.0)), "crossing threshold starts the drag");
        assert!(g.dragging());
        assert!(!g.released(), "a drag is not a click");
        assert!(!g.dragging(), "release clears the drag");
    }

    #[test]
    fn pixel_scroll_converts_physical_to_logical_points() {
        // A trackpad PixelDelta is physical px; dividing by the scale factor gives
        // the logical points the window position is in. The sign is negated so the
        // window tracks the scroll gesture rather than the content direction.
        let (dx, dy) =
            scroll_drag_delta(MouseScrollDelta::PixelDelta(at(20.0, -8.0)), 2.0);
        assert_eq!((dx, dy), (-10.0, 4.0));
    }

    #[test]
    fn window_moves_opposite_the_reported_scroll() {
        // A winit scroll delta points the way the content would scroll; the window
        // moves the other way so it follows the gesture (grab-and-move).
        let (dx, dy) =
            scroll_drag_delta(MouseScrollDelta::PixelDelta(at(5.0, 7.0)), 1.0);
        assert!(dx < 0.0 && dy < 0.0, "window moves opposite the content-scroll delta");
    }

    #[test]
    fn line_scroll_steps_by_a_readable_amount() {
        let (dx, dy) = scroll_drag_delta(MouseScrollDelta::LineDelta(0.0, -1.0), 2.0);
        // LineDelta is in lines, independent of the scale factor; sign negated.
        assert_eq!(dx, 0.0);
        assert_eq!(dy, LINE_POINTS);
    }
}
