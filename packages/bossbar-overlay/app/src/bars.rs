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
    /// Fill fraction, always clamped to `0.0..=1.0` at construction.
    pub progress: f32,
    pub color: Color,
    pub overlay: Overlay,
    pub position: i64,
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
