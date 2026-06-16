# media

Recording, screen capture, and audio for index: the tools that turn a terminal
session, a Mac desktop, a remote noise generator, or a line of text into a
playable media stream or file. Four of the five drive [ffmpeg](#glossary) as the
codec engine; none implement their own video or audio codec. The domain spans
one capture/encode/serve pipeline (screencast client to screencast-ingest
server) plus three standalone CLIs (a terminal-reel recorder, a noise mixer, and
a TTS speaker).

Read this page first, then the component page for the unit you are changing. The
[reel](reel/overview.md) recorder is also what generates the animated demo at
the top of the repo `README.md` (`docs/demo-{dark,light}.{avif,webp}`), so it is
the most load-bearing unit here.

## Units

| unit | kind | flake output | notes |
| --- | --- | --- | --- |
| `packages/reel` | Rust workspace crate, Nix wrapper | `.#reel` | records a terminal reel through the [tui](../terminal/tui/overview.md) PTY driver; wraps ffmpeg + the demoed CLIs |
| `packages/screencast` | Rust workspace crate, Nix wrapper | `.#screencast` | macOS-only capture client; wraps ffmpeg (avfoundation + VideoToolbox) |
| `packages/screencast-ingest` | Rust workspace crate, Nix package | `.#screencast-ingest` | cross-platform HTTP server; stores and serves HLS |
| `packages/mynoise` | Rust workspace crate, Nix package | `.#mynoise` | streams and mixes myNoise.net band loops with rodio (no ffmpeg) |
| `packages/elevenlabs-say` | Python uv app, Nix package | `.#elevenlabs-say` | `say`-style ElevenLabs TTS; wraps ffplay/ffmpeg for playback |

All four Rust units are members of the root `Cargo.toml` workspace and build via
`ix.cargoUnit.selectBinaryWithTests` (`packages/*/default.nix`). `elevenlabs-say`
is a Python app built with `ix.buildUvApplication`
(`packages/elevenlabs-say/default.nix:18`). Each package is discovered from its
`package.nix` (`id` + `flake = true`); `screencast` advertises its flake/package
outputs only on Darwin (`packages/screencast/package.nix:7-14`).

## How it fits together

```
reel:         tui PTY grid ---> rasterize RGBA ---> ffmpeg ---> docs/demo-*.{avif,webp}
screencast:   desktop --(avfoundation)--> hevc_videotoolbox --(fMP4 HLS)--> local scratch
                  --(HTTP PUT /ingest/{user}/{session}/{file})--> screencast-ingest
screencast-ingest:  PUT -> {root}/{user}/{session}/ ; GET -> playback (Safari/hls.js)
mynoise:      mynoise.net/Data/<CODE>/<n>a.ogg --(cache)--> rodio decode+mix --> speakers
elevenlabs-say:  text -> ElevenLabs convert / WebSocket --> mp3 --> ffplay / file
```

- **Capture vs encode vs serve.** Only screencast + screencast-ingest form a
  client/server pipeline: the client captures and encodes, the server only
  stores and serves (no transcode, the bytes the hardware encoder produced are
  what land on disk, `packages/screencast-ingest/src/main.rs:15-16`). reel
  captures a synthetic source (the terminal grid) and encodes to a file. mynoise
  and elevenlabs-say only fetch+play; they neither encode nor serve.
- **ffmpeg is the shared codec engine.** reel
  (`packages/reel/src/encode.rs:103`), screencast
  (`packages/screencast/src/main.rs:276`), and elevenlabs-say
  (`apply_tempo`/`play`, `__init__.py:375,439`) all shell out to ffmpeg/ffplay,
  put on PATH by their Nix wrappers (`makeWrapper ... --prefix PATH`). mynoise is
  the exception: it decodes OGG/Vorbis and mixes in-process with `rodio`
  (`packages/mynoise/src/audio.rs`).
- **HLS is the transport between the two screencast units.** Fragmented-MP4 HLS
  over plain HTTP: an `init.mp4` init segment, `seg_NNNNN.m4s` media segments,
  and an `index.m3u8` playlist (`packages/screencast/src/main.rs:45-46,510-515`).
  The client uploads only segments the local playlist already lists, and ships
  the playlist last, so the server never advertises a segment that has not landed
  (`packages/screencast/src/main.rs:376-405`).
- **The filesystem is the screencast source of truth.** screencast-ingest tracks
  nothing separately: the session list, live/complete state, and dashboard are
  all derived from a directory scan (`scan_sessions`,
  `packages/screencast-ingest/src/main.rs:335`). A session is `complete` once its
  playlist carries `#EXT-X-ENDLIST` and `live` if written within 30s
  (`LIVE_WINDOW_SECS`, `main.rs:42,424`).
- **Auth is upload-only and optional.** Both screencast units take a bearer
  `--token`/`SCREENCAST_TOKEN`; reads (playback, dashboard, `/api/sessions`) stay
  open, writes are guarded (`check_auth`, `main.rs:201`). There is no transport
  encryption; run behind a private network or tailnet.

## Cross-component invariants

- **No transcoding in the screencast path.** Segments are stored and served
  byte-for-byte; content type is inferred from extension only
  (`content_type`, `screencast-ingest/src/main.rs:149`). A session plays from the
  same URLs it was uploaded to.
- **Atomic writes for concurrently read files.** Both the client cache
  (`mynoise/src/resolve.rs:137-141`) and the server upload path
  (`screencast-ingest/src/main.rs:257-263`) write to a temp file then rename, so
  a reader (a player polling the playlist, a re-run reusing a cache) never sees a
  half-written file.
- **Path components are sanitized at both ends.** The client folds unsafe chars
  to `-` (`sanitize`, `screencast/src/main.rs:196`); the server independently
  rejects any component that is not a safe plain name (`safe_component`,
  `screencast-ingest/src/main.rs:139`), the sole guard between a client path and
  a disk write.
- **Secrets come only from the environment.** elevenlabs-say reads
  `ELEVENLABS_API_KEY` and refuses to run without it (`make_client`,
  `__init__.py:232`); there is no embedded key.

## Glossary

- **PTY grid / styled cell**: reel's capture source. The [tui](../terminal/tui/overview.md)
  driver renders a VT100 screen to an `Array2<StyledCell>` (character + fg/bg +
  bold/italic/underline/inverse), which reel rasterizes.
- **ffmpeg / ffplay**: the external codec processes. ffmpeg encodes/transcodes;
  ffplay is its headless player. Provided on PATH by the Nix wrappers.
- **AVIF / WebP**: reel's two animated outputs. AVIF (AV1 in an AVIF container)
  is primary and smallest; WebP is the broadly-supported, lower-frame-rate
  fallback (`reel/src/encode.rs:22-28`).
- **H.265 / HEVC**: the screencast video codec. `hevc_videotoolbox` is Apple's
  hardware encoder; `hvc1` is the fMP4-compatible HEVC sample-entry tag
  (`screencast/src/main.rs:482-489`).
- **VideoToolbox / avfoundation**: macOS frameworks. VideoToolbox is the
  hardware media engine ffmpeg uses to encode; avfoundation is the capture input
  device (the screen is a `Capture screen N` device).
- **HLS**: HTTP Live Streaming. A playlist (`.m3u8`) plus media segments. Here
  segments are fragmented-MP4 (`fmp4`): one `init.mp4` plus `seg_NNNNN.m4s`.
- **VOD / ENDLIST**: a finished session. ffmpeg writes `#EXT-X-ENDLIST` on a
  clean exit, turning a live `event` playlist into a complete, replayable VOD
  (`screencast/src/main.rs:320-323`).
- **session**: one capture run, stored under `{root}/{user}/{session}/`. Each
  ffmpeg (re)start is its own session id (`screencast/src/main.rs:267`).
- **band loop / generator**: mynoise terms. A myNoise generator (`<CODE>`) is a
  set of per-frequency-band stereo OGG loops (`<n>a.ogg`); the mix is per-band
  volume, summed locally.
- **atempo / wpm**: elevenlabs-say's `--rate` retiming. ffmpeg's `atempo` filter
  changes tempo while preserving pitch, against a 175 wpm baseline, emulating
  macOS `say -r` (`__init__.py:337`).

## Components

| component | page | what |
| --- | --- | --- |
| reel | [reel/overview.md](reel/overview.md) | record a terminal reel through the tui PTY driver, rasterize, encode AVIF/WebP; generates the README demo |
| screencast | [screencast/overview.md](screencast/overview.md) | macOS H.265 desktop capture client, streams fMP4 HLS to ingest (Darwin-only) |
| screencast-ingest | [screencast-ingest/overview.md](screencast-ingest/overview.md) | HTTP server: store HLS per user/session, serve back for replay/live/indexing |
| mynoise | [mynoise/overview.md](mynoise/overview.md) | stream and mix myNoise.net band loops locally with rodio |
| elevenlabs-say | [elevenlabs-say/overview.md](elevenlabs-say/overview.md) | `say`-style ElevenLabs TTS CLI with streaming input and macOS `say` flags |
