//! The typed boss bar domain, mirroring Minecraft's own boss bar API.
//!
//! Strings come off the SQLite rows untrusted, so parsing lands here at the
//! owner boundary: an unknown color or overlay falls back to the same default
//! the old web renderer used (`purple` / smooth `progress`) rather than
//! drawing nothing.

/// The seven `BossBarColor` sprite sets Minecraft ships.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Color {
    Pink,
    Blue,
    Red,
    Green,
    Yellow,
    Purple,
    White,
}

impl Color {
    /// Parse a stored color string, defaulting to `Purple` for anything
    /// unrecognized so a typo still draws a bar.
    pub fn parse(s: &str) -> Self {
        match s {
            "pink" => Self::Pink,
            "blue" => Self::Blue,
            "red" => Self::Red,
            "green" => Self::Green,
            "yellow" => Self::Yellow,
            "white" => Self::White,
            _ => Self::Purple,
        }
    }

    /// Representative RGB for this bar color (0..=255), approximating the vanilla
    /// boss bar sprite hue. Used to tint the hover panel's border so the pop-down
    /// reads as belonging to its bar; the bar sprites themselves stay textured.
    pub fn accent_rgb(self) -> [u8; 3] {
        match self {
            Self::Pink => [0xE0, 0x6A, 0xB8],
            Self::Blue => [0x46, 0x6C, 0xE6],
            Self::Red => [0xD8, 0x3A, 0x32],
            Self::Green => [0x5A, 0xC0, 0x3A],
            Self::Yellow => [0xE6, 0xC8, 0x3A],
            Self::Purple => [0x9B, 0x59, 0xD6],
            Self::White => [0xD8, 0xD8, 0xD8],
        }
    }
}

/// The notch overlays Minecraft draws on top of the color bar. `None` is the
/// smooth `progress` overlay, which draws no notch sprite.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Overlay {
    None,
    Notched6,
    Notched10,
    Notched12,
    Notched20,
}

impl Overlay {
    pub fn parse(s: &str) -> Self {
        match s {
            "notched_6" => Self::Notched6,
            "notched_10" => Self::Notched10,
            "notched_12" => Self::Notched12,
            "notched_20" => Self::Notched20,
            _ => Self::None,
        }
    }

    /// The notch sprite key, or `None` for the smooth overlay.
    pub fn notch(self) -> Option<Notch> {
        match self {
            Self::None => None,
            Self::Notched6 => Some(Notch::N6),
            Self::Notched10 => Some(Notch::N10),
            Self::Notched12 => Some(Notch::N12),
            Self::Notched20 => Some(Notch::N20),
        }
    }
}

/// The four notch sprite variants. Distinct from [`Overlay`] because the smooth
/// overlay has no sprite, and the renderer only ever loads these four.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Notch {
    N6,
    N10,
    N12,
    N20,
}

/// One boss bar row, ready to render.
#[derive(Clone, Debug, PartialEq)]
pub struct BossBar {
    pub id: i64,
    pub title: String,
    /// Longer free text revealed in a panel that unfolds below the bar on hover.
    /// Empty means the bar has no pop-down and behaves exactly as it did before.
    /// Newlines separate paragraphs; the panel wraps long lines to its width.
    pub description: String,
    /// Unix epoch (seconds) the bar started counting from. When set, the overlay
    /// appends a live, self-ticking elapsed time to the title (e.g. "2:05") and
    /// redraws once a second, so a caller writes the start once instead of
    /// rewriting the title to advance a clock. `None` (or a non-positive value in
    /// the DB) means no counter.
    pub since: Option<i64>,
    /// A URL (or any URI/path the OS opener accepts) opened when the bar is
    /// clicked without dragging. Empty means a click does nothing.
    pub url: String,
    /// Expected total duration in seconds. When set together with `since`, the
    /// overlay IGNORES the stored `progress` and instead extrapolates the fill
    /// live as `(now - since) / eta`, clamped below 1.0, redrawing each second so
    /// a long-running task's bar advances smoothly between the writer's polls
    /// instead of stepping. `None` means use the static `progress`. Persisted to
    /// the `eta` DB column.
    pub eta: Option<i64>,
    /// Whether hovering may unfold the description panel below the bar. `true`
    /// (the default) keeps the old behavior; `false` makes the bar stay bar-sized
    /// on hover with no pop-down box, even if it carries a `description`. Lets a
    /// caller draw many compact bars (e.g. one per CI run) without each growing a
    /// panel. Persisted to the `expandable` DB column.
    pub expandable: bool,
    /// Fill fraction, always clamped to `0.0..=1.0` at construction.
    pub progress: f32,
    pub color: Color,
    pub overlay: Overlay,
    pub position: i64,
    /// Pinned on-screen location in logical screen points (top-left origin of
    /// the title), set once the bar is dragged. `None` keeps the bar in the
    /// auto-stacked top-center column. Persisted to the `x`/`y` DB columns.
    pub pos: Option<glam::DVec2>,
    /// Filesystem path to a small image (PNG/JPEG) drawn as a square icon to the
    /// left of the title, e.g. the GitHub avatar of whoever opened a PR. Empty
    /// means no icon. The overlay loads and caches it by path; a path that does
    /// not exist or fails to decode is simply skipped (the bar still draws).
    /// Persisted to the `icon` DB column.
    pub icon: String,
    /// Name of a user-supplied texture set to render this bar from instead of
    /// the vanilla `color` sprites (see `theme.rs` for the on-disk layout under
    /// the themes directory). Empty means vanilla. A theme that is missing or
    /// fails to load falls back to the vanilla `color`/`overlay` sprites, the
    /// same "a typo still draws a bar" policy as `Color::parse`. Persisted to
    /// the `theme` DB column.
    pub theme: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_color_falls_back_to_purple() {
        assert_eq!(Color::parse("chartreuse"), Color::Purple);
        assert_eq!(Color::parse("pink"), Color::Pink);
    }

    #[test]
    fn unknown_overlay_is_smooth_and_drawless() {
        assert_eq!(Overlay::parse("progress"), Overlay::None);
        assert_eq!(Overlay::parse("nonsense"), Overlay::None);
        assert!(Overlay::None.notch().is_none());
        assert_eq!(Overlay::parse("notched_20").notch(), Some(Notch::N20));
    }
}
