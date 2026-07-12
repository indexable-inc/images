# minecraft tools

`packages/minecraft` is a directory of small, single-purpose Minecraft tools in
several languages. The Rust members live in the repo's top-level Cargo
workspace; the rest are Python scripts, a Kotlin CLI, and a Java agent. None
of them run a server: they encode data, play sounds, reconcile files, probe a
server, record a session, or hot-reload plugin classes. The sibling subtrees
`minestom/`, `bossbar-overlay/`, and `minecraft-assets/` are their own doc
domains; `catalogs/` holds generated version catalogs and `tools/` their
maintenance scripts. The wire-protocol
logic they share lives in one Rust crate (`protocol`), rendered into Python
and JVM bindings by unibind (`packages/unibind`). The NixOS modules and
images that consume them are cross-referenced, not documented here (see
common).

| member | lang | entry | flake output |
| --- | --- | --- | --- |
| `nbt` (`minecraft-nbt`) | Rust | `nbt/src/main.rs` | `.#minecraft-nbt` |
| `sound` (`minecraft-sound`) | Rust | `sound/src/main.rs` | `.#minecraft-sound` |
| `sync-managed` (`minecraft-sync-managed`) | Rust | `sync-managed/src/main.rs` | `.#minecraft-sync-managed` |
| `protocol` (`mc-protocol`) | Rust lib (+ `py`/`jvm` binding crates) | `protocol/src/lib.rs` | none (library) |
| `probe` (`mc-probe`) | Python | `probe/mc_probe.py` | `.#mc-probe` |
| `probe-kt` (`mc-probe-kt`) | Kotlin | `probe-kt/src/McProbe.kt` | `.#mc-probe-kt` |
| `bot` (`mc-bot`) | Rust | `bot/src/main.rs` | `.#mc-bot` |
| `rcon` (`minecraft-rcon`) | Python | `rcon/minecraft-rcon.py` | overlay only |
| `hot-reload-agent` | Java | `hot-reload-agent/src/dev/ix/minecraft/hotreload/HotReloadAgent.java` | overlay only |

## minecraft-nbt: JSON -> NBT/SNBT

Encodes a JSON description of an NBT document into Mojang binary NBT or
stringified NBT (SNBT). It is a pure encoder: there is no decode-from-binary
path.

- **Library surface** (`nbt/src/lib.rs`): `pub fn decode_document(value: &Value,
  default_root_name: &str) -> Result<Document>` (`lib.rs:21`) and `pub struct
  Document { root_name, compound }` (`lib.rs:9`). Constants `TAG_KEY =
  "__minecraftNbt"` and `VALUE_KEY = "value"` (`lib.rs:5`) define the wrapper
  convention.
- **Input format.** A value is either an implicit type (JSON object -> compound,
  array -> homogeneous list, string -> string, bool -> byte, integer -> int or
  long by range, float -> double) or an explicit `{"__minecraftNbt": <tag>,
  "value": v}` wrapper. Tags: `byte short int long float double string byteArray
  intArray longArray list compound` plus the document-only `root` wrapper that
  also carries a `name` (`lib.rs:22`, `lib.rs:64`). The matching producer is
  `lib/minecraft/nbt.nix` (lib domain).
- **Validation invariants.** Integer tags are range-checked
  (`-128..=127` for byte, etc., `lib.rs:248`); floats must be finite and within
  32-bit range for `float` (`lib.rs:239`); lists must be homogeneous
  (`lib.rs:155`); the document root must be a compound (`lib.rs:36`); `null` has
  no NBT tag (`lib.rs:60`).
- **CLI** (`nbt/src/main.rs:16`): `--format nbt|snbt` (required), `--flavor
  uncompressed|gzip|zlib` (default uncompressed; binary only), `--root-name`
  (default empty), `--input <json>`, `--output <file>`. Binary uses
  `quartz_nbt::io::write_nbt`; SNBT uses `to_pretty_snbt` (`main.rs:77`,
  `main.rs:87`).
- **Tests.** Unit tests cover explicit tags and rejections (`lib.rs:308`).
  Hegel property tests assert the decoder never panics on arbitrary JSON and that
  byte round-trips match the range check (`nbt/tests/property.rs:42`). A
  `cargo-fuzz` target lives at `nbt/fuzz/fuzz_targets/decode_document.rs` and is
  run by `.github/workflows/fuzz-nbt.yml`.
- **Build/wiring.** `nbt/default.nix` builds via
  `ix.cargoUnit.selectBinaryWithTests`. `lib/minecraft/nbt-format.nix:22` builds
  the tool with `buildIxRustTool` and exposes a `pkgs.formats`-style `generate`
  that shells out to `minecraft-nbt` (lib domain cross-ref).

## minecraft-sound: local sound-effect player

Plays a Minecraft sound `.ogg` by name (e.g. `mob/zombie/death`) on the default
audio device, via `rodio`.

- **CLI** (`sound/src/main.rs:25`): `list [PATTERN]` and `play <SOUND> [--wait]
  [--pitch P] [--volume V]`. An empty/omitted name is a silent no-op (so a hook
  can disable an event with `""`); a non-empty unknown name is a loud error
  (`main.rs:113`). Without `--wait`, it re-spawns itself detached in
  `--foreground --wait` mode so the caller returns immediately, but resolves the
  name first so a typo fails before spawning (`main.rs:122`).
- **Asset resolution** (`sound/src/assets.rs:84`): `MinecraftAssets::Bundled`
  reads a flat `<name>.ogg` tree from `$MCSOUND_ASSETS`; `MinecraftAssets::Install`
  auto-detects a Minecraft install (`$MINECRAFT_HOME`, else the per-OS default,
  `assets.rs:319`) and maps names through the highest-versioned
  `assets/indexes/<n>.json` to hashed objects. An unknown name yields
  `UnknownSound` with up to three Levenshtein-close suggestions (`assets.rs:238`).
- **Playback** (`sound/src/audio.rs:72`): pitch is Minecraft-style (speed + pitch
  together, no resampling) clamped to `[0.5, 2.0]` (`audio.rs:10`); volume is a
  non-negative linear multiplier.
- **Build/wiring.** `sound/default.nix` wraps the binary and bakes the Nix sound
  pack in with `--set-default MCSOUND_ASSETS` (`sound/default.nix:29`), so it
  plays with zero config and no Minecraft install. The pack itself is
  `sound/sounds.nix`: a fixed-output derivation that downloads sounds from
  Mojang's CDN, pinned by `sound/sounds/lock.json` (Minecraft `26.1.2`, asset
  index `30`), excluding `music`/`records`. That store path contains Mojang
  content and MUST NOT be pushed to a shared cache (`sounds.nix:3`). This binary
  is what the [book and orb overlays](../bossbar-overlay/overview.md) shell out
  to for sound cues.

## minecraft-sync-managed: reconcile managed files

Reconciles ix-owned input trees into a running server's mutable data directory by
symlinking managed files, removing files it previously managed but no longer
owns, merging access lists by UUID, planning PlugManX hot-reloads, and
configuring RCON.

- **CLI** (`sync-managed/src/main.rs:12`): `--data-dir`, `--dropin-dir`,
  `--managed-root`, `--plugman-reload`, `--rcon-enable`, repeatable
  `--plugman-ignored-plugin` and `--datapack-world`, `--rcon-port`,
  `--rcon-password-file`, `--rcon-broadcast-to-ops`. The flag list is assembled
  by `lib/minecraft/sync-managed.nix:50` and run by `services.minecraft`.
- **Sync model** (`main.rs:157` `sync_tree`): for each tree (`managed-dropins`
  -> `<dropin-dir>`, `managed-config` -> `config`, `managed-server-files` ->
  data root, `managed-datapacks` -> `<world>/datapacks`), it deletes files
  recorded in the prior `.ix-managed-<name>` manifest, then symlinks the current
  managed files and rewrites the manifest as `rel <abs source>` lines. Paths are
  validated against traversal and shell-unsafe characters (`main.rs:80`,
  `main.rs:65`); `ops.json`/`whitelist.json` are preserved from the
  server-files sweep (`main.rs:734`).
- **Access reconcile** (`main.rs:679`): `whitelist.json` and `ops.json` are
  three-way merged by player `uuid` (current vs previous-applied vs desired), so
  manual entries survive and managed entries update; duplicate or missing UUIDs
  are a hard error (`main.rs:604`).
- **PlugManX plan** (`main.rs:430`): emits a `.reload-plan` of `load`/`reload`/
  `unload` lines for changed plugin jars and changed per-plugin config files, in
  a deterministic order, skipping `PlugManX.jar` and ignored plugins.
- **RCON** (`main.rs:526`): generates a password from two kernel UUIDs if absent
  (chmod 0600), de-symlinks `server.properties`, and sets `enable-rcon`,
  `rcon.port`, `rcon.password`, `broadcast-rcon-to-ops`.
- **Build.** `sync-managed/default.nix` via `ix.cargoUnit.selectBinaryWithTests`.

## mc-protocol: the shared wire-protocol crate

One Rust implementation of the protocol pieces the repo speaks
(`protocol/src/lib.rs`): VarInt encoding and packet framing (`varint`),
length-prefixed strings (`wire`), the Server List Ping status exchange
(`slp`), and legacy `§`/`&` format-code stripping for MOTD comparison
(`text`). Every probe and test harness goes through it, whatever its
language:

- **Python** — `protocol/py`, unibind-rendered pyo3 bindings imported as
  `mc_protocol`; consumed by mc-probe.
- **JVM** — `protocol/jvm`, the same three-call surface (parse address,
  status, strip format codes) rendered by unibind's jvm backend into C-ABI
  shims plus one generated FFM class (`McProtocolJvm`); consumed by
  mc-probe-kt.
- **Rust** — consumed directly by mc-bot for its packet layer.

Non-goal: SRV record lookup. In-repo health checks and tests address servers
by explicit `host:port`; resolve SRV records before calling in.

## mc-probe: Server List Ping health check

An exit-code probe for fleet health checks (`probe/mc_probe.py`), a single
Python script over the `mc_protocol` bindings.

- **CLI** (`mc_probe.py:56`): `mc-probe <host[:port]>` with optional
  `--motd-contains SUBSTRING` (repeatable; strips `section`/`&` format codes both
  sides), `--protocol-version N`, `--min-max-players N`,
  `--timeout SECONDS` (default 5).
- **Contract.** Exit 0 only if the SLP handshake answered and every requested
  assertion held; otherwise each failure is named on stderr and it exits 1.
  Built with `ix.writePythonApplication` over a `python3.withPackages` that
  carries the unibind-built `mc_protocol` module (`probe/default.nix`).

## mc-probe-kt: the same probe, from Kotlin

The JVM twin of mc-probe (`probe-kt/src/McProbe.kt`): same flags, same
exit-code contract, but speaking the wire format through `McProtocolJvm`,
the unibind-rendered FFM class over the Rust crate. It exists to prove the
JVM binding surface end-to-end against a live server (the spleef VM test
runs both probes), and as the seed for JVM-side test harnesses.
`probe-kt/default.nix` compiles the generated class + the Kotlin CLI with
JDK 25/kotlinc and wraps a launcher that passes
`--enable-native-access=ALL-UNNAMED` and the native library location.

## mc-bot: record a session as a ReplayMod replay

A headless Minecraft client (`bot/src/main.rs`) that joins a server as a
real offline-mode player — full login, registry configuration, spawn — sits
in the play state answering keep-alives, and records every clientbound
packet into a ReplayMod `.mcpr` archive (a zip of `recording.tmcpr`, the
timestamped packet stream, plus `metaData.json`). Open the file in ReplayMod
to scrub through exactly what the client saw; the spleef VM test exports one
as a test artifact so a failing e2e run leaves a client-side trace.

- **CLI**: `mc-bot <host[:port]> --protocol-version N --mc-version NAME
  [--username U] [--record-seconds S] [--timeout S] --output FILE.mcpr`.
  The protocol version is explicit at the call site, like the probes', so
  version bumps are deliberate.
- **Scope.** Offline login only (an encryption request is a hard error); the
  bot never moves or interprets game content — packet ids it must answer
  (keep-alive, ping, the configuration handshake) are transcribed from the
  pinned Minestom's `PacketRegistry` in `bot/src/packets.rs`. The replay is
  written even when the session ends in a kick or network failure.
- **Build.** `bot/default.nix` via `ix.cargoUnit.selectBinaryWithTests`;
  framing/`VarInt`/string primitives come from mc-protocol.

## minecraft-rcon: minimal RCON client

A dependency-free Source RCON client (`rcon/minecraft-rcon.py`): packs and reads
the little-endian RCON frame format itself (`AUTH=3`, `COMMAND=2`,
`minecraft-rcon.py:11`).

- **CLI** (`minecraft-rcon.py:44`): `--host` (default `127.0.0.1`), `--port`
  (required), one of `--password` / `--password-file`, then the command words.
  Authenticates, prints any server output, exits 1 on auth failure
  (`minecraft-rcon.py:77`). Built with `writePythonApplication`; exposed only as
  `pkgs.minecraft-rcon` (overlay, no flake output). The RCON password it speaks
  is the one `minecraft-sync-managed` provisions.

## minecraft-hot-reload-agent: dev JVM agent

A development-only Java agent (`HotReloadAgent.java:25`) that redefines loaded
plugin classes in place so a server picks up rebuilt code without a restart.

- **Mechanism.** Loaded as `premain`/`agentmain` (`HotReloadAgent.java:31`), it
  starts a daemon thread serving a Unix domain socket (default
  `/run/minecraft-hot-reload/socket`, override `socket=<path>`). The line
  protocol is `PING` -> `OK pong` and `REDEFINE_DIR <dir>` -> iterate `*.jar`,
  redefine every already-loaded, modifiable class via
  `Instrumentation.redefineClasses`, and report counts/failures
  (`HotReloadAgent.java:117`). Its own `main` is a tiny client for the same
  socket (`HotReloadAgent.java:187`).
- **Build.** `hot-reload-agent/default.nix` compiles with `jdk25` (`--release
  21`) and jars it with `MANIFEST.MF`; exposed as
  `pkgs.minecraft-hot-reload-agent` (overlay only). Used by the
  `services.minecraft` dev path (modules domain cross-ref).
