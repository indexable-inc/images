# Boss Bar Overlay

A transparent, always-on-top, click-through desktop overlay that draws
Minecraft boss bars across the top of your screen. The bars are driven entirely
by a single SQLite file: write rows into it from anything (a shell, a script, a
cron job, another program) and they appear on screen within ~200ms.

The bars are rendered from Minecraft's actual boss bar sprite textures, layered
the same way the game's `BossHealthOverlay` does (color background, color
progress clipped to the fill, then the notch overlay), so the result is a 1:1
match. Built with [Tauri v2](https://tauri.app); the window passes the mouse
straight through, so the bars float over whatever you're doing.

![preview](docs/preview.png)

## Run

```sh
bun install
bun run tauri dev      # dev build with hot reload
# or a real app bundle:
bun run tauri build    # produces a .app / .dmg under src-tauri/target/release/bundle
```

The first `dev`/`build` runs `scripts/fetch-assets.sh` automatically (via the
`predev`/`prebuild` hooks) to download the boss bar textures into
`src/assets/boss_bar/`. Run it on its own with `bun run fetch-assets`. It needs
network access once; after that it is a no-op.

Quit from the tray icon (menu bar on macOS). The tray menu also has **Open
database folder**.

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
printed to stdout on launch, shown in the tray tooltip, and printed by
`./bossbar db`. Override it with `BOSSBAR_DB=/path/to.db`.

Any committed write bumps SQLite's `PRAGMA data_version`, which the app polls
four times a second to know when to re-read. The database runs in WAL mode so
your writers never block the overlay's reader.

## How it works

- `src-tauri/src/lib.rs` — opens the DB (bundled SQLite via `rusqlite`), polls
  `data_version`, and emits a `bossbars` event to the webview on every change.
  Also configures the overlay window (transparent, always-on-top, ignores the
  cursor, spans the top of the primary monitor) and the tray icon.
- `src/main.ts` — listens for `bossbars` events and reconciles the DOM by row
  `id`, so progress changes animate instead of flickering.
- `src/styles.css` — stacks the four sprite layers per bar and clips the
  progress layers to the fill fraction, scaled from the native 182x5 textures.
  Titles use a pixel-accurate Minecraft TTF generated from the real Minecraft
  font definitions ([tryashtar/minecraft-ttf](https://github.com/tryashtar/minecraft-ttf)),
  rendered 1:1 at an integer multiple-of-12px size with antialiasing off.

## Notes / limits

- macOS transparency uses `macOSPrivateApi`, which is fine for personal use but
  is rejected by the Mac App Store.
- The window covers the top ~45% of the **primary** monitor. Multi-monitor and
  per-monitor placement aren't handled yet.
- The boss bar textures and the Minecraft title TTF are Mojang-derived art and
  are **not** redistributed in this repo; they are fetched at build time
  (`fetch-assets.sh` for the sprites, the nix derivation for the font) for
  personal use. This project is not affiliated with or endorsed by Mojang.

Implemented with AI assistance (Claude, Opus 4.8).
