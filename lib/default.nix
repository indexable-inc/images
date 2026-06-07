# ix/images public lib. Helpers documented per binding with RFC-0145
# doc-comments below; the file's job is to wire them together.
{
  nixpkgs,
  paths,
  rust-overlay,
  home-manager,
  hermes-agent,
  btop-src,
  drgn-src,
  fff-src,
  launchk-src,
  clippy-fork,
  ghostty,
  # Flake source revision, stamped into builds that want to report it (see
  # `sharedHelpers.rev`). Defaulted so a direct `import ./lib` still evaluates.
  rev ? "dev",
  # Commit time of `rev` as unix epoch seconds (Nix's `self.lastModified`), for
  # builds that want to show a human date and relative age alongside the
  # revision. The `build-version` crate renders it. `0` when unknown. Defaulted
  # so a direct `import ./lib` still evaluates.
  revEpoch ? 0,
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

  inherit (import ./util/writers.nix { inherit lib; })
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
    # Pure cross-cutting helpers (deepMerge, writers, ...) so overlay packages
    # that take an `ix` argument resolve it the same way flake-output packages
    # do. Defined below in this recursive `let`; threaded lazily.
    ix = sharedHelpers;
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

  # Portable user-service layer (launchd + systemd from one spec). Lives
  # outside `modules/` on purpose: it is a home-manager module, not a NixOS
  # module, so it must not be swept into `nixosModules` above. Exposed to
  # consumers as `homeModules.portable-services` from the flake.
  portableServices = import ./services/portable-services.nix { inherit lib; };

  # Declarative-but-writable JSON config files (last-applied 3-way merge for
  # files an app rewrites at runtime). Also a home-manager module, not a NixOS
  # one, so it stays outside `modules/`. Exposed as `homeModules.mutable-json`.
  mutableJson = import ./services/mutable-json.nix { inherit lib; };

  # Flat list of module paths from the auto-discovered registry under
  # `modules/`. Pulled in unconditionally so every option is in scope; each
  # module stays inert until its `enable` flag is set.
  moduleList = lib.collect builtins.isPath nixosModules;

  bunLockFor =
    pkgs:
    import ./build/bun-lock.nix {
      inherit lib pkgs;
    };
  buildJsSite = import ./build/js-site.nix {
    inherit bunLockFor errors;
  };
  buildSvelteSite = import ./build/svelte-site.nix {
    inherit bunLockFor errors writeNushellApplication;
  };
  buildNpmVitest = import ./build/npm-vitest.nix;
  buildZigPackage = import ./build/zig-package.nix { };
  buildLibghosttyVt = import ./build/libghostty-vt.nix { inherit lib writeNushellApplication; };
  uvLockFor =
    pkgs:
    import ./build/uv-lock.nix {
      inherit lib pkgs;
    };
  buildUvApplication = import ./build/uv-application.nix { inherit uvLockFor; };
  buildGradleFatJar = import ./build/gradle-fat-jar.nix { inherit lib; };
  secrets = import ./util/secrets.nix {
    inherit lib pkgs writeNushellApplication;
  };
  agentContext = import ./agent-context { inherit lib paths; };
  skills = import ./agent-context/skills.nix { inherit lib paths; };
  # Shared JetBrains Islands palette (both variants), the single source of truth
  # for syntax color across the repo: the code-highlight crate embeds this JSON
  # for the search `-c` output, and the base profile generates its
  # Neovim colorscheme from the same data through this value.
  islandsTheme = lib.importJSON (paths.packagesRoot + "/code-highlight/src/islands-theme.json");
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
    (import ./rust/tooling.nix {
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
    import ./build/go-unit.nix {
      inherit lib pkgs;
      inherit (languages) go;
    };
  goUnit = goUnitFor pkgs;

  systemdHardening = import ./services/systemd-hardening.nix;

  /**
    Helpers that throw with a fixable error message instead of a deep-eval
    crash. See [`lib/util/errors.nix`](lib/util/errors.nix) for the full surface:
    `assertEnum`, `requireArg`, `requireAttr`.
  */
  errors = import ./util/errors.nix { inherit lib; };

  /**
    Recursive attrset merge with two collision policies (`strict` throws,
    `rhs` wins) plus an N-ary `strictList`. Single sanctioned replacement
    for hand-rolled deep-merge and the patterns the `no-recursive-update`
    rule flags. See [`lib/util/deep-merge.nix`](lib/util/deep-merge.nix).
  */
  deepMerge = import ./util/deep-merge.nix { inherit lib; };

  /**
    Utilities for option values that are later joined under a runtime
    directory.

    `isSafe` accepts relative paths with ordinary segments and rejects empty,
    absolute, `.`, `..`, and repeated-slash forms. Use `isSafeName` for values
    that become one directory entry rather than a nested path. `shellPath` and
    `shellParent` return shell snippets for joining a root expression such as
    `$out` with a validated relative path.
  */
  relativePath = import ./util/relative-path.nix { inherit lib; };

  mkMinecraftLoader = import ./minecraft/loader.nix;

  /**
    Declare a continuous-benchmark suite against the `indexbench` CLI.

    `mkBenchSuite pkgs { name; indexbench; macros ? []; allocCheck ? null; runs ? 10; }`
    returns `{ app; check ? }`:

    - `app` is a `nix run`-able wrapper that runs the suite's macro commands
      through `indexbench run`, recording timing, peak RSS, and any `@bench`
      custom metrics, and exiting non-zero on a regression. Belongs in
      `apps.bench` / the perf job, never in `checks` (timing and RSS are not
      reproducible in the Nix sandbox).
    - `check`, present only when `allocCheck = { bench; budgets; }` is set, is a
      `nix flake check` derivation that runs the bench once through
      `indexbench assert` and fails if a metric exceeds its budget. Allocation
      counts are reproducible, so this path is a real, hermetic CI gate.

    See [`lib/util/bench.nix`](lib/util/bench.nix) for the argument shape.
  */
  mkBenchSuite = import ./util/bench.nix {
    inherit lib writeNushellApplication;
  };

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
  mkMinecraftNbtFormat = import ./minecraft/nbt-format.nix {
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
    import ./minecraft/sync-managed.nix (
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
  artifacts = import ./util/artifacts.nix { inherit lib pkgs paths; };

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
      clippy-fork
      ghostty
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
  rustWorkspaceFor = import ./rust/workspace.nix {
    inherit
      lib
      paths
      packageRegistry
      cargoUnitFor
      buildSvelteSite
      ghostty
      writeNushellApplication
      macosSdk
      appleSdkToolchain
      ;
    rustToolchainFor = languages.rust.toolchain;
  };
  rustWorkspace = rustWorkspaceFor pkgs;

  /**
    Pinned macOS SDK used to cross-compile Rust to Darwin from Linux. A
    function `{ pkgs }: derivation`; override it to supply your own SDK.
    See [`lib/darwin/macos-sdk.nix`](lib/darwin/macos-sdk.nix).
  */
  macosSdk = import ./darwin/macos-sdk.nix;

  /**
    zig + macOS SDK cross toolchain. `{ appleSdk, lib, pkgs, target }` returns
    `{ env, runtimeInputs, rustcArgsForPlatform }` consumed by
    `rustWorkspace.unitsFor`. See [`lib/darwin/apple-sdk-toolchain.nix`](lib/darwin/apple-sdk-toolchain.nix).
  */
  appleSdkToolchain = import ./darwin/apple-sdk-toolchain.nix;

  /**
    Helper surface shared by both the per-module `specialArgs.ix`
    (`ixSpecialArgs`) and the public `index.lib` (`ixReturn`). Listed once
    here so a new shared helper reaches both surfaces from a single edit;
    each consumer splices its own extras on top with `//`.
  */
  sharedHelpers = {
    inherit
      rev
      revEpoch
      agentContext
      artifacts
      buildGradleFatJar
      buildJsSite
      buildLibghosttyVt
      buildNpmVitest
      buildSvelteSite
      buildUvApplication
      buildZigPackage
      cargoUnit
      deepMerge
      goUnit
      languages
      minecraft
      mkBenchSuite
      mkMinecraftLoader
      mkMinecraftNbtFormat
      mkMinecraftSyncManaged
      mutableJson
      relativePath
      rustWorkspace
      rustWorkspaceFor
      secrets
      skills
      systemdHardening
      writeNushellApplication
      writeProcessComposeApplication
      writePythonApplication
      ;
    btopSrc = btop-src;
    drgnSrc = drgn-src;
    fffSrc = fff-src;
    launchkSrc = launchk-src;
  };

  /**
    Cross-cutting helpers handed to every module through `specialArgs.ix`.
    Keep this surface small and stable: anything here is part of the
    cross-module contract.
  */
  ixSpecialArgs = sharedHelpers // {
    inherit buildRustPackage islandsTheme;
    packages = packageSetFor pkgs;
  };

  inherit
    (import ./image {
      inherit
        lib
        nixpkgs
        paths
        system
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
    mkNonNixImage
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
  ixReturn = sharedHelpers // {
    inherit
      appleSdkToolchain
      bunLockFor
      cargoUnitFor
      discoverImages
      discoverModules
      discoverTree
      errors
      evalImageConfig
      exampleFleetsFor
      goUnitFor
      macosSdk
      mkFleet
      mkFleetFor
      mkImage
      mkNonNixImage
      nixosModules
      overlay
      overlays
      packageSetFor
      pkgs
      portableServices
      system
      uvLockFor
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
