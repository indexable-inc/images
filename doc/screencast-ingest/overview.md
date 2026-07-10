# screencast-ingest

`packages/screencast-ingest` is the server half of the screencast pipeline: an
`axum` HTTP server that receives the fragmented-MP4 HLS streams the
[screencast](../screencast/overview.md) client `PUT`s, writes them under a root
directory (one folder per user and session), and serves them back so the same
URLs play in any HLS client (Safari natively, others via hls.js)
(`src/main.rs:1-9`). Everything is retained: a finished session is a complete VOD
ready to feed downstream data/context pipelines (frame sampling, OCR, indexing)
(`src/main.rs:11-13`). Cross-platform, so it deploys on the Linux fleet; there is
no transcoding (`src/main.rs:15-16`).

A Rust workspace crate (`Cargo.toml`); flake output `.#screencast-ingest`. The
filesystem is the single source of truth: the session index and dashboard are
derived from what is on disk, nothing is tracked separately (`src/main.rs:7-9`).

## Public surface

### CLI flags (`src/main.rs:50-65`)

| flag | env | default | meaning |
| --- | --- | --- | --- |
| `--root` | `SCREENCAST_ROOT` | `./screencast-data` | directory streams are stored under as `{user}/{session}/...` |
| `--addr` | `SCREENCAST_ADDR` | `0.0.0.0:8080` | listen address |
| `--token` | `SCREENCAST_TOKEN` | none | if set, uploads must carry `Authorization: Bearer <token>`; reads stay open |

`--root` is created and canonicalized at startup (`src/main.rs:94-100`). Logging
is `tracing` + `EnvFilter`, default `info`, with a `tower_http` trace layer
(`src/main.rs:86-91,118`). Graceful shutdown on Ctrl-C (`src/main.rs:127-130`).

### Routes (`src/main.rs:110-119`)

| method + path | handler | purpose |
| --- | --- | --- |
| `GET /` | `dashboard` | single-page dashboard (static HTML, `src/main.rs:449`) |
| `GET /healthz` | inline | returns `ok` |
| `GET /api/sessions` | `api_sessions` | JSON `Vec<SessionInfo>`, for the dashboard and indexers |
| `PUT /ingest/{user}/{session}/{file}` | `upload` | store a playlist or segment |
| `GET /ingest/{user}/{session}/{file}` | `serve` | play back a stored file |
| `DELETE /ingest/{user}/{session}/{file}` | `remove` | delete a file (for expiring HLS muxers; rarely used) |

The dashboard is `include_str!("dashboard.html")` (`src/main.rs:449-450`), a
fully static page that fetches `/api/sessions` itself and plays sessions with
hls.js; no server-side templating.

### SessionInfo JSON (`src/main.rs:312-330`)

Per session: `user`, `session`, `playlist` (URL
`/ingest/{user}/{session}/index.m3u8`), `started`/`updated` (epoch-second file
mtimes), `segments` (count of `.m4s`/`.ts`), `bytes` (total on disk), `live`
(written within the live window and not complete), `complete` (playlist has
`#EXT-X-ENDLIST`). Sorted most-recently-active first (`src/main.rs:364`).

## Key internals

- **Path safety** (`safe_component`, `src/main.rs:139-146`; `resolve`,
  `src/main.rs:181-196`). Every `{user}/{session}/{file}` component must be a
  non-empty, `<=128`-char plain name (alphanumeric plus `_ - .`, not `.`/`..`).
  This is the sole guard between a client path and a disk write; it rejects
  anything it does not positively recognize. It mirrors the client's own
  `sanitize` but is enforced independently.
- **Atomic uploads** (`upload`, `src/main.rs:235-265`). Auth is checked before
  the body is read, so an unauthenticated request never buffers a large upload.
  The extension must be `m3u8`/`m4s`/`mp4`/`ts`. The body (`<= MAX_UPLOAD` = 64
  MiB, `src/main.rs:39`) is written to a hidden temp file in the destination dir
  and renamed into place, so a reader polling the playlist never sees a
  half-written file. Returns `201 Created`.
- **Serve** (`serve`, `src/main.rs:268-283`). Reads the resolved file and returns
  it with a `Content-Type` mapped from extension (`content_type`,
  `src/main.rs:149-156`): `.m3u8` -> `application/vnd.apple.mpegurl`, `.m4s`/`.mp4`
  -> `video/mp4`, `.ts` -> `video/mp2t`. No transcoding.
- **Auth** (`check_auth`/`ct_eq`, `src/main.rs:201-229`). When `--token` is set,
  mutating requests (`PUT`, `DELETE`) require a matching bearer token, compared
  constant-time (length may leak). Reads are always open.
- **Scan + cache** (`scan_sessions`/`summarize`, `src/main.rs:335-427`). Two-level
  directory walk (`root/user/session`); per-session it sums bytes, counts
  segments, tracks earliest/latest mtime, requires an `index.m3u8`, and reads it
  for `#EXT-X-ENDLIST`. Hidden `.`-prefixed files (in-flight temps) are skipped.
  Per-entry errors are skipped rather than failing the whole listing.
  `api_sessions` caches the scan for `SCAN_CACHE_TTL` = 2s
  (`src/main.rs:44-47,432-445`) so a poll storm (many dashboard tabs, or an
  unauthenticated caller) cannot amplify into a per-request full-tree walk.
- **Live window** `LIVE_WINDOW_SECS` = 30s (`src/main.rs:42,424`): a session is
  `live` if not complete and its newest file changed within 30s.
- **Error hygiene** (`internal`, `src/main.rs:303-309`). Logs the detailed cause
  and returns a generic 500 so an OS error string (which can include absolute
  server paths) never reaches the client. `HttpError` (`src/main.rs:161-170`) is
  the typed `(status, message)` response shape returned from handlers.

## Build and wiring (`default.nix`)

Built directly with `ix.cargoUnit.selectBinaryWithTests`; no wrapper is needed
(it shells out to nothing, `default.nix:15-18`). A `printsHelp` passthru test
asserts `screencast-ingest --help` prints `Usage: screencast-ingest`
(`default.nix:20-37`).

## Run

```
nix run .#screencast-ingest -- --root /var/lib/screencast --addr 0.0.0.0:8080
```

Open `http://<host>:8080/` for the dashboard; `GET /api/sessions` is the same
data as JSON. Add `--token <secret>` (or `SCREENCAST_TOKEN`) to require auth on
uploads; reads stay open. No transport encryption of its own; run behind a
private network or tailnet (`screencast/README.md:20-29`).

## Caveats

- Storage grows unbounded by design (everything retained). `DELETE` exists for
  expiring muxers but the default client keeps everything (`src/main.rs:285-287`).
- The dashboard pins hls.js from a CDN (`src/dashboard.html:7`), so the dashboard
  UI needs outbound internet; playback itself does not.
