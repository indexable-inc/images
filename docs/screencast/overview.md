# screencast

`packages/screencast` is the macOS capture client: it streams a Mac's desktop to
a [screencast-ingest](../screencast-ingest/overview.md) server as
hardware-encoded H.265, so a whole team can push screens to one place that stores
every session for replay and downstream data/context use (`README.md:3-14`). It
does not implement capture or encoding itself; it configures, supervises, and
ships the output of `ffmpeg` (`src/main.rs:1-13`).

A Rust workspace crate (`Cargo.toml`); flake output `.#screencast`.
**Darwin-only**: avfoundation capture and `hevc_videotoolbox` are macOS APIs, so
`meta.platforms = lib.platforms.darwin` (`default.nix:13-14`) and the package
advertises its flake/package outputs only on `aarch64-darwin`/`x86_64-darwin`
(`package.nix:7-14`, mirroring vmkit). The server half is cross-platform; only
this client is Mac-only (`README.md:59-60`).

## The pipeline (`src/main.rs:4-24`)

1. Capture the desktop through the avfoundation input device.
2. Encode with `hevc_videotoolbox`, Apple's hardware H.265 encoder (near-free on
   battery/CPU).
3. Mux to fragmented-MP4 HLS written to a local scratch directory.
4. Upload each finalized segment plus the rolling playlist to the ingest server
   with ordinary sized HTTP `PUT`s.

ffmpeg can `PUT` HLS directly, but its HTTP muxer keeps connections open without
finalizing each request body in a way a strict HTTP/1.1 server accepts, so
segments hang server-side. Writing locally and uploading from here sidesteps that
and buys retries: a failed upload is simply retried on the next sync, so a brief
blip does not drop the stream (`src/main.rs:15-19`, `README.md:44-52`).

The HLS playlist is `event` type with an unbounded list (`-hls_list_size 0`), so
a session is a live stream while recording and a complete VOD once ffmpeg writes
`#EXT-X-ENDLIST` (`src/main.rs:21-24,496-506`). Only segments the local playlist
already lists are uploaded, so the server never references a segment that has not
landed yet.

## Public surface: CLI flags (`src/main.rs:51-108`)

| flag | env | default | meaning |
| --- | --- | --- | --- |
| `--server` | `SCREENCAST_SERVER` | (required) | base URL of the ingest server, e.g. `http://ingest.host:8080` |
| `--user` | `SCREENCAST_USER` | `$USER` | stream owner; top-level folder on the server |
| `--screen N` | | auto | avfoundation device index; auto-detects the first `Capture screen` device |
| `--list-screens` | | | list capturable avfoundation devices and exit |
| `--fps` | | `30` | capture frame rate |
| `--bitrate` | | `6M` | target H.265 bitrate (ffmpeg `-b:v`), e.g. `10M` |
| `--segment-seconds` | | `4` | HLS segment length; also drives the keyframe interval |
| `--sync-ms` | | `1000` | how often the scratch dir is checked for finalized segments to push |
| `--max-height` | | none | downscale so output is at most this tall (cuts Retina bandwidth) |
| `--no-cursor` | | off | exclude the mouse cursor from capture |
| `--token` | `SCREENCAST_TOKEN` | none | bearer token sent as `Authorization: Bearer <token>` on every upload |
| `--ffmpeg` | | `ffmpeg` | ffmpeg binary to invoke |

Logging is `tracing` with `RUST_LOG`-style `EnvFilter`, default `info`
(`src/main.rs:113-118`). Upload URL shape is
`{server}/ingest/{user}/{session}/{file}` (`src/main.rs:272`), matching the
server's ingest route.

## Key internals

- **Screen resolution** (`resolve_screen`, `src/main.rs:144-184`). With no
  `--screen`, it runs `ffmpeg -f avfoundation -list_devices true` and
  `parse_screen_index` scrapes the first `[N] Capture screen` line (the index is
  the bracket immediately before the label, since the line carries two bracketed
  fields). Device 0 is often a camera, so auto-detect is necessary
  (`src/main.rs:61-64`).
- **Preflight** (`preflight`, `src/main.rs:210-229`). Verifies ffmpeg runs and
  its `-encoders` list contains `hevc_videotoolbox`, failing with a clear message
  rather than a black stream or cryptic error later.
- **Supervisor** (`supervise`, `src/main.rs:256-318`). A loop that spawns ffmpeg,
  then `tokio::select!`s over a sync tick (`uploader.sync()`), `child.wait()`
  (ffmpeg died: sync once more, restart with exponential backoff capped at 30s),
  and `ctrl_c` (graceful stop, final sync, return). Each (re)start gets a fresh
  session id `{UTC timestamp}-{attempt:02}` so a crash never reuses or corrupts
  an earlier session's segment numbering (`src/main.rs:253-255,267`).
- **Graceful stop** (`stop_gracefully`, `src/main.rs:324-337`). Writes `q` to
  ffmpeg's stdin (its documented interactive stop) so it flushes the final
  segment and writes `#EXT-X-ENDLIST`, turning the session into a finished VOD;
  falls back to `kill` after 10s.
- **Uploader** (`Uploader`, `src/main.rs:341-440`). Tracks `sent` names so it
  never re-ships a file. `sync` reads the local playlist, uploads `init.mp4` then
  each listed segment not yet sent, and ships the playlist last and only when
  every referenced file is uploaded and the playlist changed
  (`src/main.rs:381-405`). `reqwest::Client` has bounded connect (5s) and request
  (30s) timeouts so a hung upload cannot stall the supervisor select loop; on
  timeout the `PUT` errors and is retried next sync (`src/main.rs:351-360`).

## ffmpeg argv (`build_ffmpeg_args`, `src/main.rs:455-518`)

Input: `-f avfoundation -capture_cursor {0|1} -framerate {fps} -i {screen}:none`
(`:none` = no audio). Optional `-vf scale=-2:{max_height}`. Encode:
`-c:v hevc_videotoolbox -realtime 1 -b:v {bitrate} -tag:v hvc1 -pix_fmt yuv420p
-g {fps*segment_seconds}` (the `hvc1` tag is the fMP4/Apple-compatible HEVC
sample entry; some players refuse the stream without it; keyframe interval ==
segment length so HLS splits on IDR boundaries). Output: `-f hls -hls_time
{segment_seconds} -hls_playlist_type event -hls_list_size 0 -hls_flags
independent_segments -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4
-hls_segment_filename {dir}/seg_%05d.m4s {dir}/index.m3u8`. The playlist and
init names are constants (`PLAYLIST = "index.m3u8"`, `INIT = "init.mp4"`,
`src/main.rs:45-46`).

## Build and wiring (`default.nix`)

Built via `ix.cargoUnit.selectBinaryWithTests` then `makeWrapper`-wrapped to put
`pkgs.ffmpeg` on PATH (`default.nix:25-36`); nixpkgs ffmpeg is built with
VideoToolbox on Darwin, so it carries `hevc_videotoolbox` (`default.nix:22-24`).
A `printsHelp` passthru test asserts `screencast --help` prints `Usage:
screencast` (`default.nix:38-55`). Unit tests cover `parse_screen_index`,
`parse_segments`, and `sanitize` (`src/main.rs:520-559`).

## Run

```
nix run .#screencast -- --server http://<host>:8080
```

Auto-detects the display, captures at 30 fps / 6 Mbit/s H.265, streams under
`<you>/<timestamp>`, Ctrl-C to finalize. Grant the terminal Screen Recording
permission first, or avfoundation captures a black frame with no error
(`src/main.rs:26-28`, `README.md:31-42`).

## Bad fit / caveats

- Not for sub-second live latency: HLS buffers a few segments. Reach for WebRTC
  or SRT if glass-to-glass latency matters (`README.md:54-58`).
- No transport encryption of its own; run behind a private network/tailnet
  (`README.md:27-29`).
- macOS only (see above).
