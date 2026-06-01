//! `screencast`: stream the macOS desktop to a `screencast-ingest` server as
//! hardware-encoded H.265.
//!
//! ffmpeg does the capture and encode (the same way `reel` wraps ffmpeg for
//! terminal reels); this binary configures it, supervises it, and ships its
//! output. The pipeline is:
//!
//! 1. capture the desktop through the `avfoundation` input device,
//! 2. encode it with `hevc_videotoolbox`, the Apple media-engine H.265 encoder
//!    (hardware, so it stays near-free on battery and CPU),
//! 3. mux to fragmented-MP4 HLS written to a local scratch directory, and
//! 4. upload each finalized segment plus the rolling playlist to the ingest
//!    server with ordinary sized HTTP `PUT`s.
//!
//! ffmpeg can `PUT` HLS directly, but its HTTP muxer keeps connections open and
//! does not finalize each request body in a way a strict HTTP/1.1 server
//! accepts, so segments hang server-side. Writing locally and uploading from
//! here sidesteps that and buys retries: a failed upload is simply retried on
//! the next sync, so a brief network or server blip does not drop the stream.
//!
//! The HLS playlist uses `event` type with an unbounded list, so a session is a
//! live stream while recording and a complete, replayable VOD once ffmpeg
//! finishes. Only segments the local playlist already lists are uploaded, so the
//! server's playlist never references a segment that has not landed yet.
//!
//! macOS only. Screen Recording permission must be granted to the terminal (or
//! whatever process) running this binary, or `avfoundation` captures a black
//! frame with no error.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use chrono::Utc;
use clap::Parser;
use color_eyre::eyre::{Result, bail, eyre};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tokio::signal;
use tokio::time::{MissedTickBehavior, interval, sleep};
use tracing::{error, info, warn};

/// The fixed name of the HLS playlist and init segment within a session.
const PLAYLIST: &str = "index.m3u8";
const INIT: &str = "init.mp4";

/// Capture this Mac's screen as H.265 and stream it to an ingest server.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Base URL of the `screencast-ingest` server, e.g. `http://ingest.host:8080`.
    #[arg(long, env = "SCREENCAST_SERVER")]
    server: String,

    /// Stream owner; becomes the top-level folder on the server. Defaults to
    /// `$USER`.
    #[arg(long, env = "SCREENCAST_USER")]
    user: Option<String>,

    /// `avfoundation` video device index for the display to capture. Defaults to
    /// auto-detecting the first "Capture screen" device, since the index differs
    /// per machine (device 0 is often a camera). Run with `--list-screens` to
    /// see the options.
    #[arg(long)]
    screen: Option<u32>,

    /// List the capturable `avfoundation` video devices and exit.
    #[arg(long)]
    list_screens: bool,

    /// Capture frame rate.
    #[arg(long, default_value_t = 30)]
    fps: u32,

    /// Target H.265 bitrate, passed to ffmpeg `-b:v` (e.g. `6M`, `10M`).
    #[arg(long, default_value = "6M")]
    bitrate: String,

    /// HLS segment length in seconds. Also drives the keyframe interval so
    /// segments split on clean boundaries.
    #[arg(long, default_value_t = 4)]
    segment_seconds: u32,

    /// Upload sync interval in milliseconds: how often the scratch directory is
    /// checked for finalized segments to push.
    #[arg(long, default_value_t = 1000)]
    sync_ms: u64,

    /// Downscale so the output is at most this tall (keeps aspect, even width).
    /// Retina displays are large; capping height cuts bandwidth a lot. Omit to
    /// capture at native resolution.
    #[arg(long)]
    max_height: Option<u32>,

    /// Exclude the mouse cursor from the capture.
    #[arg(long)]
    no_cursor: bool,

    /// Bearer token sent as `Authorization: Bearer <token>` on every upload, to
    /// match a server started with `--token`.
    #[arg(long, env = "SCREENCAST_TOKEN")]
    token: Option<String>,

    /// ffmpeg binary to invoke.
    #[arg(long, default_value = "ffmpeg")]
    ffmpeg: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    if args.list_screens {
        return list_screens(&args.ffmpeg).await;
    }

    preflight(&args.ffmpeg).await?;

    let user = sanitize(&args.user.clone().unwrap_or_else(default_user));
    if user.is_empty() {
        bail!("could not determine a user name; pass --user");
    }
    let server = args.server.trim_end_matches('/').to_owned();

    let screen = resolve_screen(&args).await?;
    info!(server = %server, user = %user, screen, "streaming desktop as H.265 to ingest server");

    let scratch = tempfile::tempdir().map_err(|e| eyre!("creating scratch dir: {e}"))?;
    supervise(&args, &server, &user, screen, scratch.path()).await
}

/// Resolve the avfoundation device index for the display: the explicit
/// `--screen` if given, otherwise the first auto-detected "Capture screen"
/// device. Bails with the device list if no screen is found.
async fn resolve_screen(args: &Args) -> Result<u32> {
    if let Some(n) = args.screen {
        return Ok(n);
    }
    let out = Command::new(&args.ffmpeg)
        .args(["-hide_banner", "-f", "avfoundation", "-list_devices", "true", "-i", ""])
        .output()
        .await
        .map_err(|e| eyre!("listing avfoundation devices: {e}"))?;
    let devices = String::from_utf8_lossy(&out.stderr);
    parse_screen_index(&devices).ok_or_else(|| {
        eyre!("no 'Capture screen' device found; pass --screen with one of:\n{devices}")
    })
}

/// Find the index of the first `[N] Capture screen ...` entry in ffmpeg's
/// avfoundation device listing. The line carries two bracketed fields
/// (`[AVFoundation indev @ ..] [N] Capture screen 0`); the wanted index is the
/// bracket immediately before the label.
fn parse_screen_index(devices: &str) -> Option<u32> {
    for line in devices.lines() {
        let Some(pos) = line.find("] Capture screen") else {
            continue;
        };
        let before = &line[..pos];
        if let Some(open) = before.rfind('[')
            && let Ok(n) = before[open + 1..].trim().parse::<u32>()
        {
            return Some(n);
        }
    }
    None
}

/// Default stream owner, taken from the environment.
fn default_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("SUDO_USER"))
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default()
}

/// Reduce a string to the URL- and path-safe charset the server accepts,
/// folding everything else to `-`.
fn sanitize(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Verify ffmpeg exists and exposes the hardware H.265 encoder, failing with a
/// clear message rather than a black stream or a cryptic ffmpeg error later.
async fn preflight(ffmpeg: &str) -> Result<()> {
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .await
        .map_err(|e| eyre!("could not run {ffmpeg:?} (is ffmpeg installed?): {e}"))?;
    if !out.status.success() {
        bail!("{ffmpeg} -encoders failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    if !String::from_utf8_lossy(&out.stdout).contains("hevc_videotoolbox") {
        bail!(
            "this ffmpeg has no hevc_videotoolbox encoder; rebuild ffmpeg with VideoToolbox \
             support (the nix wrapper provides one)"
        );
    }
    Ok(())
}

/// Print the `avfoundation` capture devices (the `[N] Capture screen M` lines
/// are the displays).
async fn list_screens(ffmpeg: &str) -> Result<()> {
    // `-list_devices true` writes the device table to stderr and exits non-zero
    // by design, so the status is ignored and stderr is the payload.
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-f", "avfoundation", "-list_devices", "true", "-i", ""])
        .output()
        .await
        .map_err(|e| eyre!("could not run {ffmpeg:?}: {e}"))?;
    print!("{}", String::from_utf8_lossy(&out.stderr));
    Ok(())
}

/// Run ffmpeg, restarting it under a fresh session id if it dies, until the
/// user interrupts with Ctrl-C. Each (re)start is its own server-side session so
/// a crash never reuses or corrupts an earlier session's segment numbering.
async fn supervise(args: &Args, server: &str, user: &str, screen: u32, scratch: &Path) -> Result<()> {
    let mut backoff = Duration::from_secs(1);
    let mut attempt: u32 = 0;

    loop {
        let session = format!("{}-{attempt:02}", Utc::now().format("%Y%m%dT%H%M%SZ"));
        let dir = scratch.join(&session);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| eyre!("creating session scratch {}: {e}", dir.display()))?;
        let base = format!("{server}/ingest/{user}/{session}/");
        let mut uploader = Uploader::new(&base, args.token.clone(), dir.clone());

        info!(session = %session, "starting capture");
        let mut child = Command::new(&args.ffmpeg)
            .args(build_ffmpeg_args(args, screen, &dir))
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| eyre!("failed to spawn ffmpeg: {e}"))?;
        let started = std::time::Instant::now();

        let mut tick = interval(Duration::from_millis(args.sync_ms.max(100)));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let restart = loop {
            tokio::select! {
                _ = tick.tick() => {
                    uploader.sync().await;
                }
                status = child.wait() => {
                    // Push whatever ffmpeg managed to finalize before dying.
                    uploader.sync().await;
                    error!(?status, "ffmpeg exited unexpectedly");
                    break true;
                }
                _ = signal::ctrl_c() => {
                    info!("interrupt received; stopping ffmpeg so the playlist finalizes");
                    stop_gracefully(&mut child).await;
                    uploader.sync().await; // ship the final segments and ENDLIST playlist
                    info!(uploaded = uploader.count(), "stream finalized");
                    break false;
                }
            }
        };

        if !restart {
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(30) {
            backoff = Duration::from_secs(1); // a long run was healthy; reset
        }
        warn!("retrying in {:?}", backoff);
        sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
        attempt = attempt.saturating_add(1);
    }
}

/// Ask ffmpeg to quit cleanly (write `q` to its stdin, the documented
/// interactive stop) so it flushes the final segment and writes the HLS
/// `#EXT-X-ENDLIST` that turns the session into a finished VOD. Falls back to a
/// kill if it does not exit promptly.
async fn stop_gracefully(child: &mut Child) {
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"q").await;
        let _ = stdin.flush().await;
    }
    match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
        Ok(Ok(status)) => info!(?status, "ffmpeg stopped"),
        Ok(Err(e)) => warn!("error waiting for ffmpeg: {e}"),
        Err(_) => {
            warn!("ffmpeg did not exit within 10s; killing");
            let _ = child.kill().await;
        }
    }
}

/// Uploads a session's local HLS output to the ingest server, segment by
/// segment, never re-sending a file it has already shipped.
struct Uploader {
    client: reqwest::Client,
    base: String,
    token: Option<String>,
    dir: PathBuf,
    sent: HashSet<String>,
    last_playlist: Vec<u8>,
}

impl Uploader {
    fn new(base: &str, token: Option<String>, dir: PathBuf) -> Self {
        // Bounded timeouts matter: sync() is awaited inside the supervisor's
        // select arm, so an upload that hangs would otherwise stall the whole
        // loop (no further ticks, no Ctrl-C). On timeout the PUT errors and is
        // retried on the next sync instead.
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            client,
            base: base.to_owned(),
            token,
            dir,
            sent: HashSet::new(),
            last_playlist: Vec::new(),
        }
    }

    /// Number of media files (init + segments) successfully uploaded so far.
    fn count(&self) -> usize {
        self.sent.len()
    }

    /// Upload anything new: the init segment, every media segment the local
    /// playlist now lists, then the playlist itself (only when it changed). The
    /// playlist is shipped last so the server never advertises a segment that
    /// has not been uploaded yet. Files that are listed but not yet on disk, and
    /// uploads that fail, are simply retried on the next sync.
    async fn sync(&mut self) {
        let Ok(playlist) = tokio::fs::read(self.dir.join(PLAYLIST)).await else {
            return; // ffmpeg has not written the playlist yet
        };

        // Track whether every file the playlist references is now uploaded. The
        // playlist is shipped only when that holds, so the server never
        // advertises a segment that has not landed (a failed or not-yet-flushed
        // segment defers the playlist to the next sync).
        let mut all_ready = true;
        if !self.sent.contains(INIT) {
            all_ready &= self.put_file(INIT).await;
        }
        for seg in parse_segments(&playlist) {
            if !self.sent.contains(&seg) {
                all_ready &= self.put_file(&seg).await;
            }
        }
        if all_ready
            && playlist != self.last_playlist
            && self.put_bytes(PLAYLIST, playlist.clone()).await.is_ok()
        {
            self.last_playlist = playlist;
        }
    }

    /// Read a local file and upload it, marking it sent on success. Returns
    /// whether the file is now on the server. A file that is listed in the
    /// playlist but not yet flushed to disk, or whose upload failed, returns
    /// `false` and is retried on the next sync.
    async fn put_file(&mut self, name: &str) -> bool {
        if let Ok(bytes) = tokio::fs::read(self.dir.join(name)).await
            && self.put_bytes(name, bytes).await.is_ok()
        {
            self.sent.insert(name.to_owned());
            return true;
        }
        false
    }

    /// `PUT` a body to `{base}{name}`. Returns `Err` on any transport or status
    /// failure so the caller leaves it unmarked for retry.
    async fn put_bytes(&self, name: &str, bytes: Vec<u8>) -> Result<()> {
        let mut req = self.client.put(format!("{}{name}", self.base)).body(bytes);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => Ok(()),
            Ok(resp) => {
                warn!(name, status = %resp.status(), "upload rejected");
                bail!("status {}", resp.status())
            }
            Err(e) => {
                warn!(name, "upload failed: {e}");
                Err(e.into())
            }
        }
    }
}

/// Extract the media-segment file names listed in an HLS playlist: the lines
/// that are neither blank nor directives.
fn parse_segments(playlist: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(playlist)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect()
}

/// Build the ffmpeg argument vector that captures, hardware-encodes to H.265,
/// and writes fMP4 HLS into the local `dir`.
fn build_ffmpeg_args(args: &Args, screen: u32, dir: &Path) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut push = |s: &str| a.push(s.to_owned());

    push("-hide_banner");
    push("-loglevel");
    push("warning");

    // Input: the avfoundation screen device, no audio (`:none`).
    push("-f");
    push("avfoundation");
    push("-capture_cursor");
    push(if args.no_cursor { "0" } else { "1" });
    push("-framerate");
    push(&args.fps.to_string());
    push("-i");
    push(&format!("{screen}:none"));

    // Optional downscale before encoding.
    if let Some(h) = args.max_height {
        push("-vf");
        push(&format!("scale=-2:{h}"));
    }

    // Hardware H.265. `-realtime 1` tells VideoToolbox to favor latency, and the
    // `hvc1` tag is the fMP4/Apple-compatible HEVC sample entry (without it some
    // players refuse the stream).
    push("-c:v");
    push("hevc_videotoolbox");
    push("-realtime");
    push("1");
    push("-b:v");
    push(&args.bitrate);
    push("-tag:v");
    push("hvc1");
    push("-pix_fmt");
    push("yuv420p");
    // Keyframe interval == segment length so HLS splits on IDR boundaries.
    push("-g");
    push(&(args.fps * args.segment_seconds).to_string());

    // fMP4 HLS to local disk. `event` + unbounded list keeps the whole session
    // in the playlist (live now, replayable later); ffmpeg writes ENDLIST on a
    // clean exit, marking the VOD complete.
    push("-f");
    push("hls");
    push("-hls_time");
    push(&args.segment_seconds.to_string());
    push("-hls_playlist_type");
    push("event");
    push("-hls_list_size");
    push("0");
    push("-hls_flags");
    push("independent_segments");
    push("-hls_segment_type");
    push("fmp4");
    push("-hls_fmp4_init_filename");
    push(INIT);
    push("-hls_segment_filename");
    push(&dir.join("seg_%05d.m4s").to_string_lossy());
    push(&dir.join(PLAYLIST).to_string_lossy());

    a
}

#[cfg(test)]
mod tests {
    use super::{parse_screen_index, parse_segments, sanitize};

    #[test]
    fn finds_first_capture_screen_index() {
        let listing = "[AVFoundation indev @ 0x1] AVFoundation video devices:\n\
            [AVFoundation indev @ 0x1] [0] FaceTime HD Camera\n\
            [AVFoundation indev @ 0x1] [4] Capture screen 0\n\
            [AVFoundation indev @ 0x1] [5] Capture screen 1\n";
        assert_eq!(parse_screen_index(listing), Some(4));
    }

    #[test]
    fn no_screen_device_returns_none() {
        assert_eq!(parse_screen_index("[x] [0] FaceTime HD Camera\n"), None);
    }

    #[test]
    fn parses_only_segment_lines() {
        let playlist = b"#EXTM3U\n\
            #EXT-X-VERSION:7\n\
            #EXT-X-MAP:URI=\"init.mp4\"\n\
            #EXTINF:2.000,\n\
            seg_00000.m4s\n\
            #EXTINF:2.000,\n\
            seg_00001.m4s\n\
            #EXT-X-ENDLIST\n";
        assert_eq!(parse_segments(playlist), ["seg_00000.m4s", "seg_00001.m4s"]);
    }

    #[test]
    fn empty_playlist_has_no_segments() {
        assert!(parse_segments(b"#EXTM3U\n#EXT-X-VERSION:7\n").is_empty());
    }

    #[test]
    fn sanitize_folds_unsafe_chars() {
        assert_eq!(sanitize("ok.name_-1"), "ok.name_-1");
        assert_eq!(sanitize("a/b c"), "a-b-c");
        assert_eq!(sanitize("../evil"), "..-evil");
    }
}
