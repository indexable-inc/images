# Boss Bar Overlay

A transparent, always-on-top, click-through desktop overlay that draws
Minecraft boss bars across the top of your screen. The bars are driven entirely
by a single SQLite file: write rows into it from anything (a shell, a script, a
cron job, another program) and they appear on screen within ~200ms.

The bars are rendered from Minecraft's actual boss bar sprite textures, layered
the same way the game's `BossHealthOverlay` does (color background, color
progress clipped to the fill, then the notch overlay), so the result is a 1:1
match. It is a native [`winit`](https://github.com/rust-windowing/winit) +
[`wgpu`](https://github.com/gfx-rs/wgpu) app with no webview: the window is
transparent, always-on-top, and passes the mouse straight through, so the bars
float over whatever you're doing.

![preview](docs/preview.png)

## Run

```sh
nix run .#bossbar-overlay
```

For local Rust development, fetch the Mojang art once (sprites + the Minecraft
TTF), then build with cargo:

```sh
cd app
bash scripts/fetch-assets.sh   # downloads into app/assets/, no-op once present
cargo run                      # the overlay
```

The window covers the top of the primary monitor. There is no tray; quit it the
way you quit any foreground process (Ctrl-C from the terminal, or stop the
service that runs it). `BOSSBAR_SCALE=3` (or `--scale 3`) enlarges the bars.

To verify rendering without a window, render the current bars straight to a
transparent PNG:

```sh
nix run .#bossbar-overlay -- --snapshot out.png --scale 3 --size 700x260
```

## CLI

`./bossbar` is a small wrapper around the same database the overlay reads, so
you don't have to hand-write SQL. It works whether or not the app is running and
creates the schema on demand.

```sh
./bossbar add "Ender Dragon" --color pink --overlay notched_20 --progress 0.8
./bossbar set "Ender Dragon" --progress 0.5      # match by title ...
./bossbar set 1 --color red --visible 1          # ... or by id
./bossbar list
./bossbar rm "Ender Dragon"
./bossbar clear
./bossbar db                                     # print the database path
```

Or skip the wrapper and write SQL directly:

```sh
DB="$(./bossbar db)"
sqlite3 "$DB" "UPDATE bossbars SET progress = 0.5 WHERE title = 'Ender Dragon';"
```

## The data contract

The overlay reads one table. On first launch it creates the database and seeds
three example bars so you can see it working.

```sql
CREATE TABLE bossbars (
  id        INTEGER PRIMARY KEY,
  title     TEXT    NOT NULL DEFAULT '',     -- text shown above the bar
  progress  REAL    NOT NULL DEFAULT 1.0,    -- fill fraction, 0.0 .. 1.0
  color     TEXT    NOT NULL DEFAULT 'purple',
  overlay   TEXT    NOT NULL DEFAULT 'progress',
  visible   INTEGER NOT NULL DEFAULT 1,      -- 0 hides the row
  position  INTEGER NOT NULL DEFAULT 0       -- sort order, top to bottom
);
```

- **color**: `pink`, `blue`, `red`, `green`, `yellow`, `purple`, `white`
  (Minecraft's seven boss bar colors). Unknown values fall back to `purple`.
- **overlay**: `progress` (smooth) or `notched_6` / `notched_10` / `notched_12`
  / `notched_20` (segmented), matching Minecraft's overlay styles.

This mirrors Minecraft's own boss bar API, so the fields should feel familiar.

### Where the database lives

Default: `~/Library/Application Support/bossbar-overlay/bossbars.db` on macOS
(`$XDG_DATA_HOME/bossbar-overlay/bossbars.db` on Linux). The resolved path is
printed to stdout on launch and printed by `./bossbar db`. Override it with
`BOSSBAR_DB=/path/to.db`.

Any committed write bumps SQLite's `PRAGMA data_version`, which the app polls
four times a second to know when to re-read. The database runs in WAL mode so
your writers never block the overlay's reader.

## How it works

The app lives under `app/` as a standalone Rust crate (its own Cargo workspace,
off the repo's main graph):

- `app/src/db.rs` — opens the DB (bundled SQLite via `rusqlite`), polls
  `data_version` four times a second, and hands fresh bars to the UI thread on
  every change.
- `app/src/overlay.rs` — the winit event loop and window: transparent,
  always-on-top, click-through (`set_cursor_hittest(false)`), spanning the top
  of the primary monitor. macOS runs as an `Accessory` app (no Dock icon). The
  watcher wakes the loop with a user event, so a DB write triggers a redraw.
- `app/src/render.rs` — the wgpu renderer: one textured-quad pipeline draws the
  same layer stack Minecraft uses, clipping the progress layers to the fill
  fraction with nearest sampling (crisp pixels). Titles are drawn with
  [`glyphon`](https://github.com/grovesNL/glyphon) in a pixel-accurate Minecraft
  TTF ([tryashtar/minecraft-ttf](https://github.com/tryashtar/minecraft-ttf)),
  with the vanilla one-pixel drop shadow.
- `app/src/snapshot.rs` — runs the identical renderer headlessly into a PNG, so
  the transparent overlay can be verified from a file.

## Notes / limits

- There is no tray icon; quit the overlay like any foreground process
  (Ctrl-C, or via the service that runs it).
- Click-through relies on `set_cursor_hittest`, which some Wayland compositors
  do not implement; on those the window may capture clicks.
- The window covers the top ~45% of the **primary** monitor. Multi-monitor and
  per-monitor placement aren't handled yet.
- The boss bar textures and the Minecraft title TTF are Mojang-derived art and
  are **not** redistributed in this repo; they are fetched at build time (the
  Nix derivation, or `app/scripts/fetch-assets.sh` for local builds) for
  personal use. This project is not affiliated with or endorsed by Mojang.

Implemented with AI assistance (Claude, Opus 4.8).
