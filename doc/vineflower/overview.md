# vineflower

`packages/vineflower` packages [Vineflower](https://vineflower.org/), the
actively-maintained fork of Fernflower, the Java decompiler. It downloads the
upstream release jar and wraps it in a `java -jar` launcher; there is no
recompilation.

## What this repo changes

`default.nix` fetches the release jar and writes a thin launcher
(`packages/vineflower/default.nix`):

- Source: `fetchurl` of
  `vineflower-${version}.jar` from the upstream GitHub release, pinned by
  `version = "1.12.0"` and an inline SRI `hash`
  (`packages/vineflower/default.nix:9-13`).
- Build: `stdenvNoCC.mkDerivation` with `dontUnpack = true`; the install phase
  copies the jar to `$out/share/java/vineflower.jar` and writes
  `$out/bin/vineflower`, a shell script that `exec`s `${jdk}/bin/java -jar`
  against it with `"$@"` (`packages/vineflower/default.nix:25-34`). `jdk` is the
  nixpkgs default JDK, baked into the launcher path.
- `meta`: `description` "Modern fork of Fernflower...", `license = asl20`,
  `mainProgram = "vineflower"`, `platforms = unix`
  (`packages/vineflower/default.nix:37-43`).

This is the JVM-tool shape: the repo owns only the version pin and the launcher
that hard-codes the JDK, so the tool runs without the user having a `java` on
PATH.

## Build and wiring

- Flake output: `package.nix` carries only `id = "vineflower"`
  (`packages/vineflower/package.nix`); with no `packageSet`/`flake`/`overlay`
  target set, it surfaces in the package set under its id but is not exposed as
  a top-level `nix run` flake output. It is consumed directly through the
  registry: the Minecraft decompile-server tool callPackages it as a runtime
  input (`lib/per-system.nix:372`).
- Bump: edit `version` and refresh the `hash` with `nix-prefetch-url`. There is
  no `manifest.json` and no `updateScript`; the pin is inline in `default.nix`.
