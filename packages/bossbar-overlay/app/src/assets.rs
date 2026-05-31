//! Embedded Mojang art. The boss bar sprite PNGs and the Minecraft "Mojangles"
//! TTF are Mojang-derived and are *not* committed to this repo (they are
//! gitignored). The Nix build and `scripts/fetch-assets.sh` drop them into
//! `assets/` before compilation, and `include_bytes!` bakes them into the
//! single binary so there is no runtime asset path to resolve.

use crate::bars::{Color, Notch};

/// The Minecraft title font (tryashtar/minecraft-ttf). Its internal family name
/// is `"Minecraft"`, which the text renderer selects by name.
pub const FONT: &[u8] = include_bytes!("../assets/fonts/MinecraftDefault-Regular.ttf");
pub const FONT_FAMILY: &str = "Minecraft";

macro_rules! sprite {
    ($name:literal) => {
        include_bytes!(concat!("../assets/boss_bar/", $name, ".png"))
    };
}

/// `(background, progress)` sprite bytes for one boss bar color.
pub fn color_sprites(c: Color) -> (&'static [u8], &'static [u8]) {
    match c {
        Color::Pink => (sprite!("pink_background"), sprite!("pink_progress")),
        Color::Blue => (sprite!("blue_background"), sprite!("blue_progress")),
        Color::Red => (sprite!("red_background"), sprite!("red_progress")),
        Color::Green => (sprite!("green_background"), sprite!("green_progress")),
        Color::Yellow => (sprite!("yellow_background"), sprite!("yellow_progress")),
        Color::Purple => (sprite!("purple_background"), sprite!("purple_progress")),
        Color::White => (sprite!("white_background"), sprite!("white_progress")),
    }
}

/// `(background, progress)` sprite bytes for one notch overlay.
pub fn notch_sprites(n: Notch) -> (&'static [u8], &'static [u8]) {
    match n {
        Notch::N6 => (sprite!("notched_6_background"), sprite!("notched_6_progress")),
        Notch::N10 => (
            sprite!("notched_10_background"),
            sprite!("notched_10_progress"),
        ),
        Notch::N12 => (
            sprite!("notched_12_background"),
            sprite!("notched_12_progress"),
        ),
        Notch::N20 => (
            sprite!("notched_20_background"),
            sprite!("notched_20_progress"),
        ),
    }
}

/// Every color, for preloading textures at startup.
pub const COLORS: [Color; 7] = [
    Color::Pink,
    Color::Blue,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Purple,
    Color::White,
];

/// Every notch, for preloading textures at startup.
pub const NOTCHES: [Notch; 4] = [Notch::N6, Notch::N10, Notch::N12, Notch::N20];
