# minecraft

`images/games/minecraft` builds a Java Minecraft server. The image directory is a
version-agnostic base plus a `versions.nix` sidecar, so discovery produces one
package per version variant (`.#minecraft_<key>`) and a `.#minecraft` alias for
the default variant. This is the only image in the tree that uses the
`versions.nix` multi-version mechanism (`README.md:61`, `nix build .#minecraft`).

## What it builds

`images/games/minecraft/default.nix` (23 lines) is the version-agnostic base:

- `ix.image.name = "minecraft"` (`:16`).
- enables the server and seeds the cross-version `common` mod set so every
  variant ships the baseline performance/QoL mods (`:11-22`):

```nix
commonCatalog = ix.artifacts.minecraft.modCatalogs.common;
services.minecraft = {
  enable = true;
  properties.motd = "ix-powered Minecraft";
  mods = lib.genAttrs (lib.attrNames commonCatalog) (_: { });
};
```

It deliberately does NOT set `version` or a loader; the version overlay does.

## Per-version variants (`versions.nix`)

`images/games/minecraft/versions.nix` declares each variant as `{ loader,
version, mods }` and renders it into a module that sets `ix.image.tag`,
`services.minecraft.version`, the enabled mod slugs, and
`services.minecraft.<loader>.enable` (`:65-79`). `default = "26.1.2-fabric"`
(`:13`). The variants (`:15-63`):

| variant key (`.#minecraft_<key>`) | loader | version | notable mods |
| --- | --- | --- | --- |
| `26w17a-fabric` | fabric | 26.2-snapshot-5 | fabric-api, c2me-fabric |
| `26.1.2-fabric` (default `.#minecraft`) | fabric | 26.1.2 | lithium, krypton, ferrite-core, servercore, vmp-fabric, spark, grimac, ... |
| `26.1.2-paper` | paper | 26.1.2 | (none; Paper) |
| `1.21.11-fabric` | fabric | 1.21.11 | fabric-api, spark, terrain-diffusion |
| `1.21.11-paper` | paper | 1.21.11 | (none; Paper) |

Discovery maps each key to `minecraft_<key>` and aliases `minecraft` to the
`default` key (`lib/discovery.nix:81-111`); the discovery layer asserts the
`default` key exists (`:107-108`). The `mods` list per variant is unioned with
the base `common` set from `default.nix`.

## Composed module: `services.minecraft`

Defined in `modules/services/minecraft/default.nix`. It is loader-agnostic: a
loader module fills the `serverJar` and `dropinDir` slots via module merge
(`:1-7`). `version` is the single source of truth (`:809-821`): a loader derives
the jar from `ix.artifacts.minecraft.servers."${version}-${loader}"` and the
default `modCatalog` is `modCatalogs.common` merged with `modCatalogs.${version}`
(`:858-872`, `lib/util/artifacts.nix:44-48`). Key surface:

- `enable` (`:807`), `version` (`:809`), `serverJar` slot (`:823-826`),
  `dropinDir` (default `mods`; paper uses `plugins`, `:828-832`).
- `mods` keyed by Modrinth slug, `plugins`, `datapacks`, `players`, `worlds`,
  `worldBorder` (`:840-856,880-884,219-279`).
- `properties` (freeform `server.properties`; `port` default 25565 seeds
  `server-port`; defaults include `online-mode = true`, `view-distance = 32`,
  `:1028-1061,1130-1150`).
- `openFirewall` (default true, `:1063`), `rcon` (default off, port 25575,
  `:934-947`), `maxRAMPercentage` (default 85, JVM auto-scales to VM RAM,
  `:834-838`), `jvmFlags` (Aikar's G1GC flags, `:902-932`), `javaPackage`
  (Temurin JRE, `:900`).
- Loaders are sibling modules under `modules/services/minecraft/<loader>/`,
  built via `ix.mkMinecraftLoader` (e.g. fabric:
  `modules/services/minecraft/fabric/default.nix:15-24`), all present in every
  image and gated on `services.minecraft.<loader>.enable`.

Runtime wiring (`:1167-1285`): claims TCP `port` (`minecraft`) plus RCON when
enabled, opens the firewall for `openFirewall`, declares two health checks
(`minecraft` unit active + `minecraft-status` SLP probe via
`ix.packages.mc-probe` against `127.0.0.1:<port>`, asserting the configured MOTD;
plus a host-side reachability check when the firewall is open), writes managed
dropins/datapacks/config/server-files/access via `environment.etc`, and runs
`systemd.services.minecraft` with `eula=true` written at `preStart`
(`:1280-1282`) and hardened service config.

## Build

```
nix build .#minecraft            # default variant (26.1.2-fabric)
nix build .#minecraft_1.21.11-paper
```

## Eval test (`tests/default.nix:3456+`)

The `minecraft` test group exercises the loader/version wiring and the SLP health
check command (`mc-probe 127.0.0.1:<port> --motd-contains ...`), so a misrouted
loader or a wrong port lands as a check failure rather than a bad server.

## Related images

- [minecraft-status](../minecraft-status/overview.md): a stripped Fabric variant
  used as the ix status canary.
- [minecraft-bedrock](../minecraft-bedrock/overview.md): the native Bedrock
  server (separate module family).
- [minestom](../minestom/overview.md): a from-scratch JVM server library, no
  loaders/mods/EULA.
