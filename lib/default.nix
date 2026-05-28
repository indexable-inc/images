# ix/images public lib. Helpers documented per binding with RFC-0145
# doc-comments below; the file's job is to wire them together.
{
  nixpkgs,
  paths,
  rust-overlay,
  determinate,
  home-manager,
  hermes-agent,
  clippy-fork,
  cliArtifacts ? { },
}:
let
  inherit (nixpkgs) lib;

  system = "x86_64-linux";

  packageRegistry = import (paths.packagesRoot + "/registry.nix") {
    inherit lib;
    root = paths.packagesRoot;
  };
  packagePath =
    id:
    let
      entry = packageRegistry.byId.${id} or (throw "ix.lib: package registry has no `${id}` entry");
    in
    entry.path;

  inherit (import ./writers.nix { inherit lib errors basedpyrightTypeCheckingModes; })
    writePythonApplication
    writeNushellApplication
    writeProcessComposeApplication
    ;

  /**
    Repo-local nixpkgs overlay.

    Exposes the few repo-owned packages that NixOS modules expect to find
    as `pkgs.<name>`. Flake-output-only packages live in `packageSetFor`
    instead so they don't leak into the nixpkgs namespace inside images.
  */
  overlay = import ./overlay.nix {
    inherit
      lib
      packageRegistry
      buildIxRustTool
      clippy-fork
      writePythonApplication
      ;
  };
  overlays = [ overlay ];

  /**
    nixpkgs instance with the repo overlay applied, evaluated for
    `x86_64-linux`. Use this when the image build needs `pkgs` directly.
  */
  pkgs = import nixpkgs { inherit system overlays; };

  # Auto-discovered NixOS module registry. The walk lives next to
  # `discoverImages` for symmetry; see its doc-comment for the discovery rules.
  nixosModules = discoverModules { root = paths.modules; };

  # Flat list of module paths from the auto-discovered registry under
  # `modules/`. Pulled in unconditionally so every option is in scope; each
  # module stays inert until its `enable` flag is set.
  moduleList = lib.collect builtins.isPath nixosModules;

  bunLockFor =
    pkgs:
    import ./bun-lock.nix {
      inherit lib pkgs;
    };
  buildBunSite = import ./build-bun-site.nix {
    inherit bunLockFor;
  };
  buildNpmSite = import ./build-npm-site.nix;
  buildSvelteSite = import ./build-svelte-site.nix {
    inherit bunLockFor errors writeNushellApplication;
  };
  buildNpmVitest = import ./build-npm-vitest.nix;
  buildZigPackage = import ./build-zig-package.nix { };
  uvLockFor =
    pkgs:
    import ./uv-lock.nix {
      inherit lib pkgs;
    };
  basedpyrightTypeCheckingModes = [
    "off"
    "basic"
    "standard"
    "strict"
    "recommended"
    "all"
  ];
  buildUvApplication = import ./build-uv-application.nix {
    inherit errors uvLockFor;
    validTypeCheckingModes = basedpyrightTypeCheckingModes;
  };
  buildGradleFatJar = import ./build-gradle-fat-jar.nix { inherit lib; };
  secrets = import ./secrets.nix {
    inherit lib pkgs writeNushellApplication;
  };
  agentsMd = import ./agents-md.nix { inherit lib paths; };
  languages = {
    cpp = import ./languages/cpp.nix { inherit errors; };
    dhall = import ./languages/dhall.nix { };
    elixir = import ./languages/elixir.nix { inherit errors; };
    erlang = import ./languages/erlang.nix { inherit errors; };
    futhark = import ./languages/futhark.nix { };
    gleam = import ./languages/gleam.nix { };
    go = import ./languages/go.nix { inherit errors; };
    haskell = import ./languages/haskell.nix { inherit errors; };
    idris = import ./languages/idris.nix { };
    java = import ./languages/java { inherit errors lib; };
    javascript = import ./languages/javascript.nix { inherit errors; };
    kotlin = import ./languages/kotlin.nix { inherit errors; };
    ocaml = import ./languages/ocaml.nix { inherit errors; };
    python = import ./languages/python.nix { inherit errors; };
    rust = import ./languages/rust.nix { inherit errors rust-overlay; };
    scala = import ./languages/scala.nix { inherit errors; };
    zig = import ./languages/zig.nix { inherit errors; };
  };
  inherit
    (import ./rust-tooling.nix {
      inherit
        lib
        packagePath
        languages
        writePythonApplication
        rustWorkspaceFor
        clippy-fork
        ;
      repoRoot = paths.root;
    })
    buildIxRustTool
    cargoUnitFor
    buildRustPackage
    ;
  cargoUnit = cargoUnitFor pkgs;
  goUnitFor =
    pkgs:
    import ./go-unit.nix {
      inherit lib pkgs;
      inherit (languages) go;
    };
  goUnit = goUnitFor pkgs;

  systemdHardening = import ./systemd-hardening.nix;

  /**
    Helpers that throw with a fixable error message instead of a deep-eval
    crash. See [`lib/errors.nix`](lib/errors.nix) for the full surface:
    `assertEnum`, `requireArg`, `requireAttr`.
  */
  errors = import ./errors.nix { inherit lib; };

  /**
    Utilities for option values that are later joined under a runtime
    directory.

    `isSafe` accepts relative paths with ordinary segments and rejects empty,
    absolute, `.`, `..`, and repeated-slash forms. Use `isSafeName` for values
    that become one directory entry rather than a nested path. `shellPath` and
    `shellParent` return shell snippets for joining a root expression such as
    `$out` with a validated relative path.
  */
  relativePath = import ./relative-path.nix { inherit lib; };

  mkMinecraftLoader = import ./minecraft-loader.nix;

  /**
    Repo-owned Minecraft helpers exposed through `specialArgs.ix` and the
    flake's `lib` output.

    - `nbt`: typed NBT-tag constructors. Plain Nix scalars (attrset, list,
      string, bool, int, float) round-trip to compound, list, string, byte,
      int/long, and double tags. These constructors are the escape hatch for
      Minecraft's narrower tag types: bytes, shorts, floats, typed numeric
      arrays, and named roots.
    - `dimensionType`: vanilla dimension-type JSON snapshots plus a `withBase`
      merge helper. Lets `services.minecraft.datapacks.<n>.dimensionTypes.<dim>`
      set `base = "minecraft:overworld"` and override only the height knobs
      (or any other field) instead of restating the whole schema. See
      [`lib/minecraft/dimension-type.nix`](lib/minecraft/dimension-type.nix).
  */
  minecraft = {
    nbt = import ./minecraft/nbt.nix;
    dimensionType = import ./minecraft/dimension-type.nix { inherit lib; };
  };

  /**
    Build a `pkgs.formats`-style generator for Minecraft NBT data.

    Arguments:
    - `pkgs`: package set used to build the encoder and output derivation.
    - `format`: `snbt` for readable stringified NBT or `nbt` for binary NBT.
    - `flavor`: binary NBT compression flavor: `uncompressed`, `gzip`, or
      `zlib`. Ignored for `snbt`.

    Returns an attrset with `type` and `generate`, matching `pkgs.formats.*`.
  */
  mkMinecraftNbtFormat = import ./minecraft-nbt-format.nix {
    inherit lib buildIxRustTool packagePath;
  };

  /**
    Build the `minecraft-sync-managed` wrapper for a Minecraft service.

    The wrapper passes the mutable data directory, managed `/etc/minecraft`
    roots, datapack worlds, reload settings, and RCON settings to the Rust
    sync tool. The tool then syncs ordinary managed files and datapacks, and
    reconciles `whitelist.json` and `ops.json` against the live server files
    by UUID.
  */
  mkMinecraftSyncManaged =
    args:
    import ./minecraft-sync-managed.nix (
      {
        package = buildIxRustTool pkgs (packagePath "minecraft-sync-managed");
        inherit writeNushellApplication;
      }
      // args
    );

  /**
    Pinned artifact catalogs surfaced to images and presets by name.
    Presets must consume entries through this set (or one of the module
    options it seeds) rather than inlining URLs and hashes.
  */
  artifacts = import ./artifacts.nix { inherit lib pkgs paths; };

  /**
    Flake-output-only repo packages, callPackage-style.

    These are derivations that flake consumers can reach as
    `packages.<system>.<name>`, but that we don't want to inject into the
    nixpkgs namespace inside an image's evaluation. Each entry takes the
    standard `pkgs` it should build against and the cross-cutting
    `specialArgs.ix` bundle.
  */
  packageSetFor = import ./packages.nix {
    inherit
      lib
      packageRegistry
      ixSpecialArgs
      cargoUnitFor
      goUnitFor
      rustWorkspaceFor
      cliArtifacts
      clippy-fork
      ;
  };

  /**
    Shared Rust workspace source and unit graph for repo-owned crates.

    The root Cargo.toml and Cargo.lock are the source of truth for IDEs,
    dependency versions, and package builds. The filtered source keeps the Nix
    closure to Rust workspace inputs instead of the full repository.

    `rustWorkspaceFor pkgs` returns `{ root; src; cargoLock; units; }` for the
    caller's package set. The default `rustWorkspace` uses the repo's
    `x86_64-linux` package set for image and module evaluation.
  */
  rustWorkspaceFor = import ./rust-workspace.nix {
    inherit
      lib
      paths
      packageRegistry
      cargoUnitFor
      ;
  };
  rustWorkspace = rustWorkspaceFor pkgs;

  /**
    Cross-cutting helpers handed to every module through `specialArgs.ix`.
    Keep this surface small and stable: anything here is part of the
    cross-module contract.
  */
  ixSpecialArgs = {
    inherit
      artifacts
      agentsMd
      buildBunSite
      buildGradleFatJar
      buildRustPackage
      buildNpmSite
      buildNpmVitest
      buildSvelteSite
      buildUvApplication
      buildZigPackage
      cargoUnit
      goUnit
      languages
      minecraft
      mkMinecraftLoader
      mkMinecraftNbtFormat
      mkMinecraftSyncManaged
      relativePath
      rustWorkspace
      rustWorkspaceFor
      secrets
      systemdHardening
      writeNushellApplication
      writeProcessComposeApplication
      writePythonApplication
      ;
    packages = packageSetFor pkgs;
  };

  inherit
    (import ./images.nix {
      inherit
        lib
        nixpkgs
        paths
        system
        determinate
        home-manager
        overlays
        ixSpecialArgs
        moduleList
        writeNushellApplication
        secrets
        packageSetFor
        ;
    })
    evalImageConfig
    mkImage
    mkFleetFor
    mkFleet
    ;

  inherit
    (import ./discovery.nix {
      inherit
        lib
        paths
        artifacts
        mkImage
        mkFleetFor
        ixReturn
        ;
    })
    discoverTree
    discoverImages
    discoverModules
    exampleFleetsFor
    ;

  # Self-reference (let-bindings are mutually recursive): `exampleFleetsFor`
  # passes `ixReturn` back into examples as `index.lib`. Forced only when
  # an example actually reads from it.
  ixReturn = {
    inherit
      system
      pkgs
      overlay
      overlays
      evalImageConfig
      mkImage
      mkFleet
      mkFleetFor
      discoverTree
      discoverImages
      discoverModules
      nixosModules
      exampleFleetsFor
      artifacts
      agentsMd
      buildBunSite
      buildGradleFatJar
      buildNpmSite
      buildNpmVitest
      buildSvelteSite
      buildUvApplication
      buildZigPackage
      bunLockFor
      cargoUnit
      cargoUnitFor
      errors
      goUnit
      goUnitFor
      languages
      minecraft
      mkMinecraftLoader
      mkMinecraftNbtFormat
      mkMinecraftSyncManaged
      packageSetFor
      relativePath
      rustWorkspace
      rustWorkspaceFor
      secrets
      systemdHardening
      uvLockFor
      writeNushellApplication
      writeProcessComposeApplication
      writePythonApplication
      ;

    /**
      Nous Research's Hermes agent flake. Examples consume
      `index.lib.hermesAgent.nixosModules.default` to add the
      `services.hermes-agent.*` option surface to an image, plus
      `index.lib.hermesAgent.overlays.default` if they want the
      `hermes-agent` package available at module-eval time.
    */
    hermesAgent = hermes-agent;
  };
in
ixReturn
