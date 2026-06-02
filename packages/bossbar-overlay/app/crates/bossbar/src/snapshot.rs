//! Headless render to a PNG. This is the overlay's observability hook: it builds
//! the exact same quads the live window draws and hands them to
//! [`overlay_core::snapshot::render_to_png`], so a transparent always-on-top
//! window (which is awkward to screenshot) is verifiable pixel-for-pixel from a
//! file. Every bar shows its description panel fully open.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::bars::BossBar;
use crate::scene;

/// Render `bars` at `scale` into a `width`x`height` transparent PNG at `out`.
pub fn run(
    scale: f32,
    width: u32,
    height: u32,
    bars: &[BossBar],
    out: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let scale = scale.max(1.0);
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    overlay_core::snapshot::render_to_png(
        width,
        height,
        |gpu| {
            let tex = scene::register(gpu);
            // Resolve each bar's icon to a texture (or None when absent/unreadable)
            // so the snapshot draws avatars exactly as the live overlay does.
            let icons: Vec<Option<overlay_core::TexHandle>> = bars
                .iter()
                .map(|b| {
                    if b.icon.is_empty() {
                        return None;
                    }
                    std::fs::read(&b.icon)
                        .ok()
                        .and_then(|bytes| gpu.register_image_scaled(&bytes, scene::ICON_MAX_PX))
                })
                .collect();
            scene::build_all(gpu, &tex, &icons, scale, width, now_unix, bars, None)
        },
        out,
    )
}
