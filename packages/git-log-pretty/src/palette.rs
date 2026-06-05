//! Terminal color helpers keyed off the detected light/dark theme.
//!
//! Styling goes through [`anstyle`] so callers render SGR sequences the same way
//! the rest of the repo's terminal surfaces do. The light/dark decision is owned
//! by [`terminal_theme`] and re-exported here as [`Theme`]; a dark terminal gets
//! brighter foregrounds and a light terminal gets darker, higher-contrast ones.

use std::hash::{Hash, Hasher};

use anstyle::{Color, RgbColor, Style};

pub use terminal_theme::{Theme, detect};

/// Map a [`Theme`] to the [`devicons`] theme so file icons pick readable glyph
/// colors against the detected background.
pub const fn devicons(theme: Theme) -> devicons::Theme {
    match theme {
        Theme::Light => devicons::Theme::Light,
        Theme::Dark => devicons::Theme::Dark,
    }
}

/// Neutral gray used for tree connectors and directory segments. Picked to read
/// on both light and dark backgrounds without competing with the file colors.
pub const GRAY: RgbColor = RgbColor(128, 128, 128);

/// Build a foreground style for `color`.
pub const fn fg(color: Color) -> Style {
    Style::new().fg_color(Some(color))
}

/// Wrap `text` in `style`, appending the matching reset so later output is
/// unstyled. `anstyle`'s `Display` renders the SGR prefix; `render_reset` closes
/// it, which is the pattern used elsewhere in the repo (see `code-highlight`).
pub fn paint(style: Style, text: &str) -> String {
    format!("{style}{text}{reset}", reset = style.render_reset())
}

/// Pick a stable background color for a conventional-commit type by hashing the
/// type string into a hue. Saturation and value shift with the theme so the
/// chip stays legible under white text on dark terminals and black text on light
/// ones.
pub fn hashed_chip_background(label: &str, theme: Theme) -> RgbColor {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    label.hash(&mut hasher);
    // `% 360` bounds the value to `0..360`, well within `u16`, so the cast is
    // lossless.
    let hue = f32::from((hasher.finish() % 360) as u16);

    let (saturation, value) = match theme {
        Theme::Dark => (0.6, 0.5),
        Theme::Light => (0.4, 0.8),
    };

    hsv_to_rgb(hue, saturation, value)
}

/// Foreground that contrasts with [`hashed_chip_background`] for the same theme.
pub const fn chip_foreground(theme: Theme) -> RgbColor {
    match theme {
        Theme::Dark => RgbColor(255, 255, 255),
        Theme::Light => RgbColor(0, 0, 0),
    }
}

/// Parse a `#rrggbb` (or `rrggbb`) hex string into an RGB color, falling back to
/// the neutral gray when the input is malformed. Devicons hands back colors in
/// this shape.
pub fn parse_hex(hex: &str) -> RgbColor {
    let hex = hex.trim_start_matches('#');
    let byte = |range: std::ops::Range<usize>| -> Option<u8> {
        hex.get(range).and_then(|s| u8::from_str_radix(s, 16).ok())
    };

    match (byte(0..2), byte(2..4), byte(4..6)) {
        (Some(r), Some(g), Some(b)) if hex.len() == 6 => RgbColor(r, g, b),
        _ => GRAY,
    }
}

/// Convert an HSV triple (hue in degrees, saturation and value in `0.0..=1.0`)
/// into an 8-bit RGB color. Used to spread hashed commit-type hues across the
/// wheel at a fixed saturation and value.
fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> RgbColor {
    let chroma = value * saturation;
    let second = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let base = value - chroma;

    let (red, green, blue) = match hue {
        h if h < 60.0 => (chroma, second, 0.0),
        h if h < 120.0 => (second, chroma, 0.0),
        h if h < 180.0 => (0.0, chroma, second),
        h if h < 240.0 => (0.0, second, chroma),
        h if h < 300.0 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };

    // The clamp pins the value into `0..=255` before the cast, so neither
    // truncation nor sign loss can change the result; the lints fire on the
    // `as u8` syntax regardless.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let to_byte = |component: f32| ((component + base) * 255.0).round().clamp(0.0, 255.0) as u8;
    RgbColor(to_byte(red), to_byte(green), to_byte(blue))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_round_trips_six_digit_colors() {
        assert_eq!(parse_hex("#114957"), RgbColor(0x11, 0x49, 0x57));
        assert_eq!(parse_hex("ffffff"), RgbColor(0xff, 0xff, 0xff));
    }

    #[test]
    fn parse_hex_falls_back_on_bad_input() {
        assert_eq!(parse_hex("#xyz"), GRAY);
        assert_eq!(parse_hex(""), GRAY);
        assert_eq!(parse_hex("#1234"), GRAY);
    }

    #[test]
    fn hashed_chip_background_is_stable_per_label() {
        assert_eq!(
            hashed_chip_background("feat", Theme::Dark),
            hashed_chip_background("feat", Theme::Dark),
        );
    }

    #[test]
    fn hsv_primaries_map_to_expected_corners() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), RgbColor(255, 0, 0));
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), RgbColor(0, 255, 0));
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), RgbColor(0, 0, 255));
    }
}
