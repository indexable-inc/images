//! Stream rendered frames to ffmpeg and write an animated AVIF or WebP.
//!
//! Frames are rendered at full size and piped to ffmpeg as raw RGBA, then
//! lanczos-downscaled to the output width (rendering full and downscaling
//! supersamples the text for clean edges). AVIF is the primary output: AV1's
//! inter-frame compression keeps the many identical hold frames almost free, so
//! a 60fps clip stays well under GitHub's 10 MB image cap. WebP is the fallback
//! for renderers without AVIF, emitted at a lower frame rate to stay small.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use color_eyre::eyre::{Result, WrapErr, eyre};

use crate::font::FontSet;
use crate::raster::{Layout, render_frame};
use crate::scene::Frame;
use crate::theme::Theme;

/// The animation container to encode.
#[derive(Clone, Copy, Debug)]
pub enum Codec {
    /// AV1 in an AVIF container: smallest and sharpest, the primary output.
    Avif,
    /// Animated WebP: broadly supported fallback.
    Webp,
}

impl Codec {
    /// The output file extension.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Avif => "avif",
            Self::Webp => "webp",
        }
    }

    /// The ffmpeg encoder flags for this codec.
    const fn encoder_args(self) -> &'static [&'static str] {
        match self {
            Self::Avif => &[
                "-c:v",
                "libsvtav1",
                "-crf",
                "30",
                "-preset",
                "7",
                "-pix_fmt",
                "yuv420p",
                "-loop",
                "0",
                "-an",
            ],
            Self::Webp => &[
                "-c:v",
                "libwebp",
                "-lossless",
                "0",
                "-q:v",
                "50",
                "-compression_level",
                "6",
                "-preset",
                "picture",
                "-loop",
                "0",
                "-an",
            ],
        }
    }
}

/// One output's encode settings.
#[derive(Clone, Copy, Debug)]
pub struct Encoding {
    pub codec: Codec,
    /// Frame rate the rawvideo frames are fed to ffmpeg at.
    pub render_fps: u32,
    /// Frame rate ffmpeg resamples the output to (thins the WebP fallback below
    /// the AVIF frame rate).
    pub output_fps: u32,
    /// Output width in pixels; height follows the aspect ratio.
    pub width: u32,
}

/// Render every frame in `theme` and encode them to `out` per `encoding`.
pub fn encode(
    out: &Path,
    frames: &[Frame],
    theme: Theme,
    font: &mut FontSet,
    layout: &Layout,
    encoding: Encoding,
) -> Result<()> {
    let palette = theme.palette();
    let size = format!("{}x{}", layout.width, layout.height);
    let scale = format!("scale={}:-2:flags=lanczos", encoding.width);
    let render_rate = encoding.render_fps.to_string();
    let output_rate = encoding.output_fps.to_string();

    let mut child = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &size,
            "-r",
            &render_rate,
            "-i",
            "-",
        ])
        .args(["-vf", &scale, "-r", &output_rate])
        .args(encoding.codec.encoder_args())
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .wrap_err("spawn ffmpeg")?;

    // If ffmpeg dies mid-stream the write side sees a broken pipe. Record that
    // error but keep going to wait() so ffmpeg's own exit status, the real
    // cause, is what gets reported.
    let mut write_err = None;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| eyre!("ffmpeg stdin was not piped"))?;
        for frame in frames {
            let buffer = render_frame(frame, &palette, font, layout);
            if let Err(err) = stdin.write_all(&buffer) {
                write_err = Some(err);
                break;
            }
        }
    }

    let status = child.wait().wrap_err("wait for ffmpeg")?;
    if !status.success() {
        return Err(eyre!("ffmpeg exited with status {status}"));
    }
    if let Some(err) = write_err {
        return Err(err).wrap_err("write frame to ffmpeg");
    }
    Ok(())
}
