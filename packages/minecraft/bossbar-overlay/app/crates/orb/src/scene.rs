//! Turning a pop event into textured quads. Two pop "kinds" share one footprint
//! and the same label rendering: the `orb` (success) is the vanilla experience
//! orb, sized by XP amount and tinted a pulsing green-yellow; the `villager`
//! (failure) is the grey angry-villager particle, drawn at the orb's footprint
//! with a steady warm-grey tint. The orb art is one 64x64 sheet of 16x16 icons in
//! a 4x4 grid; the villager art is a single 8x8 sprite upscaled to the footprint.

use overlay_core::{anim, BitmapFont, Gpu, Quad, TexHandle, SHADOW};

use crate::assets;
use crate::orb::Orb;

/// On-screen sprite footprint (source px) shared by both kinds: the orb sheet's
/// cell size, and what the smaller villager sprite is upscaled to.
const SPRITE_PX: u32 = 16;
/// The orb sheet width (px) and its column count.
const SHEET: f32 = 64.0;
const COLS: u32 = 4;

/// Largest the orb grows to on hover.
pub const MAX_MUL: f32 = 1.18;

/// Bob amplitude in source pixels: the orb drifts +/- this (times `scale`) about
/// its resting centre. Small, so it reads as a gentle float, not a bounce.
const BOB_PX: u32 = 2;

/// Label font size relative to the sprite scale, and the gap (source px) between
/// the sprite and its label in a "pop".
const LABEL_SCALE: f32 = 0.6;
const GAP_PX: u32 = 3;

/// Full-texture UV: the villager sprite is a single image, drawn whole.
const FULL_UV: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// Tint for the villager puff. The vanilla angry-villager particle is already a
/// grey cloud with warm anger sparks, so it is drawn faithfully (identity tint);
/// unlike the grey orb sheet it needs no recolouring to read. Kept as a named
/// constant so the look stays adjustable in one place.
const VILLAGER_TINT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Which pop sprite to draw. The wire form (`"orb"` / `"villager"`) is what the
/// `events` table stores and what `xp-orb-overlay push --kind` accepts.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    /// Success: the experience orb, sized by amount, rising with the XP sound.
    #[default]
    Orb,
    /// Failure: the grey angry-villager puff, with the villager "no" sound.
    Villager,
}

impl Kind {
    /// Parse the wire form. An unknown string yields `None` so a typo is a CLI
    /// usage error rather than a silently wrong sprite.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "orb" => Some(Self::Orb),
            "villager" => Some(Self::Villager),
            _ => None,
        }
    }

    /// The wire form stored in the DB and accepted on the CLI.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Orb => "orb",
            Self::Villager => "villager",
        }
    }
}

/// The registered sprite textures, one per [`Kind`].
pub struct Sprites {
    orb: TexHandle,
    villager: TexHandle,
}

pub fn register(gpu: &mut Gpu) -> Sprites {
    Sprites {
        orb: gpu.register_png(assets::EXPERIENCE_ORB),
        villager: gpu.register_png(assets::ANGRY_VILLAGER),
    }
}

impl Sprites {
    /// The texture handle, UV sub-rect, and tint for `kind`. The orb picks its
    /// sheet cell from `amount` and pulses with `shimmer`; the villager uses its
    /// whole sprite with a steady tint (both args ignored).
    fn draw(&self, kind: Kind, amount: i64, shimmer: f32) -> (TexHandle, [f32; 4], [f32; 4]) {
        match kind {
            Kind::Orb => (self.orb, icon_uv(icon_for(amount)), shimmer_tint(shimmer)),
            Kind::Villager => (self.villager, FULL_UV, VILLAGER_TINT),
        }
    }
}

/// Vanilla XP-amount -> orb icon (0..=10): the same thresholds Minecraft uses to
/// pick a bigger, brighter orb for more experience.
pub fn icon_for(amount: i64) -> u32 {
    const THRESHOLDS: [(i64, u32); 11] = [
        (2477, 10),
        (1237, 9),
        (617, 8),
        (307, 7),
        (149, 6),
        (73, 5),
        (37, 4),
        (17, 3),
        (7, 2),
        (3, 1),
        (0, 0),
    ];
    for (min, icon) in THRESHOLDS {
        if amount >= min {
            return icon;
        }
    }
    0
}

/// UV sub-rect for icon `i` within the 4x4 sheet.
fn icon_uv(i: u32) -> [f32; 4] {
    let i = i.min(COLS * COLS - 1);
    let s = SPRITE_PX as f32;
    let x = (i % COLS) as f32 * s;
    let y = (i / COLS) as f32 * s;
    [x / SHEET, y / SHEET, (x + s) / SHEET, (y + s) / SHEET]
}

/// Square window (physical px) holding the orb at its hovered size plus room for
/// the bob, kept tight so the desktop stays click-through around it.
pub fn orb_window_px(scale: u32) -> (u32, u32) {
    let orb = SPRITE_PX * scale;
    let grow = ((orb as f32) * (MAX_MUL - 1.0)).ceil() as u32;
    let margin = BOB_PX * scale + grow + scale;
    let side = orb + 2 * margin;
    (side, side)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// The pulsing green-yellow tint for shimmer phase `t` in `0..=1`. The orb art is
/// mid-grey, so the tint is overdriven past 1.0 to glow: green stays high while a
/// little red and blue breathe in and out, swinging green <-> yellow-green.
fn shimmer_tint(t: f32) -> [f32; 4] {
    [
        lerp(0.55, 1.05, t) * 1.4,
        lerp(0.95, 1.05, t) * 1.5,
        lerp(0.15, 0.35, t) * 1.4,
        1.0,
    ]
}

/// Build the orb's quads. `hover` is the eased 0..=1 grow amount, `shimmer` the
/// 0..=1 colour phase, and `bob` a -1..=1 vertical oscillation.
pub fn build(
    tex: &Sprites,
    orb: &Orb,
    scale: u32,
    win_w: u32,
    win_h: u32,
    hover: f32,
    shimmer: f32,
    bob: f32,
) -> Vec<Quad> {
    let size = (SPRITE_PX * scale) as f32;
    let bob_off = bob * (BOB_PX * scale) as f32;
    let cx = win_w as f32 * 0.5;
    let cy = win_h as f32 * 0.5;
    let x = cx - size * 0.5;
    let y = cy - size * 0.5 + bob_off;

    let uv = icon_uv(icon_for(orb.amount));
    let mut quads = vec![Quad::sub(tex.orb, x, y, size, size, uv, shimmer_tint(shimmer))];

    // Grow the whole orb about the window centre on hover, like the book.
    let mul = 1.0 + hover * (MAX_MUL - 1.0);
    anim::scale_quads_about(&mut quads, cx, cy, mul);
    quads
}

/// Label height (drawn glyph cell, physical px) at the given sprite scale.
fn label_cell(scale: u32) -> f32 {
    BitmapFont::cell_px() * (scale as f32) * LABEL_SCALE
}

/// Pixel size (physical) of one "pop": the sprite plus a gap plus the measured
/// label. The footprint is kind-independent, so this needs no [`Kind`]. Uses the
/// shared static font, so the snapshot can pick a canvas that frames the whole
/// thing before any [`Gpu`] exists.
pub fn pop_size(text: &str, scale: u32) -> (u32, u32) {
    let sprite = (SPRITE_PX * scale) as f32;
    let label_w = overlay_core::bitmap_font::shared().measure(text, (scale as f32) * LABEL_SCALE);
    let w = sprite + (GAP_PX * scale) as f32 + label_w;
    (w.ceil() as u32, sprite.ceil() as u32)
}

/// Build one announcement "pop" for `kind`: the sprite with its top-left at
/// `(x, y)` plus its label to the right, vertically centred, both scaled by
/// `alpha` (1.0 opaque, 0.0 invisible). `shimmer` is the 0..=1 colour phase (used
/// only by the orb). Shared by the feed overlay and the labelled snapshot so they
/// cannot drift.
#[allow(clippy::too_many_arguments)]
pub fn build_pop(
    gpu: &Gpu,
    tex: &Sprites,
    kind: Kind,
    text: &str,
    amount: i64,
    scale: u32,
    x: f32,
    y: f32,
    alpha: f32,
    shimmer: f32,
    out: &mut Vec<Quad>,
) {
    let a = alpha.clamp(0.0, 1.0);
    let size = (SPRITE_PX * scale) as f32;
    let (handle, uv, mut tint) = tex.draw(kind, amount, shimmer);
    tint[3] *= a;
    out.push(Quad::sub(handle, x, y, size, size, uv, tint));

    let label_scale = (scale as f32) * LABEL_SCALE;
    let tx = x + size + (GAP_PX * scale) as f32;
    let ty = y + (size - label_cell(scale)) * 0.5;
    let fg = [1.0, 1.0, 1.0, a];
    let mut shadow = SHADOW;
    shadow[3] *= a;
    gpu.text_shadow(text, tx, ty, label_scale, fg, shadow, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_grows_with_amount() {
        assert_eq!(icon_for(0), 0);
        assert_eq!(icon_for(1), 0);
        assert_eq!(icon_for(7), 2);
        assert_eq!(icon_for(2477), 10);
        assert_eq!(icon_for(i64::MAX), 10);
    }

    #[test]
    fn icon_uv_stays_in_unit_square() {
        for i in 0..16 {
            let [u0, v0, u1, v1] = icon_uv(i);
            assert!((0.0..=1.0).contains(&u0) && (0.0..=1.0).contains(&v1));
            assert!(u1 > u0 && v1 > v0);
        }
    }

    #[test]
    fn window_is_square_and_holds_the_grown_orb() {
        let (w, h) = orb_window_px(4);
        assert_eq!(w, h);
        assert!(w >= (16 * 4) + 2, "leaves margin around the orb");
    }

    #[test]
    fn kind_wire_form_round_trips() {
        for k in [Kind::Orb, Kind::Villager] {
            assert_eq!(Kind::parse(k.as_str()), Some(k));
        }
        assert_eq!(Kind::parse("nope"), None);
        assert_eq!(Kind::default(), Kind::Orb);
    }
}
