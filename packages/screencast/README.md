# screencast

Stream a Mac's screen to a central server as hardware-encoded H.265, so a whole
team can push their screens to one place that stores every session for replay and
for downstream data and context use.

Two pieces:

- **`screencast`** (this crate): the macOS client. Captures the desktop, encodes
  it with `hevc_videotoolbox` (the Apple media engine, so it stays near-free on
  CPU and battery), and uploads the stream to the server.
- **`screencast-ingest`** (sibling crate): the server. Accepts uploads, stores
  every session on disk, and serves them back for live view, replay, and
  indexing.

The wire format is plain fragmented-MP4 HLS over HTTP, so a stored session plays
in Safari directly and in any browser through hls.js. Nothing is transcoded: the
bytes the hardware encoder produced are what land on disk.

## Run the server

    nix run .#screencast-ingest -- --root /var/lib/screencast --addr 0.0.0.0:8080

Open `http://<host>:8080/` for the dashboard (a session list plus a player).
`GET /api/sessions` returns the same data as JSON for scripts and indexers.

To require auth on uploads (reads stay open), pass `--token <secret>` (or set
`SCREENCAST_TOKEN`). Run it behind a private network or tailnet; there is no
transport encryption of its own.

## Stream from a Mac

    nix run .#screencast -- --server http://<host>:8080

That auto-detects the display, captures at 30 fps and 6 Mbit/s H.265, and streams
under `<you>/<timestamp>`. Stop with Ctrl-C, which finalizes the session into a
complete VOD. Grant Screen Recording permission to the terminal first, or the
capture is a black frame with no error.

Useful flags: `--user`, `--screen N` (`--list-screens` to enumerate displays),
`--fps`, `--bitrate 10M`, `--segment-seconds`, `--max-height 1440` (downscale to
cut bandwidth on Retina displays), `--no-cursor`, `--token`.

## How it streams

ffmpeg writes HLS segments to a local scratch directory; the client uploads each
segment the playlist has finalized, then the playlist itself, with ordinary sized
HTTP `PUT`s. This is deliberate: ffmpeg can `PUT` HLS directly, but its HTTP muxer
holds connections open without finalizing each request body, which a strict
HTTP/1.1 server leaves hanging. Uploading from the client also means a failed
`PUT` is just retried on the next sync, so a brief network blip does not drop the
stream.

## Bad fit if

- You need sub-second live latency. HLS buffers a few segments; this targets
  reliable capture and replay, not real-time control. Reach for WebRTC or SRT if
  glass-to-glass latency matters.
- You are not on macOS. Capture uses `avfoundation` + VideoToolbox. The server is
  cross-platform; only the client is Mac-only.
