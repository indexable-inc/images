# Minecraft Desktop Overlays

Two transparent, always-on-top, click-through desktop overlays drawn in the
Minecraft style with [`wgpu`](https://github.com/gfx-rs/wgpu): a **boss bar** HUD
and an open **book**. Both are driven entirely by a single SQLite file each: write
rows from anything (a shell, a script, a cron job, another program) and the change
appears on screen within ~200ms.

Both share one engine, [`overlay-core`](app/crates/overlay-core), which owns the
float window (transparent, borderless, always-on-top, click-through, drag-to-move)
and a single textured-quad wgpu pipeline. Text is the real Minecraft bitmap font
rendered as glyph quads through that same pipeline, so titles and page text are
just more sprites. The two apps ([`bossbar`](app/crates/bossbar),
[`book`](app/crates/book)) are thin domain layers on top.

All Minecraft art (boss bar sprites, the book texture and page widgets, the font
sheet) is extracted from Mojang's official `client.jar` by the
[`minecraft-assets`](../minecraft-assets) Nix derivation, pinned by Mojang's own
hash. Nothing is vendored into the repo or pulled from a third-party mirror.

![preview](docs/preview.png)

## Run

```sh
nix run .#bossbar-overlay     # the boss bar HUD across the top of the screen
nix run .#book-overlay        # a floating open book
```

For local Rust development, populate the gitignored art once (it is copied out of
the `minecraft-assets` derivation), then build with cargo:

```sh
cd app
bash scripts/fetch-assets.sh   # nix-builds minecraft-assets and copies the slices
cargo run -p bossbar           # or: cargo run -p book
```

There is no tray; quit either overlay the way you quit any foreground process
(Ctrl-C, or stop the service that runs it).

## Boss bar

Each bar is its own small transparent, always-on-top, borderless window sized to
just that bar, so the desktop stays usable everywhere else: only the bars
intercept the mouse. `BOSSBAR_SCALE=3` (or `--scale 3`) enlarges the bars.

The bars are rendered from Minecraft's actual boss bar sprite textures, layered
the same way the game's `BossHealthOverlay` does (color background, color progress
clipped to the fill, then the notch overlay). Hover one and it eases to fully
opaque and gently grows with a slow breathing pulse; a press that moves past a few
pixels starts the platform's native window drag, and the drop location is saved to
the bar's `x`/`y` columns so it stays put across restarts. A two-finger trackpad
scroll over a bar nudges it the same way, without pressing. A press that does not
move is a click: it opens the bar's `url` if it has one. A bar with a
`description` unfolds a flat panel beneath it on hover; a bar with a `since` (Unix
epoch) shows a live elapsed timer in its title (`Build (2:05)`).

Render the current bars straight to a transparent PNG to verify without a window:

```sh
nix run .#bossbar-overlay -- --snapshot out.png --scale 3 --size 760x620
```

### Boss bar CLI

`./bossbar` is a small wrapper around the same database the overlay reads, so you
don't have to hand-write SQL. It works whether or not the app is running and
creates the schema on demand.

```sh
./bossbar add "Ender Dragon" --color pink --overlay notched_20 --progress 0.8 \
  --description "Destroy the End Crystals first or it heals back to full."
./bossbar set "Ender Dragon" --progress 0.5      # match by title ...
./bossbar set 1 --color red --visible 1          # ... or by id
./bossbar list
./bossbar rm "Ender Dragon"
./bossbar db                                     # print the database path
```

### Boss bar data contract

The overlay reads one table; on first launch it seeds three example bars.

```sql
CREATE TABLE bossbars (
  id          INTEGER PRIMARY KEY,
  title       TEXT    NOT NULL DEFAULT '',   -- text shown above the bar
  description TEXT    NOT NULL DEFAULT '',   -- hover pop-down body (wraps/paragraphs)
  progress    REAL    NOT NULL DEFAULT 1.0,  -- fill fraction, 0.0 .. 1.0
  color       TEXT    NOT NULL DEFAULT 'purple',
  overlay     TEXT    NOT NULL DEFAULT 'progress',
  visible     INTEGER NOT NULL DEFAULT 1,    -- 0 hides the row
  position    INTEGER NOT NULL DEFAULT 0,    -- sort order in the auto column
  x           REAL,                          -- pinned location (logical points)
  y           REAL,                          -- NULL/NULL = auto-stacked
  since       INTEGER,                       -- Unix epoch; live elapsed timer in the title
  url         TEXT    NOT NULL DEFAULT ''    -- opened with the system opener on click
);
```

- **color**: `pink`, `blue`, `red`, `green`, `yellow`, `purple`, `white`. Unknown
  values fall back to `purple`.
- **overlay**: `progress` (smooth) or `notched_6` / `notched_10` / `notched_12` /
  `notched_20` (segmented).

Default DB: `~/Library/Application Support/bossbar-overlay/bossbars.db` on macOS
(`$XDG_DATA_HOME/bossbar-overlay/bossbars.db` on Linux). Override with
`BOSSBAR_DB=/path/to.db`.

## Book

A single floating window shows an open book as a two-page spread. Minecraft ships
a one-page book texture; the spread is that page drawn twice, mirrored on the left
so the spiral binding meets at the centre spine and normal on the right. Pages of
text come from SQLite; each page shows a `Page N of M` header and its wrapped
body. Click the page-turn arrows at the bottom outer corners to advance, drag the
book (or two-finger scroll over it) to move it (its position is saved), and hover
to raise it above other windows. `BOOK_SCALE=3` (or `--scale 3`) resizes it.

```sh
nix run .#book-overlay
nix run .#book-overlay -- --snapshot spread.png --scale 3 --page 0
```

### Book data contract

```sql
CREATE TABLE book (
  id    INTEGER PRIMARY KEY CHECK (id = 1),  -- the singleton book row
  title TEXT    NOT NULL DEFAULT '',
  x     REAL, y REAL                         -- pinned window position (logical points)
);
CREATE TABLE pages (
  id   INTEGER PRIMARY KEY,
  idx  INTEGER NOT NULL DEFAULT 0,           -- page order
  body TEXT    NOT NULL DEFAULT ''           -- newlines start new lines/paragraphs
);
```

On first launch the DB is seeded with a short four-page book. Default DB:
`~/Library/Application Support/book-overlay/book.db` (XDG on Linux); override with
`BOOK_DB=/path/to.db`. The current page (spread) is in-memory state changed by the
arrows; only the position persists.

## How it works

The workspace lives under `app/` as its own Cargo workspace (off the repo's main
graph, so the GUI crates skip the strict workspace lints), with one vendored
`Cargo.lock`. `app/default.nix` builds both binaries from it.

- [`overlay-core`](app/crates/overlay-core) — the shared engine. `window.rs`: the
  float window attributes, surface/adapter setup, transparent alpha-mode
  selection, and a non-activating raise (`-[NSWindow orderFrontRegardless]` on
  macOS). `gpu.rs`: one textured-quad pipeline with a texture registry, plus the
  bitmap font baked in so `text()`/`text_shadow()` emit glyph quads.
  `bitmap_font.rs`: measures the vanilla `ascii.png` glyphs (white-on-transparent,
  width = rightmost inked column + 1) the way the game does. `gesture.rs`: the
  press/drag/click state machine. `snapshot.rs`: the same engine rendered
  headlessly into a PNG.
- [`bossbar`](app/crates/bossbar) — bars domain (`bars.rs`), the SQLite source
  (`db.rs`), the bar/panel scene builder, and the per-bar window loop.
- [`book`](app/crates/book) — book domain (`book.rs`), its SQLite source
  (`db.rs`), the two-page spread scene builder, and the single-window loop.
- [`minecraft-assets`](../minecraft-assets) — the reproducible Mojang extraction:
  `client.jar` pinned by Mojang's hash, unzipped to the textures and font sheet
  both overlays embed.

## Notes and limits

- No tray icon; quit like any foreground process.
- Click-through is structural (no window off an overlay), not `set_cursor_hittest`;
  some Wayland tiling compositors may still force-place or tile borderless windows
  and fight free-drag placement.
- The boss bar covers the top of the **primary** monitor; multi-monitor placement
  is not handled yet. The book opens centred on the primary monitor.
- The Minecraft textures and font are Mojang's art and are **not** redistributed
  in this repo; they are extracted from the official client jar at build time for
  personal use. This project is not affiliated with or endorsed by Mojang.

Implemented with AI assistance (Claude, Opus 4.8).
