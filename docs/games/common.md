# games

Minecraft data/server tooling and Minecraft-styled desktop overlays. This domain
holds the buildable artifacts under `packages/`: small command-line tools that
read and write Minecraft data formats, fetch and play Mojang assets, reconcile a
running server's mutable files, probe a server over the wire, a from-scratch
Minestom server jar, and a reusable transparent-overlay engine plus three
SQLite-driven desktop HUDs drawn in the Minecraft style. These are the leaf
tools; the Nix library helpers that wrap them (`lib/minecraft/**`), the NixOS
service modules that run servers (`modules/services/{minecraft,minecraft-bedrock,minestom}`),
and the runnable server images (`images/games/**`) live in other domains and are
cross-referenced here, not documented here.

Read this page first, then the component pages it links.

## Units

| unit | kind | flake output | what |
| --- | --- | --- | --- |
| `packages/minecraft/nbt` (`minecraft-nbt`) | Rust bin (workspace member) | `.#minecraft-nbt` | JSON -> binary NBT / SNBT encoder. See [minecraft](minecraft/overview.md). |
| `packages/minecraft/sound` (`minecraft-sound`) | Rust bin (workspace member) | `.#minecraft-sound` | local Minecraft sound-effect player + bundled Mojang pack. |
| `packages/minecraft/sync-managed` (`minecraft-sync-managed`) | Rust bin (workspace member) | `.#minecraft-sync-managed` | reconcile ix-managed files into a live server data dir. |
| `packages/minecraft/probe` (`mc-probe`) | Python (uv) | `.#mc-probe` | exit-code Server-List-Ping health probe. |
| `packages/minecraft/rcon` (`minecraft-rcon`) | Python script | overlay only (`pkgs.minecraft-rcon`) | minimal RCON client. |
| `packages/minecraft/hot-reload-agent` | Java agent (jar) | overlay only (`pkgs.minecraft-hot-reload-agent`) | dev JVM agent: redefine plugin classes over a Unix socket. |
| `packages/minecraft-assets` | Nix-only FOD | `.#minecraft-assets` | extract GUI textures + bitmap font from Mojang's client jar. See [minecraft-assets](minecraft-assets/overview.md). |
| `packages/minestom/servers/hello` | Gradle fat jar (Java) | `.#minestom-hello-server-jar` | example Minestom server. See [minestom](minestom/overview.md). |
| `packages/bossbar-overlay/app/crates/overlay-core` | Rust lib (own workspace) | (internal) | reusable float-window + wgpu pixel/text engine. See [bossbar-overlay engine](bossbar-overlay/engine.md). |
| `packages/bossbar-overlay/app/crates/{bossbar,book,orb}` | Rust bins (own workspace) | `.#bossbar-overlay` / `.#book-overlay` / `.#xp-orb-overlay` | the three desktop overlays. See [bossbar-overlay](bossbar-overlay/overview.md). |
| `packages/bossbar-overlay/bossbar` | shell script + Nix wrapper | `.#bossbar` | CLI over the boss bar SQLite DB. |

Rust crates `minecraft-nbt`/`minecraft-sound`/`minecraft-sync-managed` are members
of the repo's top-level Cargo workspace. The overlay crates are a separate,
self-contained Cargo workspace under `packages/bossbar-overlay/app` (root
`Cargo.toml:10`), kept off the main graph so winit/wgpu deps and lints do not
touch the rest of the repo.

## How it fits together

Two loosely coupled clusters share one domain:

```
SERVER TOOLING                                  DESKTOP OVERLAYS
  lib/minecraft/nbt.nix (__minecraftNbt JSON)     minecraft-assets (Mojang client.jar)
      -> minecraft-nbt --format nbt|snbt              -> font + sprites baked into
  modules/services/minecraft                              overlay-core / bossbar / book / orb
      -> minecraft-sync-managed (reconcile)         bossbar / book / orb  <--driven by-- SQLite file
      -> minecraft-hot-reload-agent (dev)               -> book/orb shell out to minecraft-sound
  mc-probe / minecraft-rcon  --over the wire-->         bossbar CLI ----writes----> bossbars.db
  minestom-hello-server-jar  (services.minestom)
```

- **NBT pipeline.** `lib/minecraft/nbt.nix` (lib domain) emits a JSON tree of
  `{"__minecraftNbt": <tag>, "value": ...}` wrappers; `minecraft-nbt` decodes
  exactly that wrapper format and writes binary NBT or SNBT. `lib/minecraft/nbt-format.nix:22`
  runs the binary as a `pkgs.formats`-style generator.
- **Managed-server reconcile.** `minecraft-sync-managed` is invoked (with a long
  flag list) by `lib/minecraft/sync-managed.nix:53`, which the
  `services.minecraft` module wires into the server's start sequence.
- **Asset provenance.** `minecraft-assets` is a fixed-output derivation that
  fetches Mojang's `client.jar` (pinned by Mojang's own hash) and unzips the
  exact texture and font paths the overlays embed. `packages/bossbar-overlay/app/default.nix:73`
  copies those slices into each crate's `assets/` before compiling, so no Mojang
  art is committed to the repo.
- **Sound bridge.** The book and orb overlays play cues by shelling out to the
  `minecraft-sound` binary (`crates/book/src/sound.rs:27`, `crates/orb/src/sound.rs:32`),
  overridable via `BOOK_SOUND_CMD` / `ORB_SOUND_CMD`. No audio backend is linked
  into the overlays.
- **One SQLite file per overlay.** Each overlay reads exactly one SQLite DB and
  re-reads it within ~200ms of any external write. Anything (a shell, the
  `bossbar` CLI, a cron job, the `services.ciBars` module) can write rows.

## Invariants

- **Untrusted input falls back, never crashes.** NBT decoding rejects bad input
  with typed errors (`packages/minecraft/nbt/src/lib.rs:21`); a fuzz target
  asserts the decoder never panics (`nbt/tests/property.rs:42`). Overlay row
  parsing defaults an unknown color/overlay to `purple`/smooth rather than
  drawing nothing (`crates/bossbar/src/bars.rs:23`). A bad sound name errors
  loudly with suggestions (`crates/.../sound assets`), and a theme name that
  escapes its directory is treated as an unknown theme (`crates/bossbar/src/theme.rs:85`).
- **Mojang art is fetched, not vendored.** Both `minecraft-assets` and the
  `minecraft-sound` pack are fixed-output derivations that download from Mojang
  at build time; the sound pack carries a DO-NOT-UPLOAD banner
  (`packages/minecraft/sound/sounds.nix:3`) and must stay out of shared caches.
- **SQLite change detection uses `PRAGMA data_version`.** Overlays poll it every
  200ms (`crates/bossbar/src/db.rs:20`); it bumps on any other connection's
  commit, so WAL writes are never missed the way file-mtime watching can be.
- **Overlay placement is OS-specific.** macOS uses winit plus raw AppKit
  (non-activating raise, background hover, context menu); wlroots Wayland uses a
  layer-shell backend (`crates/bossbar/src/layer_shell.rs:1`). Click-through is
  structural: there is simply no window where no overlay sits.

## Glossary

- **NBT**: Named Binary Tag, Minecraft's binary tree format. **SNBT** is its
  stringified form.
- **`__minecraftNbt` wrapper**: the JSON convention (`{"__minecraftNbt": tag,
  "value": v}`) that `minecraft-nbt` decodes; produced by `lib/minecraft/nbt.nix`.
- **managed-* tree**: ix-owned input directories (`managed-dropins`,
  `managed-config`, `managed-server-files`, `managed-datapacks`, `managed-access`)
  that `minecraft-sync-managed` symlinks/reconciles into a server's mutable data
  dir, tracked by `.ix-managed-*` manifests.
- **SLP**: Server List Ping, the handshake `mc-probe` asserts against.
- **Minestom**: a from-scratch Java server library (not a Mojang server fork);
  no loaders, mods, or EULA.
- **overlay-core**: the shared engine (float window + one textured-quad wgpu
  pipeline + bitmap font) every desktop overlay builds on.
- **quad**: one textured rectangle in physical pixels; the overlays' only draw
  primitive, including each glyph of text.
- **float window**: a transparent, borderless, always-on-top, click-through,
  drag-to-move overlay window with no Dock presence.

## Components

| component | page | what |
| --- | --- | --- |
| minecraft tools | [minecraft/overview.md](minecraft/overview.md) | nbt, sound, sync-managed, probe, rcon, hot-reload-agent |
| minecraft-assets | [minecraft-assets/overview.md](minecraft-assets/overview.md) | reproducible Mojang texture + font extraction |
| minestom | [minestom/overview.md](minestom/overview.md) | example Minestom server fat jar |
| bossbar-overlay | [bossbar-overlay/overview.md](bossbar-overlay/overview.md) | the three SQLite-driven desktop overlays + CLI |
| overlay engine | [bossbar-overlay/engine.md](bossbar-overlay/engine.md) | `overlay-core` float window + wgpu pixel/text engine |
