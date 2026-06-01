//! Lay each boss bar out as the same stack of [`Quad`]s Minecraft's
//! `BossHealthOverlay` draws: a color background, a color progress layer cropped
//! to the fill, then the optional notch background and notch progress, with the
//! title above in the vanilla bitmap font. A hovered bar can grow, breathe, and
//! unfold a description panel beneath it.
//!
//! This is the boss bar's domain layer on top of [`overlay_core`]: it owns no GPU
//! state and decodes no font. It registers its sprite textures once into the
//! shared [`Gpu`] and otherwise just builds a `Vec<Quad>` that the live window
//! ([`crate::overlay`]) or the headless snapshot ([`crate::snapshot`]) paints.

use std::collections::HashMap;

use overlay_core::{Gpu, Quad, TexHandle, SHADOW};

use crate::assets;
use crate::bars::{BossBar, Color, Notch};

/// Native vanilla sprite dimensions, in unscaled pixels.
const BAR_W: u32 = 182;
const BAR_H: u32 = 5;

/// Default opacity, matching the old CSS `--bar-opacity`: the HUD reads as an
/// overlay by letting the desktop bleed through a little.
pub const DEFAULT_OPACITY: f32 = 0.85;

/// How much a fully-hovered bar grows, before breathing. A small, deliberate
/// scale-up (on top of going opaque) so the hover is unmistakable.
pub const HOVER_SCALE: f32 = 1.06;

/// Breathing amplitude: a hovered bar gently scales +/- this fraction around its
/// grown size on a slow sine, so it reads as alive rather than frozen.
pub const BREATHE_AMP: f32 = 0.02;

/// Largest scale a bar can reach (grown + breathing in). Each window reserves
/// this much headroom so the bar grows and breathes in place without the window
/// resizing or shifting.
const MAX_SCALE: f32 = HOVER_SCALE * (1.0 + BREATHE_AMP);

/// White tint: a sprite shows its texture unchanged.
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Description pop-down panel, in native (unscaled) pixels. Everything is
/// multiplied by the integer sprite `scale`, so the panel stays pixel-crisp and
/// proportional to the bars at any display scale.
mod panel {
    /// Body glyph size (source px) and line advance (leading). The face matches
    /// the title's bitmap font; the extra leading gives wrapped paragraphs room.
    pub const FONT: f32 = 8.0;
    pub const LINE: f32 = 10.0;
    /// Inner text padding and the flat one-pixel border frame.
    pub const PAD: f32 = 5.0;
    pub const BORDER: f32 = 1.0;
    /// Gap between the bar's reserved (hover-headroom) area and the panel top.
    pub const GAP: f32 = 3.0;
    /// Flat dark-slate fill, kept slightly translucent so the desktop bleeds
    /// through like the bars. Straight (non-premultiplied) RGBA in 0..=1.
    pub const BG: [f32; 4] = [0x12 as f32 / 255.0, 0x0f as f32 / 255.0, 0x1a as f32 / 255.0, 0.92];
    /// Border opacity; its RGB comes from the bar color's accent.
    pub const BORDER_ALPHA: f32 = 0.95;
}

/// Straight-alpha RGBA in 0..=1 from an 8-bit RGB triple and an alpha.
fn rgba(rgb: [u8; 3], a: f32) -> [f32; 4] {
    [
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        a,
    ]
}

/// Smoothstep ramp: 0 below `lo`, 1 above `hi`, eased between. Lets the panel
/// text fade in only after the box has begun to unfold.
fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Compact elapsed time that ticks every second: "M:SS" under an hour, else
/// "H:MM:SS". Drives the live counter a bar with a `since` shows in its title.
fn fmt_elapsed(secs: i64) -> String {
    let secs = secs.max(0);
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = secs / 3600;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A bar's on-screen title: the stored title plus a live elapsed counter when the
/// bar has a `since` (e.g. "US East: VMs (2:05)"). An empty title stays empty, so
/// a counter never appears on its own.
fn title_with_elapsed(title: &str, since: Option<i64>, now_unix: i64) -> String {
    match since {
        Some(start) if !title.is_empty() => format!("{title} ({})", fmt_elapsed(now_unix - start)),
        _ => title.to_string(),
    }
}

/// Which preloaded sprite a bar layer samples.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum TexId {
    ColorBg(Color),
    ColorFill(Color),
    NotchBg(Notch),
    NotchFill(Notch),
}

/// The boss bar sprite textures, registered once into the shared [`Gpu`]. Flat
/// fills and borders use [`Gpu::white`], so no solid texture is tracked here.
pub struct BarTextures {
    sprites: HashMap<TexId, TexHandle>,
}

impl BarTextures {
    fn get(&self, id: TexId) -> TexHandle {
        self.sprites[&id]
    }
}

/// Register every color and notch sprite into `gpu`. Called once per `Gpu`, the
/// same way the live overlay and the snapshot each build their own engine.
pub fn register(gpu: &mut Gpu) -> BarTextures {
    let mut sprites = HashMap::new();
    for c in assets::COLORS {
        let (bg, fill) = assets::color_sprites(c);
        sprites.insert(TexId::ColorBg(c), gpu.register_png(bg));
        sprites.insert(TexId::ColorFill(c), gpu.register_png(fill));
    }
    for n in assets::NOTCHES {
        let (bg, fill) = assets::notch_sprites(n);
        sprites.insert(TexId::NotchBg(n), gpu.register_png(bg));
        sprites.insert(TexId::NotchFill(n), gpu.register_png(fill));
    }
    BarTextures { sprites }
}

/// Physical-pixel geometry of one laid-out bar, in the target's local space.
#[derive(Clone, Copy)]
struct BarBox {
    left: f32,
    title_top: f32,
    track_y: f32,
    bar_w: f32,
    bar_h: f32,
    title_px: f32,
    has_title: bool,
}

/// Physical-pixel geometry of the description pop-down panel, plus its current
/// reveal. The box unfolds downward as `reveal` goes 0..1; the text fades in via
/// `text_alpha` slightly behind it. Width matches the bar.
#[derive(Clone, Copy)]
struct PanelBox {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    border: f32,
    pad: f32,
    font_px: f32,
    line_px: f32,
    /// Vertical unfold of the box in `0..=1`, anchored at the top edge.
    reveal: f32,
    /// Text fade in `0..=1`, lagged behind `reveal` so the box opens first.
    text_alpha: f32,
    /// Border RGB (the bar color's accent), 0..=255; the fill is [`panel::BG`].
    border_rgb: [u8; 3],
}

/// One bar to paint: which bar, its box in target-local pixels, its opacity, and
/// an optional description panel unfolding beneath it.
struct DrawItem<'a> {
    bar: &'a BossBar,
    geom: BarBox,
    alpha: f32,
    panel: Option<PanelBox>,
}

/// Append one draw item's quads (bar layers, title, optional panel) to `quads`.
fn build_item(gpu: &Gpu, tex: &BarTextures, scale: u32, now_unix: i64, item: &DrawItem<'_>, quads: &mut Vec<Quad>) {
    let b = item.bar;
    let bx = item.geom;
    let alpha = item.alpha;
    // Bars sample real sprites, so the tint is white with the bar's opacity; only
    // the alpha channel matters for them.
    let tint = [1.0, 1.0, 1.0, alpha];
    let shadow_off = scale.max(1) as f32;

    if bx.has_title {
        // The title carries the live elapsed counter when the bar has a `since`;
        // recomputed every frame so it ticks on the overlay's own redraws, with
        // no DB write to advance it.
        let shown = title_with_elapsed(&b.title, b.since, now_unix);
        // Bitmap glyphs are 8 source px tall, so a title row of `title_px` px
        // means a scale of `title_px / 8`.
        let glyph_scale = bx.title_px / 8.0;
        let text_w = gpu.measure(&shown, glyph_scale);
        // Center the title within the bar width.
        let tx = bx.left + (bx.bar_w - text_w) * 0.5;
        let shadow = with_alpha(SHADOW, alpha);
        let fg = with_alpha(WHITE, alpha);
        // The shadow offset is a fixed (unscaled) pixel, matching the old
        // glyphon path which offset by `self.scale`, not the grown glyph scale.
        let _ = gpu.text(&shown, tx + shadow_off, bx.title_top + shadow_off, glyph_scale, shadow, quads);
        let _ = gpu.text(&shown, tx, bx.title_top, glyph_scale, fg, quads);
    }

    // Color background, then color progress cropped to the fill.
    quads.push(Quad::new(tex.get(TexId::ColorBg(b.color)), bx.left, bx.track_y, bx.bar_w, bx.bar_h, tint));
    if b.progress > 0.0 {
        quads.push(Quad::sub(
            tex.get(TexId::ColorFill(b.color)),
            bx.left,
            bx.track_y,
            bx.bar_w * b.progress,
            bx.bar_h,
            [0.0, 0.0, b.progress, 1.0],
            tint,
        ));
    }
    // Optional notch overlay on top, same draw order.
    if let Some(n) = b.overlay.notch() {
        quads.push(Quad::new(tex.get(TexId::NotchBg(n)), bx.left, bx.track_y, bx.bar_w, bx.bar_h, tint));
        if b.progress > 0.0 {
            quads.push(Quad::sub(
                tex.get(TexId::NotchFill(n)),
                bx.left,
                bx.track_y,
                bx.bar_w * b.progress,
                bx.bar_h,
                [0.0, 0.0, b.progress, 1.0],
                tint,
            ));
        }
    }

    // Description pop-down: a flat bordered box that unfolds downward, with the
    // wrapped paragraph fading in behind it.
    if let Some(p) = item.panel.filter(|p| p.reveal > 0.001) {
        let white = gpu.white();
        let revealed_h = (p.h * p.reveal).max(0.0);
        // Border frame first, then the fill inset by the border. While unfolding
        // (revealed_h < 2*border) only the accent strip shows, so the box reads as
        // opening from a thin line.
        quads.push(Quad::new(
            white,
            p.x,
            p.y,
            p.w,
            revealed_h,
            rgba(p.border_rgb, panel::BORDER_ALPHA),
        ));
        let inner_x = p.x + p.border;
        let inner_w = (p.w - 2.0 * p.border).max(0.0);
        let inner_y = p.y + p.border;
        let inner_h = (revealed_h - 2.0 * p.border).max(0.0);
        quads.push(Quad::new(white, inner_x, inner_y, inner_w, inner_h, panel::BG));

        if p.text_alpha > 0.001 && !b.description.trim().is_empty() {
            let text_w = (p.w - 2.0 * (p.border + p.pad)).max(1.0);
            let glyph_scale = p.font_px / 8.0;
            // overlay-core has no text clipping, so the reveal is approximated by
            // the text fade alone (it fades in as the box unfolds); the box still
            // unfolds via `revealed_h`. Accepted simplification.
            let fg = with_alpha(WHITE, p.text_alpha);
            let shadow = with_alpha(SHADOW, p.text_alpha);
            let mut ty = inner_y + p.pad;
            let tx = inner_x + p.pad;
            for line in wrap(gpu, &b.description, text_w, glyph_scale) {
                if !line.is_empty() {
                    let _ = gpu.text(&line, tx + shadow_off, ty + shadow_off, glyph_scale, shadow, quads);
                    let _ = gpu.text(&line, tx, ty, glyph_scale, fg, quads);
                }
                ty += p.line_px;
            }
        }
    }
}

/// Scale a straight-alpha RGBA's alpha channel by `mul`.
fn with_alpha(c: [f32; 4], mul: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * mul]
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

/// Panel metrics in physical pixels at `scale`: `(border, pad, font, line, gap)`.
fn panel_metrics(scale: u32) -> (f32, f32, f32, f32, f32) {
    let s = scale.max(1) as f32;
    (
        panel::BORDER * s,
        panel::PAD * s,
        panel::FONT * s,
        panel::LINE * s,
        panel::GAP * s,
    )
}

/// Physical-pixel size `(width, height)` of the description panel for
/// `description` at `scale`: width matches the bar, height fits the wrapped,
/// padded text. `None` for an empty description (no panel).
fn panel_size(gpu: &Gpu, description: &str, scale: u32) -> Option<(f32, f32)> {
    if description.trim().is_empty() {
        return None;
    }
    let (border, pad, font_px, line_px, _gap) = panel_metrics(scale);
    let panel_w = BAR_W as f32 * scale.max(1) as f32;
    let text_w = (panel_w - 2.0 * (border + pad)).max(1.0);
    let lines = wrap(gpu, description, text_w, font_px / 8.0).len().max(1);
    let panel_h = 2.0 * (border + pad) + lines as f32 * line_px;
    Some((panel_w, panel_h))
}

/// Physical-pixel advance width of `bar`'s on-screen title (the stored title plus
/// its live elapsed counter) at the fully grown hover scale, where it is widest,
/// or 0 when the bar has no title. [`bar_window_px`] reserves at least this width
/// so a title longer than the 182px bar is never clipped at the window edge. It
/// measures through the shared CPU font, so the first window can be sized before
/// any GPU surface exists.
pub fn title_extent_px(bar: &BossBar, scale: u32, now_unix: i64) -> f32 {
    if bar.title.is_empty() {
        return 0.0;
    }
    let shown = title_with_elapsed(&bar.title, bar.since, now_unix);
    // The grown title row is `8 * scale * MAX_SCALE` px tall, so each glyph samples
    // at `scale * MAX_SCALE` source-px (the `title_px / 8` of `build_one`, maxed).
    let glyph_scale = scale.max(1) as f32 * MAX_SCALE;
    overlay_core::bitmap_font::shared().measure(&shown, glyph_scale)
}

/// Physical-pixel size of the window that holds one bar at `scale` (base scale
/// times the display factor), including the [`HOVER_SCALE`] headroom so a hovered
/// bar grows in place. `title_w` is the grown title advance (see
/// [`title_extent_px`]); the window widens to hold a title longer than the bar,
/// and a nonzero `title_w` adds the title row. Plus a one-pixel-scaled shadow
/// margin on the right and bottom.
pub fn bar_window_px(scale: u32, title_w: f32) -> (u32, u32) {
    let s = scale.max(1) as f32 * MAX_SCALE;
    let bar_w = BAR_W as f32 * s;
    let bar_h = BAR_H as f32 * s;
    let has_title = title_w > 0.0;
    let title = if has_title { 8.0 * s + 1.0 * s } else { 0.0 };
    let shadow = scale.max(1) as f32;
    // Hold whichever is wider, the bar sprite or the title text; both trail the
    // one-pixel shadow down-right, so the shadow margin covers the right edge too.
    let content_w = bar_w.max(title_w) + shadow;
    (content_w.ceil() as u32, (title + bar_h + shadow).ceil() as u32)
}

/// Physical-pixel window size for `bar` with its hover panel open: the collapsed
/// bar window grown downward by the gap plus the panel. Returns the collapsed
/// size when the bar has no description. The overlay grows the window to this on
/// hover so the panel has room to unfold.
pub fn expanded_window_px(gpu: &Gpu, bar: &BossBar, scale: u32, now_unix: i64) -> (u32, u32) {
    let (cw, ch) = bar_window_px(scale, title_extent_px(bar, scale, now_unix));
    match panel_size(gpu, &bar.description, scale) {
        Some((panel_w, panel_h)) => {
            let gap = panel::GAP * scale.max(1) as f32;
            (
                cw.max(panel_w.ceil() as u32),
                ch + (gap + panel_h).ceil() as u32,
            )
        }
        None => (cw, ch),
    }
}

/// Build the quads for one bar centered in its own window. `hover` is the eased
/// hover amount (0 = resting, 1 = fully hovered); `breathe` is a sine in `-1..1`
/// for the idle breathing. Together they grow the bar by up to [`MAX_SCALE`] and
/// fade it to opaque; at `hover == 0` the bar is base size and translucent and
/// the hover headroom is transparent margin. The window size must come from
/// [`bar_window_px`] so the grown bar fits without resizing.
///
/// When the bar has a description and the window has grown tall enough (the
/// overlay enlarges it on hover, see [`expanded_window_px`]), a description panel
/// unfolds beneath the bar, revealing with `hover`.
#[allow(clippy::too_many_arguments)]
pub fn build_one(
    gpu: &Gpu,
    tex: &BarTextures,
    scale: u32,
    width: u32,
    height: u32,
    now_unix: i64,
    bar: &BossBar,
    hover: f32,
    breathe: f32,
) -> Vec<Quad> {
    let scale = scale.max(1);
    let opacity = DEFAULT_OPACITY;
    let hover = hover.clamp(0.0, 1.0);
    // Grow toward HOVER_SCALE with hover, then breathe around that; the breathe
    // fades in with hover so a resting bar is perfectly still.
    let grow = 1.0 + (HOVER_SCALE - 1.0) * hover;
    let scale_mul = grow * (1.0 + BREATHE_AMP * breathe * hover);
    let alpha = opacity + (1.0 - opacity) * hover;
    let s = scale as f32 * scale_mul;
    let shadow = scale as f32;
    let has_title = !bar.title.is_empty();
    let title_px = 8.0 * s;
    let title_h = if has_title { title_px } else { 0.0 };
    let title_gap = if has_title { 1.0 * s } else { 0.0 };
    let bar_w = BAR_W as f32 * s;
    let bar_h = BAR_H as f32 * s;

    // The bar lives in the top region: the collapsed window size, which holds it
    // plus its grow/breathe headroom. Any extra window height below that is the
    // panel's drop area, so the bar stays put as the panel unfolds. Only the
    // height is read here, so the title width need not be exact.
    let collapsed_h = bar_window_px(scale, title_extent_px(bar, scale, now_unix)).1 as f32;
    let top_region_h = collapsed_h.min(height as f32);

    // Center the content (plus its shadow offset) in the top region so growth on
    // hover expands evenly from the middle rather than shifting a corner.
    let content_w = bar_w + shadow;
    let content_h = title_h + title_gap + bar_h + shadow;
    let left = ((width as f32 - content_w) * 0.5).max(0.0);
    let top = ((top_region_h - content_h) * 0.5).max(0.0);

    let geom = BarBox {
        left,
        title_top: top,
        track_y: top + title_h + title_gap,
        bar_w,
        bar_h,
        title_px,
        has_title,
    };

    // Only build the panel when the bar opts into the box (`expandable`) and the
    // window was actually grown for it; a collapsed window (height ==
    // top_region_h) has no room and skips it. The expandable check is also the
    // live gate (via the overlay's window sizing); repeating it here keeps a
    // non-expandable bar boxless even if something grows its window (e.g. a
    // snapshot rendered at expanded size).
    let panel = if bar.expandable && height as f32 > collapsed_h + 0.5 {
        panel_size(gpu, &bar.description, scale).map(|(panel_w, panel_h)| {
            let (border, pad, font_px, line_px, gap) = panel_metrics(scale);
            PanelBox {
                x: ((width as f32 - panel_w) * 0.5).max(0.0),
                y: collapsed_h + gap,
                w: panel_w,
                h: panel_h,
                border,
                pad,
                font_px,
                line_px,
                reveal: hover,
                text_alpha: ramp(hover, 0.35, 1.0),
                border_rgb: bar.color.accent_rgb(),
            }
        })
    } else {
        None
    };

    let item = DrawItem {
        bar,
        geom,
        alpha,
        panel,
    };
    let mut quads = Vec::new();
    build_item(gpu, tex, scale, now_unix, &item, &mut quads);
    quads
}

/// Build the quads for every bar auto-stacked down the top-center column (the
/// `--snapshot` PNG path). `highlight` is the id of a bar to paint opaque. Every
/// bar with a description shows its panel fully open, so the snapshot verifies the
/// pop-down the live overlay only reveals on hover.
#[allow(clippy::too_many_arguments)]
pub fn build_all(
    gpu: &Gpu,
    tex: &BarTextures,
    scale: u32,
    width: u32,
    now_unix: i64,
    bars: &[BossBar],
    highlight: Option<i64>,
) -> Vec<Quad> {
    let scale = scale.max(1);
    let opacity = DEFAULT_OPACITY;
    let (border, pad, font_px, line_px, gap) = panel_metrics(scale);
    // A non-expandable bar has no box, so it reserves no panel size (and so no
    // layout gap and no panel quads), matching the live per-window path.
    let sizes: Vec<Option<(f32, f32)>> = bars
        .iter()
        .map(|b| {
            if b.expandable {
                panel_size(gpu, &b.description, scale)
            } else {
                None
            }
        })
        .collect();
    let boxes = layout(scale, bars, width as f32, &sizes, gap);

    let mut quads = Vec::new();
    for ((bar, geom), size) in bars.iter().zip(boxes).zip(&sizes) {
        let panel = size.map(|(panel_w, panel_h)| PanelBox {
            x: ((width as f32 - panel_w) * 0.5).max(0.0),
            y: geom.track_y + geom.bar_h + gap,
            w: panel_w,
            h: panel_h,
            border,
            pad,
            font_px,
            line_px,
            reveal: 1.0,
            text_alpha: 1.0,
            border_rgb: bar.color.accent_rgb(),
        });
        let item = DrawItem {
            bar,
            geom,
            alpha: if Some(bar.id) == highlight { 1.0 } else { opacity },
            panel,
        };
        build_item(gpu, tex, scale, now_unix, &item, &mut quads);
    }
    quads
}

/// Physical-pixel geometry of every bar auto-stacked down the top-center column,
/// in draw order. `panels` is the per-bar panel size (parallel to `bars`); a bar
/// with a panel reserves `gap + panel_h` extra vertical space so the next bar
/// clears it.
fn layout(
    scale: u32,
    bars: &[BossBar],
    width: f32,
    panels: &[Option<(f32, f32)>],
    gap: f32,
) -> Vec<BarBox> {
    let s = scale.max(1) as f32;
    let bar_w = BAR_W as f32 * s;
    let bar_h = BAR_H as f32 * s;
    let title_px = 8.0 * s;
    let title_gap = 1.0 * s;
    let bar_gap = 4.0 * s;
    let top_pad = 16.0 * s;
    let center_x = (width - bar_w) * 0.5;

    let mut boxes = Vec::with_capacity(bars.len());
    let mut y = top_pad;
    for (b, panel) in bars.iter().zip(panels) {
        let has_title = !b.title.is_empty();
        let title_h = if has_title { title_px } else { 0.0 };
        let track_y = y + title_h + if has_title { title_gap } else { 0.0 };
        boxes.push(BarBox {
            left: center_x,
            title_top: y,
            track_y,
            bar_w,
            bar_h,
            title_px,
            has_title,
        });
        // Advance past whatever this bar drew last: its panel bottom when it has
        // one (sitting `gap` below the bar), otherwise the bar bottom.
        let bottom = match panel {
            Some((_, panel_h)) => track_y + bar_h + gap + panel_h,
            None => track_y + bar_h,
        };
        y = bottom + bar_gap;
    }
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bars::Overlay;

    #[test]
    fn fmt_elapsed_ticks_in_seconds_then_minutes_then_hours() {
        assert_eq!(fmt_elapsed(0), "0:00");
        assert_eq!(fmt_elapsed(5), "0:05");
        assert_eq!(fmt_elapsed(65), "1:05");
        assert_eq!(fmt_elapsed(600), "10:00");
        assert_eq!(fmt_elapsed(3661), "1:01:01");
        // A clock that ran backwards (since in the future) clamps to zero.
        assert_eq!(fmt_elapsed(-10), "0:00");
    }

    #[test]
    fn title_with_elapsed_only_appends_when_counting() {
        // since=1000, now=1125 -> 125s = 2:05.
        assert_eq!(
            title_with_elapsed("US East: VMs", Some(1000), 1125),
            "US East: VMs (2:05)"
        );
        // No `since` leaves the title untouched.
        assert_eq!(title_with_elapsed("Idle", None, 1125), "Idle");
        // A counter never appears on its own when there is no title.
        assert_eq!(title_with_elapsed("", Some(1000), 1125), "");
    }

    fn bar_with_title(title: &str, since: Option<i64>) -> BossBar {
        BossBar {
            id: 1,
            title: title.to_string(),
            description: String::new(),
            since,
            url: String::new(),
            progress: 0.5,
            color: Color::Purple,
            overlay: Overlay::None,
            position: 0,
            pos: None,
            expandable: true,
        }
    }

    /// The whole point of the title-aware window: a title longer than the 182px
    /// bar must fit inside the window the overlay reserves for it, at the widest
    /// the bar ever draws (fully hovered and breathing in). This mirrors the
    /// placement math in `build_one`, so a regression there that re-clips the
    /// title (the bug this fixes) trips the assert. The pre-fix `bar_window_px`
    /// sized to the bar alone, so this would have failed.
    #[test]
    fn long_title_fits_reserved_window() {
        // A real downtime title plus a live H:MM:SS counter, far wider than 182px.
        let bar = bar_with_title("US East: Health lifecycle image", Some(0));
        let now = 6815; // 1:53:35

        for scale in [1u32, 2, 3, 4] {
            let text_w = title_extent_px(&bar, scale, now);
            let (width, _h) = bar_window_px(scale, text_w);
            let width = width as f32;

            // The title overflows the bar, so the window must be widened for it.
            let s_max = scale as f32 * MAX_SCALE;
            let bar_w = BAR_W as f32 * s_max;
            assert!(
                text_w > bar_w,
                "test title should exceed the bar at scale {scale}: {text_w} vs {bar_w}",
            );
            let bar_only = bar_window_px(scale, 0.0).0 as f32;
            assert!(width > bar_only, "window must grow past bar-only at scale {scale}");

            // Reproduce build_one's widest layout (hover = breathe = 1 → MAX_SCALE).
            let shadow = scale as f32;
            let content_w = bar_w + shadow;
            let left = ((width - content_w) * 0.5).max(0.0);
            let tx = left + (bar_w - text_w) * 0.5;
            // Drawn extent runs from the foreground left edge to the shadow's right
            // edge (one scaled pixel past the glyphs). Both must lie in [0, width].
            assert!(tx >= -0.01, "title left edge clipped at scale {scale}: tx={tx}");
            let right = tx + text_w + shadow;
            assert!(
                right <= width + 0.01,
                "title right edge clipped at scale {scale}: right={right} width={width}",
            );
        }
    }

    /// Visual check (run with `--ignored`): render `build_one` for a long-titled
    /// bar into the exact window the overlay reserves, both at rest and fully
    /// grown, so the title can be eyeballed for clipping. Writes PNGs under the
    /// system temp dir and prints their paths.
    #[test]
    #[ignore = "renders PNGs for manual inspection; needs a GPU adapter"]
    fn render_long_title_window() {
        let bar = bar_with_title("US East: Health lifecycle image", Some(0));
        let now = 6815; // 1:53:35
        let scale = 3;
        let (width, height) = bar_window_px(scale, title_extent_px(&bar, scale, now));
        let dir = std::env::temp_dir();
        for (tag, hover, breathe) in [("rest", 0.0f32, 0.0f32), ("grown", 1.0, 1.0)] {
            let out = dir.join(format!("bossbar_title_{tag}_{width}x{height}.png"));
            overlay_core::snapshot::render_to_png(
                width,
                height,
                |gpu| {
                    let tex = register(gpu);
                    build_one(gpu, &tex, scale, width, height, now, &bar, hover, breathe)
                },
                &out,
            )
            .expect("render");
            println!("wrote {} ({width}x{height})", out.display());
        }
    }

    /// A bar with no title reserves no title row and stays bar-width, exactly as
    /// before, so the title-aware path does not bloat untitled bars.
    #[test]
    fn untitled_bar_is_bar_width() {
        let bar = bar_with_title("", None);
        let scale = 3;
        assert_eq!(title_extent_px(&bar, scale, 1234), 0.0);
        let s_max = scale as f32 * MAX_SCALE;
        let expected_w = (BAR_W as f32 * s_max + scale as f32).ceil() as u32;
        assert_eq!(bar_window_px(scale, 0.0).0, expected_w);
    }
}
