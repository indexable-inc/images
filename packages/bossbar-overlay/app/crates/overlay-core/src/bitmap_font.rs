//! The vanilla Minecraft bitmap font, rendered as textured glyph quads.
//!
//! `ascii.png` (extracted from the official client jar by the `minecraft-assets`
//! Nix derivation) is a 128x128 sheet: a 16x16 grid of 8x8 glyph cells indexed
//! by code-page-437 byte value, which for `0x20..=0x7E` coincides with ASCII.
//! Glyphs are white on a transparent ground, so the per-vertex color tints them
//! directly and text composites over the desktop the same way the sprites do.
//!
//! This is the whole text stack: no TTF, no shaper. Each glyph is one quad
//! sampling its cell, so titles and page text flow through the same wgpu pipeline
//! as every sprite.

use std::sync::LazyLock;

/// `ascii.png`, extracted from the official Minecraft jar by the
/// `minecraft-assets` Nix derivation and dropped here before the build (see
/// `app/scripts/fetch-assets.sh` for the local-dev copy step). The font module
/// owns the sheet; [`crate::gpu`] borrows it for the glyph texture.
pub const ASCII_PNG: &[u8] = include_bytes!("../assets/ascii.png");

/// The basic-Latin face, decoded once from the embedded sheet. Layout that has no
/// GPU yet (sizing a window to its title before the wgpu surface exists) measures
/// through this; the live [`crate::gpu::Gpu`] shares the same metrics.
pub fn shared() -> &'static BitmapFont {
    static FONT: LazyLock<BitmapFont> = LazyLock::new(|| {
        let ascii = image::load_from_memory(ASCII_PNG)
            .expect("decode embedded ascii.png")
            .to_rgba8();
        let (w, h) = ascii.dimensions();
        BitmapFont::from_ascii_rgba(&ascii, w, h)
    });
    &FONT
}

/// Glyph cell side, in source pixels.
const CELL: u32 = 8;
/// Cells per row/column in the sheet.
const COLS: u32 = 16;
/// `ascii.png` side length, for normalizing cell coordinates to UVs.
const SHEET: f32 = 128.0;
/// Advance of the space glyph, from the vanilla `space` font provider. Its cell
/// is empty, so it cannot be measured from ink and is set explicitly.
const SPACE_ADVANCE: u32 = 4;

/// Per-glyph metrics for the basic-Latin face. Decode once, then measure and lay
/// out text cheaply.
pub struct BitmapFont {
    /// Inked pixel width of each code point `0..128`. The on-screen advance is
    /// `width + 1` (the vanilla one-pixel inter-glyph gap).
    widths: [u8; 128],
}

impl BitmapFont {
    /// Build from the decoded RGBA pixels of `ascii.png` (`width`x`height`, must
    /// cover the 16x16 cell grid). A column is inked where any pixel in it has
    /// nonzero alpha; glyph width is the rightmost inked column + 1, the same
    /// measurement the vanilla client makes for the legacy font.
    pub fn from_ascii_rgba(rgba: &[u8], width: u32, height: u32) -> Self {
        assert!(
            width >= COLS * CELL && height >= COLS * CELL,
            "ascii.png is {width}x{height}, too small for a 16x16 grid of 8px cells",
        );
        let mut widths = [0u8; 128];
        for code in 0u32..128 {
            let (cx, cy) = cell_origin(code);
            let mut w = 0u32;
            for col in 0..CELL {
                let x = cx + col;
                let inked = (0..CELL).any(|row| {
                    let y = cy + row;
                    let idx = ((y * width + x) * 4 + 3) as usize;
                    rgba.get(idx).is_some_and(|a| *a > 0)
                });
                if inked {
                    w = col + 1;
                }
            }
            widths[code as usize] = w as u8;
        }
        // Space has no ink; pin its advance so `width + 1 == SPACE_ADVANCE`.
        widths[b' ' as usize] = (SPACE_ADVANCE - 1) as u8;
        Self { widths }
    }

    fn glyph_width(&self, c: char) -> u32 {
        let code = c as u32;
        if code < 128 {
            self.widths[code as usize] as u32
        } else {
            0
        }
    }

    /// On-screen advance for `c` at `scale`: inked width plus the one-pixel gap.
    pub fn advance(&self, c: char, scale: f32) -> f32 {
        (self.glyph_width(c) + 1) as f32 * scale
    }

    /// Total advance width of `text` at `scale`.
    pub fn measure(&self, text: &str, scale: f32) -> f32 {
        text.chars().map(|c| self.advance(c, scale)).sum()
    }

    /// UV rect `(u0, v0, u1, v1)` of `c`'s cell, or `None` for a code point with
    /// no glyph (space, or anything outside the basic sheet): the caller still
    /// advances by [`Self::advance`] but draws nothing.
    pub fn glyph_uv(&self, c: char) -> Option<[f32; 4]> {
        let code = c as u32;
        if code >= 128 || self.glyph_width(c) == 0 {
            return None;
        }
        let (cx, cy) = cell_origin(code);
        Some([
            cx as f32 / SHEET,
            cy as f32 / SHEET,
            (cx + CELL) as f32 / SHEET,
            (cy + CELL) as f32 / SHEET,
        ])
    }

    /// Source side length of a glyph cell (8px); the dest quad is `cell_px() * scale`.
    pub const fn cell_px() -> f32 {
        CELL as f32
    }
}

/// Top-left pixel of `code`'s cell in the 16x16 sheet.
fn cell_origin(code: u32) -> (u32, u32) {
    ((code % COLS) * CELL, (code / COLS) * CELL)
}
