//! The typed experience-orb domain: a floating orb whose size mirrors an XP
//! amount, an optional click URL, and a pinned position. Strings are untrusted but
//! carry no enums to parse; the amount is clamped non-negative so a malformed row
//! still renders the smallest orb.

use glam::DVec2;

/// One experience orb, ready to render.
#[derive(Clone, Debug, PartialEq)]
pub struct Orb {
    /// XP the orb represents. Larger amounts pick a bigger, brighter vanilla orb
    /// icon (see [`crate::scene::icon_for`]). Never negative.
    pub amount: i64,
    /// Opened with the platform opener on a click that was not a drag. Empty means
    /// the orb has no click action.
    pub url: String,
    /// Pinned on-screen location in logical screen points (window top-left), set
    /// once the orb is dragged or scrolled. `None` keeps it centered. Persisted to
    /// the `x`/`y` columns.
    pub pos: Option<DVec2>,
}

impl Default for Orb {
    fn default() -> Self {
        Self {
            amount: 0,
            url: String::new(),
            pos: None,
        }
    }
}
