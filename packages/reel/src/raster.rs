//! Rasterize one [`Frame`] to an RGBA buffer.
//!
//! Everything is flat: a chrome bar with three squared dots and a hairline
//! rule, then either the terminal grid or a centered card. There are no
//! gradients, shadows, or rounded corners.
//!
//! Each cell is placed at one monospace advance, so a wide glyph (CJK, emoji)
//! would overflow its column. The demo scenes are ASCII, so this never bites.

use crate::font::FontSet;
use crate::scene::{Card, Frame};
use crate::theme::{Palette, Rgb};

/// A fixed-size RGBA canvas. The background is opaque, so alpha stays 255.
pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    #[must_use]
    fn filled(width: u32, height: u32, bg: Rgb) -> Self {
        // Size the buffer in usize so a large (developer-supplied) canvas cannot
        // overflow u32 and wrap to a too-small allocation.
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[0] = bg.r;
            px[1] = bg.g;
            px[2] = bg.b;
            px[3] = 255;
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Fill an axis-aligned rectangle with an opaque color, clamping the right
    /// and bottom edges to the canvas. Callers keep `left`/`top` in range.
    fn rect(&mut self, left: u32, top: u32, right: u32, bottom: u32, color: Rgb) {
        let right = right.min(self.width);
        let bottom = bottom.min(self.height);
        for y in top..bottom {
            for x in left..right {
                let idx = (y as usize * self.width as usize + x as usize) * 4;
                self.pixels[idx] = color.r;
                self.pixels[idx + 1] = color.g;
                self.pixels[idx + 2] = color.b;
                self.pixels[idx + 3] = 255;
            }
        }
    }

    /// Alpha-blend a color onto one pixel, ignoring out-of-bounds writes.
    fn blend(&mut self, x: i32, y: i32, color: Rgb, alpha: u8) {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return;
        }
        let idx = (y as usize * self.width as usize + x as usize) * 4;
        let af = f32::from(alpha) / 255.0;
        for (offset, channel) in [color.r, color.g, color.b].into_iter().enumerate() {
            let dst = f32::from(self.pixels[idx + offset]);
            let blended = f32::from(channel).mul_add(af, dst * (1.0 - af));
            self.pixels[idx + offset] = blended.round() as u8;
        }
    }

    #[must_use]
    fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }
}

/// The constant pixel geometry shared by every frame: chrome bar, padding, and
/// the cell grid. All frames in one reel must share these dimensions.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pad: u32,
    chrome: u32,
    cell_w: u32,
    cell_h: u32,
}

impl Layout {
    /// Derive the geometry from the font metrics and the grid size.
    #[must_use]
    pub fn new(font: &FontSet, cols: usize, rows: usize) -> Self {
        let pad = (font.px() * 1.4).round() as u32;
        let chrome = (font.px() * 1.7).round() as u32;
        let cell_w = font.cell_w;
        let cell_h = font.cell_h;
        let width = pad * 2 + cols as u32 * cell_w;
        let height = chrome + pad * 2 + rows as u32 * cell_h;
        Self {
            width,
            height,
            pad,
            chrome,
            cell_w,
            cell_h,
        }
    }
}

/// Render one frame to an RGBA buffer of `layout.width * layout.height * 4`
/// bytes.
#[must_use]
pub fn render_frame(frame: &Frame, palette: &Palette, font: &mut FontSet, layout: &Layout) -> Vec<u8> {
    let mut canvas = Canvas::filled(layout.width, layout.height, palette.bg);
    draw_chrome(&mut canvas, palette, font, layout);
    match frame {
        Frame::Terminal { cells, cursor } => draw_terminal(&mut canvas, palette, font, layout, cells, *cursor),
        Frame::Card(card) => draw_card(&mut canvas, palette, font, layout, card),
    }
    canvas.into_pixels()
}

/// The flat window chrome: bar fill, three squared dots, a dim centered label,
/// and a hairline rule under the bar.
fn draw_chrome(canvas: &mut Canvas, palette: &Palette, font: &FontSet, layout: &Layout) {
    canvas.rect(0, 0, layout.width, layout.chrome, palette.chrome);
    canvas.rect(0, layout.chrome, layout.width, layout.chrome + 1, palette.rule);

    let dot = (font.px() * 0.34).round() as u32;
    let gap = (dot as f32 * 0.7).round() as u32;
    let dot_top = layout.chrome.saturating_sub(dot) / 2;
    for (slot, color) in [palette.ansi[1], palette.ansi[3], palette.ansi[2]].into_iter().enumerate() {
        let left = layout.pad + slot as u32 * (dot + gap);
        canvas.rect(left, dot_top, left + dot, dot_top + dot, color);
    }

    let label_px = font.px() * 0.62;
    let baseline = (layout.chrome as f32 - font.ascent_at(label_px)) / 2.0 + font.ascent_at(label_px);
    draw_centered(canvas, font, "index", label_px, baseline.round() as i32, 0, palette.dim);
}

/// Draw a captured terminal screen: per-cell background, glyph, underline, and
/// the cursor block.
fn draw_terminal(
    canvas: &mut Canvas,
    palette: &Palette,
    font: &mut FontSet,
    layout: &Layout,
    cells: &ndarray::Array2<tui::StyledCell>,
    cursor: crate::scene::Cursor,
) {
    let origin_x = layout.pad;
    let origin_y = layout.chrome + layout.pad;
    let cell_w = layout.cell_w;
    let cell_h = layout.cell_h;
    let baseline_off = ((cell_h as f32 - (font.ascent - font.descent)) / 2.0 + font.ascent).round() as i32;
    let (cell_rows, cell_cols) = cells.dim();

    for ((row, col), cell) in cells.indexed_iter() {
        let cell_x = origin_x + col as u32 * cell_w;
        let cell_y = origin_y + row as u32 * cell_h;

        let text = palette.resolve(cell.fg).unwrap_or(palette.fg);
        let fill = palette.resolve(cell.bg);
        let (ink, back) = if cell.inverse {
            (fill.unwrap_or(palette.bg), Some(text))
        } else {
            (text, fill)
        };

        if let Some(color) = back {
            canvas.rect(cell_x, cell_y, cell_x + cell_w, cell_y + cell_h, color);
        }
        if cell.character != ' ' {
            let face = FontSet::face_index(cell.bold, cell.italic);
            let glyph = font.glyph(cell.character, face);
            let gx = cell_x as i32 + glyph.metrics.xmin;
            let gy = cell_y as i32 + baseline_off - glyph.metrics.ymin - glyph.metrics.height as i32;
            blit(canvas, &glyph.coverage, glyph.metrics.width, glyph.metrics.height, gx, gy, ink);
        }
        if cell.underline {
            let y = cell_y + baseline_off as u32 + 1;
            canvas.rect(cell_x, y, cell_x + cell_w, y + 2, ink);
        }
    }

    if cursor.visible && (cursor.row as usize) < cell_rows && (cursor.col as usize) < cell_cols {
        let cur_x = origin_x + u32::from(cursor.col) * cell_w;
        let cur_y = origin_y + u32::from(cursor.row) * cell_h;
        canvas.rect(cur_x, cur_y, cur_x + cell_w, cur_y + cell_h, palette.accent);
        let cell = &cells[[cursor.row as usize, cursor.col as usize]];
        if cell.character != ' ' {
            let face = FontSet::face_index(cell.bold, cell.italic);
            let glyph = font.glyph(cell.character, face);
            let gx = cur_x as i32 + glyph.metrics.xmin;
            let gy = cur_y as i32 + baseline_off - glyph.metrics.ymin - glyph.metrics.height as i32;
            blit(canvas, &glyph.coverage, glyph.metrics.width, glyph.metrics.height, gx, gy, palette.bg);
        }
    }
}

/// Draw a centered title/subtitle/footer card.
fn draw_card(canvas: &mut Canvas, palette: &Palette, font: &FontSet, layout: &Layout, card: &Card) {
    let title_px = font.px() * 2.4;
    let sub_px = font.px() * 0.95;
    let foot_px = font.px() * 0.72;

    let line_h = |px: f32| -> f32 { px * 1.5 };
    let mut total = line_h(title_px);
    if card.subtitle.is_some() {
        total += line_h(sub_px);
    }
    if card.footer.is_some() {
        total += line_h(foot_px) + foot_px;
    }

    let area_top = layout.chrome as f32;
    let area_h = layout.height as f32 - area_top;
    let mut baseline = area_top + (area_h - total) / 2.0 + font.ascent_at(title_px);

    draw_centered(canvas, font, &card.title, title_px, baseline.round() as i32, FontSet::face_index(true, false), palette.fg);
    baseline += line_h(title_px);
    if let Some(subtitle) = &card.subtitle {
        draw_centered(canvas, font, subtitle, sub_px, baseline.round() as i32, 0, palette.dim);
        baseline += line_h(sub_px);
    }
    if let Some(footer) = &card.footer {
        baseline += foot_px;
        draw_centered(canvas, font, footer, foot_px, baseline.round() as i32, 0, palette.accent);
    }
}

/// Draw a string centered horizontally at a given baseline and size.
fn draw_centered(canvas: &mut Canvas, font: &FontSet, text: &str, px: f32, baseline: i32, face: usize, color: Rgb) {
    let advance = font.advance_at(px);
    let count = text.chars().count() as f32;
    let mut pen = advance.mul_add(-count, canvas.width as f32) / 2.0;
    for ch in text.chars() {
        if ch != ' ' {
            let glyph = font.rasterize_at(ch, px, face);
            let gx = pen.round() as i32 + glyph.metrics.xmin;
            let gy = baseline - glyph.metrics.ymin - glyph.metrics.height as i32;
            blit(canvas, &glyph.coverage, glyph.metrics.width, glyph.metrics.height, gx, gy, color);
        }
        pen += advance;
    }
}

/// Blit an 8-bit coverage bitmap onto the canvas in one color.
fn blit(canvas: &mut Canvas, coverage: &[u8], width: usize, height: usize, x: i32, y: i32, color: Rgb) {
    for row in 0..height {
        for col in 0..width {
            let alpha = coverage[row * width + col];
            if alpha != 0 {
                canvas.blend(x + col as i32, y + row as i32, color, alpha);
            }
        }
    }
}
