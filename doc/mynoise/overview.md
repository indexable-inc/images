# mynoise

`packages/mynoise` plays [myNoise.net](https://mynoise.net) generators from the
CLI by streaming and mixing their band loops locally. myNoise has no server-side
audio generation: each generator is a handful of stereo OGG loops served as
static files at `https://mynoise.net/Data/<CODE>/<n>a.ogg`, one per frequency
band, and the website's sliders are pure per-band volume mixed in the browser
(`src/main.rs:1-8`). This tool resolves a name to its `<CODE>`, downloads the
bands (cached locally), then loops and mixes them with the same per-band gains.
The audio is copyright Stephane Pigeon; streaming for personal listening is fine,
redistribution is not (`src/main.rs:10-11`).

A Rust workspace crate (`Cargo.toml`); flake output `.#mynoise`. Unlike the rest
of the domain it uses no ffmpeg: it decodes OGG/Vorbis and mixes in-process with
`rodio` (`Cargo.toml:19-22`). HTTP is `reqwest` over rustls (no system OpenSSL),
fetching over a small current-thread tokio runtime that drives only the download
phase (`Cargo.toml:15-23`, `src/main.rs:67-71`).

## Public surface: CLI (`src/main.rs:24-56`)

| arg / flag | default | meaning |
| --- | --- | --- |
| `name` (positional) | (required unless `--list`) | a bare data code (`RAIN`, `OSMOSIS`) or a generator-page slug (`rainNoiseGenerator`) |
| `gains` (positional, repeated) | per-band default | per-band gains in band order, each `0..=100`; extras past the band count are ignored |
| `--list` | | list generator slugs scraped from the myNoise index and exit |
| `--volume` | `50` | master volume `0..=100` on top of every per-band gain |
| `--default-gain` | `70` | gain `0..=100` for any band without an explicit value |
| `--cache-dir` | OS cache dir + `mynoise` | cache directory for downloaded band files |

Bare `mynoise` prints help (`arg_required_else_help`); clap requires exactly one
of `name` or `--list` via a required `ArgGroup`, so the "no target" usage error
is a clean exit 2 with no backtrace (`src/main.rs:26-30`).

## Modules (`src/main.rs:13-14`)

- **`resolve`** (`src/resolve.rs`) - name/slug -> `CODE`, then download the bands.
- **`audio`** (`src/audio.rs`) - decode, loop, mix, and play until interrupted.

## Resolution and download (`src/resolve.rs`)

- **`resolve_code`** (`src/resolve.rs:76-97`). Tries the name as a bare
  upper-cased code first, verifying it by a HEAD probe of its `0a.ogg` (band 0 is
  the lowest band and always present for a valid code, `src/resolve.rs:79-82`).
  Otherwise it fetches the generator page
  `https://mynoise.net/NoiseMachines/<name>.php` and scrapes it
  (`src/resolve.rs:84-96`).
- **`parse_data_code`** (`src/resolve.rs:49-64`). Pulls the audio `CODE` out of
  the page HTML by finding the first `Data/<CODE>/` whose code is immediately
  followed by a band file. Anchoring on the band file (`is_band_file`,
  `src/resolve.rs:27-38`: digits then `a`/`b` then a non-alphanumeric boundary)
  is robust to a page whose first `Data/` reference is a share image like
  `Data/RAIN/fb.jpg`. Pure, so it is unit-tested offline (`src/resolve.rs:183-240`).
- **`download_bands`** (`src/resolve.rs:101-149`). Probes `0a.ogg`, `1a.ogg`, ...
  up to `MAX_BANDS` = 16 (`src/resolve.rs:20`); the first 404 marks the end of
  the band list (`src/resolve.rs:124-127`). Bands are 0-indexed; starting at 1
  would drop the lowest band and shift every per-band gain by one
  (`src/resolve.rs:110-111`). Existing files are reused. Each download is written
  to a `.ogg.part` temp then renamed, so a Ctrl-C mid-download (the normal way to
  quit) cannot leave a truncated `.ogg` that the next run reuses as complete
  (`src/resolve.rs:134-141`). Only the `a` takes are used; `b` files are the
  site's alternate randomized takes (`src/resolve.rs:4-9`).
- **`list_slugs`** (`src/resolve.rs:152-181`). Fetches `noiseMachines.php` and
  scrapes `NoiseMachines/<slug>` references, sorted and de-duped.

A real `User-Agent` is set so the index/page scrape is not served a bot stub
(`src/main.rs:61-65`).

## Gain math and playback

`main` resolves per-band gains in band order (explicit value, else
`--default-gain`), each `/100.0` (`src/main.rs:98-103`). The master is
`volume/100 / sqrt(active band count)` (`src/main.rs:105-111`): the bands are
decorrelated noise, so their power (not amplitude) adds; dividing by
`sqrt(active)` keeps a full mix from clipping in rodio's mixer while holding
roughly constant perceived loudness as the band count changes. Each band's final
linear amplitude is `gain * master` (`src/main.rs:113-120`).

`audio::play` (`src/audio.rs:25-60`) opens the default device sink
(`DeviceSinkBuilder::open_default_sink`), then for each non-zero band decodes the
OGG, wraps it `.buffered().repeat_infinite().amplify(amplitude)` (the `Buffered`
wrapper is needed because `Decoder` is not `Clone`, caching decoded samples to
replay each loop), and connects a per-band `rodio::Player` to the shared mixer
(`src/audio.rs:1-6,37-48`). Every player is held for the life of playback;
dropping one stops its band. If all bands are muted it bails
(`src/audio.rs:51-53`). The loops never end on their own, so it `thread::park()`s
forever until Ctrl-C tears down the process (`src/audio.rs:55-60`).

## Build and wiring (`default.nix`)

Built directly with `ix.cargoUnit.selectBinaryWithTests` (`default.nix:3-9`); no
wrapper. No extra Nix plumbing for audio on Linux: the workspace already wires
ALSA's pkg-config and `-lasound` link path for every unit (see
`lib/rust/workspace.nix`, noted at `Cargo.toml:19-21`). Flake output `.#mynoise`.

## Run

```
nix run .#mynoise -- RAIN              # play the RAIN generator at default gains
nix run .#mynoise -- --list            # list generator slugs
nix run .#mynoise -- OSMOSIS 100 80 0  # per-band gains in band order
```

## Caveats

- Needs network on first play of a code (to scrape/download); subsequent plays
  reuse the cache. `--list` and slug resolution always hit the network.
- Needs a working audio output device; there is no file-output mode.
- `MAX_BANDS = 16` caps probing; real generators top out around 10 bands
  (`src/resolve.rs:17-20`).
