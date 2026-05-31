//! Lay the book out as a two-page spread of [`Quad`]s.
//!
//! Minecraft ships a single-page book texture. A spread is that one page drawn
//! twice: mirrored on the left (so its spiral binding lands at the centre spine)
//! and normal on the right. Both pages share the spine in the middle, so the
//! open book is real Mojang art with no synthetic texture. Page bodies render in
//! the vanilla bitmap font; page-turn arrows sit at the bottom outer corners.

use overlay_core::{Gpu, Quad, TexHandle};

use crate::book::Book;

/// `book.png` is a 256x256 sheet; the single-page background occupies this source
/// rect (measured from the 1.21 texture), binding on its left edge.
const SHEET: f32 = 256.0;
const SRC_X: f32 = 20.0;
const SRC_Y: f32 = 1.0;
/// Page sprite size, in source (unscaled) pixels; also the spread's half width.
pub const PAGE_W: f32 = 146.0;
pub const PAGE_H: f32 = 180.0;

/// Page-turn arrow sprite size.
const ARROW_W: f32 = 23.0;
const ARROW_H: f32 = 13.0;
/// Arrow placement, in page-local source pixels: vertical row, plus the
/// backward arrow's x on the left page and the forward arrow's x on the right.
const ARROW_Y: f32 = 152.0;
const BACK_X: f32 = 18.0;
const FWD_X: f32 = 105.0;

/// Text box within a normally-drawn page (binding on the left), in source pixels:
/// the parchment runs from `TEXT_L` to `TEXT_R`, clear of the binding and the
/// outer border. A mirrored page reflects this to the page's other side.
const TEXT_L: f32 = 22.0;
const TEXT_R: f32 = 130.0;
const HEADER_TOP: f32 = 14.0;
const BODY_TOP: f32 = 30.0;
/// Line advance for the 8px font (one pixel of leading), in source pixels.
const LINE: f32 = 9.0;

/// White tint: a sprite shows its texture unchanged.
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// Near-black ink for page text; the book has no drop shadow, unlike bar titles.
const INK: [f32; 4] = [
    0x28 as f32 / 255.0,
    0x20 as f32 / 255.0,
    0x16 as f32 / 255.0,
    1.0,
];

/// The overlay's textures, registered once into the shared [`Gpu`].
pub struct BookTextures {
    book: TexHandle,
    fwd: TexHandle,
    bwd: TexHandle,
}

/// Register the book sheet and the page-turn arrows.
pub fn register(gpu: &mut Gpu) -> BookTextures {
    BookTextures {
        book: gpu.register_png(crate::assets::BOOK),
        fwd: gpu.register_png(crate::assets::PAGE_FORWARD),
        bwd: gpu.register_png(crate::assets::PAGE_BACKWARD),
    }
}

/// Physical-pixel window size that holds one spread at `scale`.
pub fn spread_window_px(scale: u32) -> (u32, u32) {
    let s = scale.max(1) as f32;
    (
        (2.0 * PAGE_W * s).ceil() as u32,
        (PAGE_H * s).ceil() as u32,
    )
}

/// Top-left of the spread within a `(win_w, win_h)` window (centred if the window
/// carries margin).
fn spread_origin(scale: u32, win_w: u32, win_h: u32) -> (f32, f32) {
    let s = scale.max(1) as f32;
    let x = ((win_w as f32 - 2.0 * PAGE_W * s) * 0.5).max(0.0);
    let y = ((win_h as f32 - PAGE_H * s) * 0.5).max(0.0);
    (x, y)
}

/// Physical-pixel rect `(x, y, w, h)` of the backward arrow (bottom-left page).
pub fn back_arrow_rect(scale: u32, win_w: u32, win_h: u32) -> (f32, f32, f32, f32) {
    let s = scale.max(1) as f32;
    let (ox, oy) = spread_origin(scale, win_w, win_h);
    (ox + BACK_X * s, oy + ARROW_Y * s, ARROW_W * s, ARROW_H * s)
}

/// Physical-pixel rect `(x, y, w, h)` of the forward arrow (bottom-right page).
pub fn fwd_arrow_rect(scale: u32, win_w: u32, win_h: u32) -> (f32, f32, f32, f32) {
    let s = scale.max(1) as f32;
    let (ox, oy) = spread_origin(scale, win_w, win_h);
    (
        ox + (PAGE_W + FWD_X) * s,
        oy + ARROW_Y * s,
        ARROW_W * s,
        ARROW_H * s,
    )
}

/// Source UV rect of the page sprite; `mirror` swaps the horizontal span so the
/// binding flips to the right edge (used for the left page of the spread).
fn page_uv(mirror: bool) -> [f32; 4] {
    let (u0, u1) = (SRC_X / SHEET, (SRC_X + PAGE_W) / SHEET);
    let (v0, v1) = (SRC_Y / SHEET, (SRC_Y + PAGE_H) / SHEET);
    if mirror {
        [u1, v0, u0, v1]
    } else {
        [u0, v0, u1, v1]
    }
}

/// Screen text box `(x, width)` for a page drawn at `page_ox`. A mirrored page
/// reflects the parchment to the page's left side.
fn text_box(page_ox: f32, scale: u32, mirror: bool) -> (f32, f32) {
    let s = scale.max(1) as f32;
    let (l, r) = if mirror {
        (PAGE_W - TEXT_R, PAGE_W - TEXT_L)
    } else {
        (TEXT_L, TEXT_R)
    };
    (page_ox + l * s, (r - l) * s)
}

/// Build the spread for `book` showing the two pages starting at `spread`.
/// `show_back`/`show_fwd` gate the arrows (drawn only when that turn is possible).
pub fn build(
    gpu: &Gpu,
    tex: &BookTextures,
    book: &Book,
    spread: usize,
    scale: u32,
    win_w: u32,
    win_h: u32,
    show_back: bool,
    show_fwd: bool,
) -> Vec<Quad> {
    let s = scale.max(1) as f32;
    let (ox, oy) = spread_origin(scale, win_w, win_h);
    let pw = PAGE_W * s;
    let ph = PAGE_H * s;

    let mut quads = Vec::new();
    // Two page sprites: left mirrored, right normal, sharing the centre spine.
    quads.push(Quad::sub(tex.book, ox, oy, pw, ph, page_uv(true), WHITE));
    quads.push(Quad::sub(tex.book, ox + pw, oy, pw, ph, page_uv(false), WHITE));

    page_content(gpu, &mut quads, book, spread, ox, oy, scale, true);
    page_content(gpu, &mut quads, book, spread + 1, ox + pw, oy, scale, false);

    if show_back {
        let (x, y, w, h) = back_arrow_rect(scale, win_w, win_h);
        quads.push(Quad::new(tex.bwd, x, y, w, h, WHITE));
    }
    if show_fwd {
        let (x, y, w, h) = fwd_arrow_rect(scale, win_w, win_h);
        quads.push(Quad::new(tex.fwd, x, y, w, h, WHITE));
    }
    quads
}

/// Draw one page's header and wrapped body, if the page exists.
fn page_content(
    gpu: &Gpu,
    quads: &mut Vec<Quad>,
    book: &Book,
    idx: usize,
    page_ox: f32,
    oy: f32,
    scale: u32,
    mirror: bool,
) {
    if idx >= book.page_count() {
        return;
    }
    let s = scale.max(1) as f32;
    let (tx, tw) = text_box(page_ox, scale, mirror);

    let header = format!("Page {} of {}", idx + 1, book.page_count());
    let hw = gpu.measure(&header, s);
    gpu.text(&header, tx + (tw - hw) * 0.5, oy + HEADER_TOP * s, s, INK, quads);

    let mut y = oy + BODY_TOP * s;
    for line in wrap(gpu, book.page(idx), tw, s) {
        if !line.is_empty() {
            gpu.text(&line, tx, y, s, INK, quads);
        }
        y += LINE * s;
    }
}

/// Greedy word-wrap `text` to `max_w` (physical px) using the font's own metrics.
/// Each `\n` ends a line; a blank segment (from `\n\n`) yields a blank line.
fn wrap(gpu: &Gpu, text: &str, max_w: f32, scale: f32) -> Vec<String> {
    let mut out = Vec::new();
    for seg in text.split('\n') {
        if seg.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in seg.split_whitespace() {
            let trial = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            // Keep the word on this line if it fits, or if the line is empty (a
            // single over-long word still has to go somewhere).
            if line.is_empty() || gpu.measure(&trial, scale) <= max_w {
                line = trial;
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}
