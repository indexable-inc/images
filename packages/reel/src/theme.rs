//! Flat, ghostty-minimal palettes and color resolution.
//!
//! Two palettes, dark and light, each with the base surfaces (background,
//! foreground, a dim tone, an accent, the chrome bar, and its hairline rule)
//! plus the 16 ANSI colors. The ANSI colors are deliberately muted rather than
//! saturated so the demo reads as calm and flat instead of neon.

use tui::Color;

/// A 24-bit color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// A terminal palette: the base surfaces plus the 16 ANSI colors.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Rgb,
    pub fg: Rgb,
    pub dim: Rgb,
    pub accent: Rgb,
    pub chrome: Rgb,
    pub rule: Rgb,
    pub ansi: [Rgb; 16],
}

/// Which palette to render with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    /// A short stable token used in output file names.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// The palette for this theme.
    #[must_use]
    pub const fn palette(self) -> Palette {
        match self {
            Self::Dark => DARK,
            Self::Light => LIGHT,
        }
    }
}

impl Palette {
    /// Resolve a VT color to concrete RGB. [`Color::Default`] returns `None` so
    /// the caller can substitute the surface's own default (foreground for text,
    /// background for fills).
    #[must_use]
    pub const fn resolve(&self, color: Color) -> Option<Rgb> {
        match color {
            Color::Default => None,
            Color::Indexed(index) => Some(self.indexed(index)),
            Color::Rgb(r, g, b) => Some(Rgb::new(r, g, b)),
        }
    }

    const fn indexed(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[index as usize],
            16..=231 => cube(index - 16),
            232..=255 => gray(index - 232),
        }
    }
}

/// One channel level for the xterm-256 color cube.
const fn channel_level(component: u8) -> u8 {
    if component == 0 {
        0
    } else {
        55 + 40 * component
    }
}

/// The 6x6x6 color cube entry for an xterm-256 index in `0..216`.
const fn cube(offset: u8) -> Rgb {
    Rgb::new(
        channel_level(offset / 36),
        channel_level((offset / 6) % 6),
        channel_level(offset % 6),
    )
}

/// The grayscale ramp entry for an xterm-256 index in `0..24`.
const fn gray(offset: u8) -> Rgb {
    let value = 8 + 10 * offset;
    Rgb::new(value, value, value)
}

const DARK: Palette = Palette {
    bg: Rgb::new(13, 14, 17),
    fg: Rgb::new(208, 212, 219),
    dim: Rgb::new(110, 116, 128),
    accent: Rgb::new(110, 160, 224),
    chrome: Rgb::new(22, 24, 29),
    rule: Rgb::new(34, 36, 42),
    ansi: [
        Rgb::new(38, 40, 46),
        Rgb::new(224, 108, 117),
        Rgb::new(140, 200, 130),
        Rgb::new(214, 184, 110),
        Rgb::new(110, 160, 224),
        Rgb::new(180, 150, 220),
        Rgb::new(110, 196, 200),
        Rgb::new(208, 212, 219),
        Rgb::new(90, 96, 108),
        Rgb::new(232, 140, 148),
        Rgb::new(168, 214, 160),
        Rgb::new(228, 206, 150),
        Rgb::new(150, 190, 236),
        Rgb::new(202, 178, 230),
        Rgb::new(150, 212, 214),
        Rgb::new(238, 240, 244),
    ],
};

const LIGHT: Palette = Palette {
    bg: Rgb::new(252, 252, 251),
    fg: Rgb::new(44, 48, 56),
    dim: Rgb::new(140, 146, 156),
    accent: Rgb::new(48, 108, 196),
    chrome: Rgb::new(244, 244, 243),
    rule: Rgb::new(225, 226, 224),
    ansi: [
        Rgb::new(60, 64, 72),
        Rgb::new(196, 60, 72),
        Rgb::new(60, 150, 80),
        Rgb::new(168, 128, 32),
        Rgb::new(48, 108, 196),
        Rgb::new(150, 80, 180),
        Rgb::new(40, 150, 160),
        Rgb::new(60, 64, 72),
        Rgb::new(150, 156, 166),
        Rgb::new(204, 72, 84),
        Rgb::new(72, 162, 92),
        Rgb::new(180, 140, 44),
        Rgb::new(60, 120, 204),
        Rgb::new(162, 92, 192),
        Rgb::new(52, 162, 172),
        Rgb::new(20, 22, 28),
    ],
};
