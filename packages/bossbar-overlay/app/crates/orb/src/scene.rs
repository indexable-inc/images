//! Turning an [`Orb`] into textured quads: pick the vanilla orb icon for its XP
//! amount, tint the grey sprite a pulsing green-yellow (the shimmer the real orb
//! has), and let the caller bob and grow it. The orb art is one 64x64 sheet of
//! 16x16 icons in a 4x4 grid.

use overlay_core::{anim, Gpu, Quad, TexHandle};

use crate::assets;
use crate::orb::Orb;

/// Source cell size (px) in the orb sheet, the sheet width (px), and its columns.
const ORB_PX: u32 = 16;
const SHEET: f32 = 64.0;
const COLS: u32 = 4;

/// Largest the orb grows to on hover.
pub const MAX_MUL: f32 = 1.18;

/// Bob amplitude in source pixels: the orb drifts +/- this (times `scale`) about
/// its resting centre. Small, so it reads as a gentle float, not a bounce.
const BOB_PX: u32 = 2;

/// The registered orb texture handle.
pub struct OrbTexture {
    orb: TexHandle,
}

pub fn register(gpu: &mut Gpu) -> OrbTexture {
    OrbTexture {
        orb: gpu.register_png(assets::EXPERIENCE_ORB),
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
    let s = ORB_PX as f32;
    let x = (i % COLS) as f32 * s;
    let y = (i / COLS) as f32 * s;
    [x / SHEET, y / SHEET, (x + s) / SHEET, (y + s) / SHEET]
}

/// Square window (physical px) holding the orb at its hovered size plus room for
/// the bob, kept tight so the desktop stays click-through around it.
pub fn orb_window_px(scale: u32) -> (u32, u32) {
    let orb = ORB_PX * scale;
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
    tex: &OrbTexture,
    orb: &Orb,
    scale: u32,
    win_w: u32,
    win_h: u32,
    hover: f32,
    shimmer: f32,
    bob: f32,
) -> Vec<Quad> {
    let size = (ORB_PX * scale) as f32;
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
}
