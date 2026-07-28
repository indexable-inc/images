# bossbar-overlay

`packages/bossbar-overlay` is three transparent, always-on-top, click-through
desktop overlays drawn in the Minecraft style with `wgpu`: a boss bar HUD
(`bossbar`), an open book (`book`), and a floating experience orb (`orb`). Each is
driven entirely by a single SQLite file: write rows from anything and the change
appears within ~200ms. They share one engine, `overlay-core`, documented
separately in [engine.md](engine.md). This page covers the package layout, the
three apps, their data contracts, the CLI, and how it all builds.

The apps are a separate Cargo workspace under `app/` (root `Cargo.toml:10`), off
the repo's main graph so winit/wgpu deps and lints stay isolated (`Cargo.toml:1`).
All Mojang art comes from [minecraft-assets](../minecraft-assets/overview.md);
nothing is vendored.

## Units and flake outputs

| path | unit | output |
| --- | --- | --- |
| `app/crates/overlay-core` | shared engine (lib) | internal; see [engine.md](engine.md) |
| `app/crates/bossbar` | boss bar overlay bin | `.#bossbar-overlay` (main program) |
| `app/crates/book` | book overlay bin | `.#book-overlay` |
| `app/crates/orb` | XP-orb overlay bin | `.#xp-orb-overlay` |
| `bossbar` (shell script) | boss bar DB CLI | `.#bossbar` |

`app/default.nix` builds all three binaries from one `Cargo.lock` as the
`minecraft-overlays` derivation (`app/default.nix:44`), `mainProgram =
bossbar-overlay` (`app/default.nix:100`). `.#book-overlay`
(`book-overlay/book.nix`) and `.#xp-orb-overlay` (`xp-orb-overlay/orb.nix`) are
trivial symlink derivations that re-expose the already-built binaries with a
different main program, so there is no second wgpu/winit compile
(`book-overlay/book.nix:6`). The top-level `.#bossbar`
(`package.nix:2`, `cli.nix`) is a separate shell-script wrapper, not the overlay.

Run:

```sh
nix run .#bossbar-overlay     # boss bar HUD across the top of the screen
nix run .#book-overlay        # floating open book
nix run .#xp-orb-overlay      # floating experience orb
nix run .#xp-orb-overlay -- feed   # full-screen "rise & pop" karma feed
```

Platforms: `aarch64`/`x86_64` darwin and linux (`app/default.nix:101`). On Linux,
`app/default.nix` patchelfs and wraps each binary with the X11/Wayland/Vulkan
runtime libraries it dlopens (`app/default.nix:86`).

## Common app shape

Every app is a winit + wgpu loop (no webview) following the same pattern:
`main.rs` parses args and env, resolves the DB path, then either runs a headless
`--snapshot OUT` to a transparent PNG and exits, or runs the live overlay
(`crates/bossbar/src/main.rs:89`). Scale is set by an env var or `--scale`
(`BOSSBAR_SCALE` / `BOOK_SCALE` / `ORB_SCALE`). Each app has `assets.rs`
(embedded Mojang PNGs via `include_bytes!`), `db.rs` (the SQLite source +
watcher), `scene.rs` (build a `Vec<Quad>`), and `overlay.rs` (the winit
`ApplicationHandler`); the boss bar also has `layer_shell.rs` for Wayland and
`theme.rs` for user textures.

The SQLite contract is identical across apps (`crates/bossbar/src/db.rs:1`): open
in WAL mode, create the schema if missing, run additive `ALTER TABLE` migrations,
seed example rows only when the file is newly created, and poll `PRAGMA
data_version` every 200ms (`POLL`, `db.rs:20`) to detect any other connection's
commit. Dragged positions persist to `x`/`y` via a separate write connection
(`db.rs:120`).

## boss bar (`bossbar`)

Each bar is its own small float window sized to that bar, so only the bars
intercept the mouse. Bars render from Mojang's actual boss bar sprites layered
the way the game's `BossHealthOverlay` does (color background, color progress
clipped to the fill, then the notch overlay). Hover eases to opaque with a
breathing pulse; a press past a few pixels starts a native window drag (saved to
`x`/`y`); a stationary click opens the bar's `url`; a `description` unfolds a
panel; a `since` shows a live elapsed timer in the title.

Typed domain in `crates/bossbar/src/bars.rs`: `Color` (7 variants,
`bars.rs:10`), `Overlay`/`Notch` (smooth or `notched_6/10/12/20`, `bars.rs:54`),
and `BossBar` (`bars.rs:97`). Unknown color/overlay strings fall back to
`purple`/smooth so a typo still draws a bar (`bars.rs:23`).

Data contract (`bossbars` table, `crates/bossbar/src/db.rs:22`): `id, title,
description, progress (0..1), color, overlay, visible, position, x, y, since
(epoch), url, expandable, eta, icon, theme`. Notes:
- `since`+`eta` together make the fill extrapolate live as `(now-since)/eta`,
  ignoring the static `progress` (`bars.rs:113`).
- `expandable=0` keeps a `description`-carrying bar bar-sized on hover, for many
  compact bars (`bars.rs:120`).
- `icon` is a path to a small image drawn left of the title (e.g. a PR author's
  avatar); a missing/undecodable path is skipped (`bars.rs:135`).
- `theme` names a user-supplied texture set; see below.

DB path: `$BOSSBAR_DB`, else `<data-dir>/bossbar-overlay/bossbars.db`
(`db.rs:70`). Snapshot: `--snapshot out.png [--scale N] [--size WxH]`.

### Themed textures (`theme.rs`)

A non-empty `theme` renders a bar from a user-supplied directory under the themes
root (`$BOSSBAR_THEMES`, else `themes/` next to the DB, `theme.rs:68`). Required
`background.png`/`progress.png`, optional per-notch overrides; any PNG size is
drawn into the bar's 182:5 box (`theme.rs:139`). Theme names are validated as a
single safe path component, so `../escape` is just an unknown theme that draws
vanilla (`theme.rs:85`). Sprites are downscaled to <=2048px and memoized
(`theme.rs:113`). This repo ships no themed art; import your own with
`app/scripts/import-theme.sh`.

### bossbar CLI (`.#bossbar`)

`bossbar` is a hand-written shell script wrapped with `sqlite3` + coreutils on
PATH (`cli.nix:8`). It writes the same DB the overlay reads, works whether or not
the overlay is running, and creates the schema on demand:

```sh
bossbar add "Ender Dragon" --color pink --overlay notched_20 --progress 0.8
bossbar set "Ender Dragon" --progress 0.5     # by title ...
bossbar set 1 --color red --visible 1         # ... or by id
bossbar list ; bossbar rm "Ender Dragon" ; bossbar db
```

The `services.ciBars` home module
(`packages/minecraft/bossbar-overlay/ci-bars-home-module.nix`) drives this DB to draw one
bar per in-flight GitHub Actions run.

## book (`book`)

One floating window shows an open book as a two-page spread (Mojang's one-page
texture drawn twice, mirrored on the left). Page-turn arrows advance, drag/scroll
moves it (position saved), hover raises it. Schema (`crates/book/src/db.rs:19`):
a singleton `book` row (`id=1`, `title`, `x`, `y`) and a `pages` table (`id`,
`idx` order, `body` with newlines as paragraph breaks). Current page is in-memory
state; only position persists. DB: `$BOOK_DB`, else
`<data-dir>/book-overlay/book.db` (`db.rs:42`). Turning a page plays a vanilla
flip sound by shelling out to `minecraft-sound` (`crates/book/src/sound.rs:32`,
override `BOOK_SOUND_CMD`).

## orb (`orb`)

A floating experience orb that bobs and shimmers; its XP `amount` picks the orb
size. Two modes (`crates/orb/src/main.rs:10`): the default pinned single-orb
overlay, and `feed`, a full-screen "rise & pop" karma feed. `push TEXT [--amount
N] [--kind orb|villager]` queues one labelled pop and exits. Schema
(`crates/orb/src/db.rs:20`): singleton `orb` row (`id=1`, `amount`, `url`, `x`,
`y`) and an append-only `events` table (`text`, `amount`, `kind`, `created`) the
feed consumes. DB: `$ORB_DB`, else `<data-dir>/xp-orb-overlay/orb.db`
(`db.rs:42`). A success pop plays the orb-pickup sound, a failure (villager) pop
plays a cycled villager "no" grunt, again via `minecraft-sound`
(`crates/orb/src/sound.rs:22`, override `ORB_SOUND_CMD`).

## Limits

No tray icon; quit like any foreground process. Click-through is structural (no
window where no overlay sits), not `set_cursor_hittest`, so some Wayland tiling
compositors may still fight free placement (`README.md:218`). The boss bar covers
the primary monitor only; the book opens centered there.
