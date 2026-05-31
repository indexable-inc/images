//! Stream rendered frames to ffmpeg and write an animated WebP.
//!
//! Frames are rendered at full size and piped to ffmpeg as raw RGBA, then
//! lanczos-downscaled to the output width. WebP keeps anti-aliased monospace
//! text crisp at a fraction of a GIF's size, and `-loop 0` makes it loop on
//! GitHub. Rendering full size and downscaling supersamples the text for clean
//! edges.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use color_eyre::eyre::{Result, WrapErr, eyre};

use crate::font::FontSet;
use crate::raster::{Layout, render_frame};
use crate::scene::Frame;
use crate::theme::Theme;

/// Render every frame in `theme` and encode them to one animated WebP at `out`.
pub fn encode_webp(
    out: &Path,
    frames: &[Frame],
    theme: Theme,
    font: &mut FontSet,
    layout: &Layout,
    fps: u32,
    width: u32,
) -> Result<()> {
    let palette = theme.palette();
    let size = format!("{}x{}", layout.width, layout.height);
    let scale = format!("scale={width}:-2:flags=lanczos");

    let mut child = Command::new("ffmpeg")
        .args([
            "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &size, "-r",
            &fps.to_string(), "-i", "-",
        ])
        .args([
            "-vf", &scale, "-loop", "0", "-an", "-c:v", "libwebp", "-lossless", "0", "-q:v", "50",
            "-compression_level", "6", "-preset", "picture",
        ])
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .wrap_err("spawn ffmpeg")?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| eyre!("ffmpeg stdin was not piped"))?;
        for frame in frames {
            let buffer = render_frame(frame, &palette, font, layout);
            stdin.write_all(&buffer).wrap_err("write frame to ffmpeg")?;
        }
    }

    let status = child.wait().wrap_err("wait for ffmpeg")?;
    if !status.success() {
        return Err(eyre!("ffmpeg exited with status {status}"));
    }
    Ok(())
}
