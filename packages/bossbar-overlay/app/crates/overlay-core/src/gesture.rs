//! Press/drag/click disambiguation for a draggable overlay window.
//!
//! An overlay window is both draggable (grab it to move, the OS owns the drag via
//! `Window::drag_window`) and clickable (a stationary press-release triggers an
//! action). This state machine watches the winit cursor and left-button events
//! and tells the caller when to hand off to the native drag and whether a release
//! was a click. No cursor polling, no hit-test plumbing.

use winit::dpi::PhysicalPosition;

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
}
