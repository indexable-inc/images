//! The embedded monospace faces and a glyph rasterization cache.
//!
//! Four JetBrains Mono Nerd Font faces (the base font is SIL Open Font License
//! 1.1) are baked into the binary so a render never depends on a system font:
//! the result is identical on a clean machine and in CI. Static weights give a
//! real bold rather than a synthesized one, and the Nerd Font glyphs render the
//! file-type icons that tools like git-log-pretty emit.

use std::collections::HashMap;

use color_eyre::eyre::{Result, eyre};
use fontdue::{Font, FontSettings, Metrics};

const REGULAR: &[u8] = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../fonts/JetBrainsMono-Bold.ttf");
const ITALIC: &[u8] = include_bytes!("../fonts/JetBrainsMono-Italic.ttf");
const BOLD_ITALIC: &[u8] = include_bytes!("../fonts/JetBrainsMono-BoldItalic.ttf");

/// A rasterized glyph: its placement metrics and an 8-bit coverage bitmap.
pub struct Glyph {
    pub metrics: Metrics,
    pub coverage: Vec<u8>,
}

/// The four faces, the body pixel size, and a cache of rasterized cells.
pub struct FontSet {
    faces: [Font; 4],
    px: f32,
    /// Monospace cell advance in pixels.
    pub cell_w: u32,
    /// Cell height (body size times the line-height factor) in pixels.
    pub cell_h: u32,
    /// Distance from the cell's text top to the baseline, in pixels.
    pub ascent: f32,
    /// Distance from the baseline to the text bottom (negative), in pixels.
    pub descent: f32,
    cache: HashMap<(char, usize), Glyph>,
}

impl FontSet {
    /// Load the faces at `px` body size, with `line_height` as the multiple of
    /// `px` used for the cell height.
    pub fn new(px: f32, line_height: f32) -> Result<Self> {
        let load = |bytes: &[u8]| {
            Font::from_bytes(bytes, FontSettings::default())
                .map_err(|err| eyre!("load font: {err}"))
        };
        let faces = [
            load(REGULAR)?,
            load(BOLD)?,
            load(ITALIC)?,
            load(BOLD_ITALIC)?,
        ];
        let line = faces[0]
            .horizontal_line_metrics(px)
            .ok_or_else(|| eyre!("font is missing horizontal line metrics"))?;
        let cell_w = faces[0].metrics('M', px).advance_width.ceil() as u32;
        let cell_h = (px * line_height).ceil() as u32;
        Ok(Self {
            faces,
            px,
            cell_w,
            cell_h,
            ascent: line.ascent,
            descent: line.descent,
            cache: HashMap::new(),
        })
    }

    /// The face index for a bold/italic combination: 0 regular, 1 bold, 2
    /// italic, 3 bold italic.
    #[must_use]
    pub const fn face_index(bold: bool, italic: bool) -> usize {
        match (bold, italic) {
            (false, false) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (true, true) => 3,
        }
    }

    /// The cell body size in pixels.
    #[must_use]
    pub const fn px(&self) -> f32 {
        self.px
    }

    /// A cached, rasterized glyph at the body size.
    #[allow(
        clippy::map_entry,
        reason = "the entry API would hold a &mut borrow of the cache while the rasterize call needs &self.faces; contains-then-insert keeps the borrows disjoint"
    )]
    pub fn glyph(&mut self, ch: char, face: usize) -> &Glyph {
        if !self.cache.contains_key(&(ch, face)) {
            let (metrics, coverage) = self.faces[face].rasterize(ch, self.px);
            self.cache.insert((ch, face), Glyph { metrics, coverage });
        }
        &self.cache[&(ch, face)]
    }

    /// Rasterize a glyph at an arbitrary size, bypassing the cache. Used for the
    /// title and outro cards, which are drawn at larger sizes than the body.
    #[must_use]
    pub fn rasterize_at(&self, ch: char, px: f32, face: usize) -> Glyph {
        let (metrics, coverage) = self.faces[face].rasterize(ch, px);
        Glyph { metrics, coverage }
    }

    /// The monospace advance at an arbitrary size.
    #[must_use]
    pub fn advance_at(&self, px: f32) -> f32 {
        self.faces[0].metrics('M', px).advance_width
    }

    /// The ascent at an arbitrary size, for vertically placing card text.
    #[must_use]
    pub fn ascent_at(&self, px: f32) -> f32 {
        self.faces[0]
            .horizontal_line_metrics(px)
            .map_or(px, |line| line.ascent)
    }
}
