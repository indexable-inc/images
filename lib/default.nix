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
  perftest-src,
  fff-src,
  nu-jupyter-kernel-src,
  launchk-src,
  snix-src,
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
  # The flake's own source (`self`), carrying `.outPath` (a `-source` store
  # path with string context, so it roots into a closure like `nixpkgs`) and
  # `.narHash`. Only the flake scope sees these, so they are plumbed down to
  # `lib/image` for the guest `index` registry pin. Defaulted `null` so a bare
  # `import ./lib` (no flake) still evaluates; `lib/image` guards on it.
  self ? null,
}: let
  inherit (nixpkgs) lib;

  system = "x86_64-linux";

  packageRegistry = import (paths.packagesRoot + "/registry.nix") {
    inherit lib;
    root = paths.packagesRoot;
    inherit (lists) findDuplicates;
  };
  packagePath = id: let
    entry = packageRegistry.byId.${id} or (throw "ix.lib: package registry has no `${id}` entry");
  in
    entry.path;

  # Shared ruff selector (ANN explicit-annotations + TID251 no-typing.cast),
  # imported once here and injected into every Python build gate so the policy
  # has a single source of truth. See lib/ruff-ann.nix.
  inherit (import ./ruff-ann.nix {inherit lib;}) ruffAnnArgs;

  inherit
    (import ./util/writers.nix {inherit lib ruffAnnArgs;})
    writePythonApplication
    writeNushellApplication
    writeBashApplication
    writeRustApplication
    writeProcessComposeApplication
    ;
  netCidr = import ./util/net-cidr.nix {inherit lib;};
  publicArtifactsFor = pkgs: import ./util/public-artifacts.nix {inherit lib pkgs;};
  secretRefs = import ./util/secret-refs.nix {inherit lib;};
  selfVersionFor = self: import ./util/self-version.nix {inherit lib self;};
  checks = import ./checks.nix {inherit lib;};

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
      cargoUnitFor
      clippy-fork
      rustWorkspaceFor
      writeNushellApplication
      writePythonApplication
      ;
    # Pure cross-cutting helpers (deepMerge, writers, ...) so overlay packages
    # that take an `ix` argument resolve it the same way flake-output packages
    # do. Defined below in this recursive `let`; threaded lazily.
    ix = sharedHelpers;
  };
  overlays = [overlay];

  /**
  nixpkgs instance with the repo overlay applied, evaluated for
  `x86_64-linux`. Use this when the image build needs `pkgs` directly.
  */
  pkgs = import nixpkgs {
    inherit system overlays;
    config = {};
  };

  # Auto-discovered NixOS module registry.
  nixosModules = discoverModules {root = paths.modules;};

  # Portable user-service layer (launchd + systemd from one spec). Lives
  # outside `modules/` on purpose: it is a home-manager module, not a NixOS
  # module, so it must not be swept into `nixosModules` above. Exposed to
  # consumers as `homeModules.portable-services` from the flake.
  portableServices = import ./services/portable-services.nix {inherit lib deepMerge;};

  # Declarative-but-writable JSON config files (last-applied 3-way merge for
  # files an app rewrites at runtime). Also a home-manager module, not a NixOS
  # one, so it stays outside `modules/`. Exposed as `homeModules.mutable-json`.
  mutableJson = import ./services/mutable-json.nix {inherit lib;};

  # Flat list of module paths from the auto-discovered registry under
  # `modules/`. Pulled in unconditionally so every option is in scope; each
  # module stays inert until its `enable` flag is set.
  moduleList = lib.collect builtins.isPath nixosModules;

  bunLockFor = pkgs:
    import ./build/bun-lock.nix {
      inherit lib pkgs;
    };
  buildJsSite = import ./build/js-site.nix {
    inherit bunLockFor errors;
  };
  buildSvelteSite = import ./build/svelte-site.nix {
    inherit
      bunLockFor
      errors
      paths
      writeNushellApplication
      ;
  };
  buildNpmVitest = import ./build/npm-vitest.nix;
  buildZigPackage = import ./build/zig-package.nix {};
  buildLibghosttyVt = import ./build/libghostty-vt.nix {inherit lib writeNushellApplication;};
  uvLockFor = pkgs:
    import ./build/uv-lock.nix {
      inherit lib pkgs;
    };
  buildUvApplication = import ./build/uv-application.nix {inherit uvLockFor ruffAnnArgs;};
  # Shared Elixir quality lane (compile -Werror + format + credo --strict + test),
  # injected with the single-source-of-truth strict Credo config so every Elixir
  # gate enforces the same policy. The Elixir counterpart of buildUvApplication.
  buildElixirCheck = import ./build/elixir-check.nix {credoConfig = ./elixir/credo.exs;};
  buildPyStrictCheck = import ./build/py-strict-check.nix {inherit lib;};
  buildGradleFatJar = import ./build/gradle-fat-jar.nix {inherit lib;};
  wrapPackage = import ./build/wrap-package.nix {inherit lib;};
  # Markdown document rendering with JSON-encoded YAML frontmatter. Used by
  # typed wrappers that generate small `.md` files with parseable metadata.
  markdown = import ./util/markdown.nix {inherit lib;};
  skills = import ./skills.nix {inherit lib paths;};
  agents = import ./agents.nix {inherit lib markdown;};
  hermes = import ./hermes {};
  claudePlugin = import ./claude-plugin.nix {inherit lib skills;};
  # Shared JetBrains Islands palette (both variants), the single source of truth
  # for syntax color across the repo: the code-highlight crate embeds this JSON
  # for the search `-c` output, and the base profile generates its
  # Neovim colorscheme from the same data through this value.
  islandsTheme = lib.importJSON (paths.packagesRoot + "/code/code-highlight/src/islands-theme.json");
  # Repo-default JVM major: imported once here (single source of truth) and
  # threaded into `languages.java`, which re-exports it as
  # `ix.languages.java.defaultJvmVersion` for modules/examples that pin the JDK.
  defaultJvmVersion = import ./languages/jvm-defaults.nix;
  languages = {
    cpp = import ./languages/cpp.nix {inherit errors;};
    dhall = import ./languages/dhall.nix {};
    elixir = import ./languages/elixir.nix {inherit errors;};
    erlang = import ./languages/erlang.nix {inherit errors;};
    futhark = import ./languages/futhark.nix {};
    gleam = import ./languages/gleam.nix {};
    go = import ./languages/go.nix {inherit errors;};
    haskell = import ./languages/haskell.nix {inherit errors;};
    idris = import ./languages/idris.nix {};
    java = import ./languages/java {inherit errors lib defaultJvmVersion;};
    javascript = import ./languages/javascript.nix {inherit errors;};
    kotlin = import ./languages/kotlin.nix {inherit errors;};
    ocaml = import ./languages/ocaml.nix {inherit errors;};
    python = import ./languages/python.nix {inherit errors;};
    rust = import ./languages/rust.nix {inherit errors rust-overlay;};
    scala = import ./languages/scala.nix {inherit errors;};
    zig = import ./languages/zig.nix {inherit errors;};
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
        lists
        pins
        ;
      repoRoot = paths.root;
    })
    buildIxRustTool
    cargoUnitFor
    buildRustPackage
    ;
  cargoUnit = cargoUnitFor pkgs;
  goUnitFor = pkgs:
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
  errors = import ./util/errors.nix {inherit lib;};

  /**
  Recursive attrset merge with two collision policies (`strict` throws,
  `rhs` wins) plus an N-ary `strictList`. Single sanctioned replacement
  for hand-rolled deep-merge and the patterns the `no-recursive-update`
  rule flags. See [`lib/util/deep-merge.nix`](lib/util/deep-merge.nix).
  */
  deepMerge = import ./util/deep-merge.nix {inherit lib;};

  /**
  Utilities for option values that are later joined under a runtime
  directory.

  `isSafe` accepts relative paths with ordinary segments and rejects empty,
  absolute, `.`, `..`, and repeated-slash forms. Use `isSafeName` for values
  that become one directory entry rather than a nested path. `shellPath` and
  `shellParent` return shell snippets for joining a root expression such as
  `$out` with a validated relative path.
  */
  relativePath = import ./util/relative-path.nix {inherit lib;};

  /**
  List helpers not covered by `nixpkgs.lib`: `findDuplicates` (repeated
  elements) and `findDuplicatesBy` (elements colliding under a key function).
  See [`lib/util/lists.nix`](lib/util/lists.nix).
  */
  lists = import ./util/lists.nix {inherit lib;};

  /**
  General attrset helpers beyond `nixpkgs.lib`: `flattenToDotted` collapses a
  nested attrset to a flat one keyed by dotted paths (a config tree ->
  `key.path=value` flags or dotted env names). See
  [`lib/util/attrs.nix`](lib/util/attrs.nix).
  */
  attrs = import ./util/attrs.nix {inherit lib;};

  /**
  TOML value encoding. `scalar` renders one Nix scalar as the TOML literal a
  `key = value` pair expects (codex `--config a.b=1` flags). Scalars only;
  for whole TOML files use `pkgs.formats.toml`. See
  [`lib/util/toml.nix`](lib/util/toml.nix).
  */
  toml = import ./util/toml.nix {inherit lib;};

  /**
  Read a package's pinned hashes/digests from a sibling `pins.json` instead
  of inlining `hash = "sha256-..."` in the `.nix`. `loadPins ./pins.json`
  returns the validated `{ name = { hash; ... }; }` map; `loadPin ./pins.json
  "src"` returns one named entry. The JSON is the single source of truth an
  updater rewrites, so a bump touches one data file. See
  [`lib/util/pins.nix`](lib/util/pins.nix).
  */
  pins = import ./util/pins.nix {inherit lib;};

  /**
  Single source of truth for the MCP servers baked into the agent wrappers.
  Define a server once in a neutral shape and render it to each tool's native
  config with `mcp.toClaudeJson` (Claude Code's `mcpServers` JSON) and
  `mcp.toCodexEntries` (dotted `mcp_servers.*` codex `-c` flags), so `index`
  is declared in one place rather than copied into both wrappers. See
  [`lib/util/mcp.nix`](lib/util/mcp.nix).
  */
  mcp = import ./util/mcp.nix {inherit lib;};

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
    dimensionType = import ./minecraft/dimension-type.nix {inherit lib deepMerge;};
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
  mkMinecraftSyncManaged = args:
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
  artifacts = import ./util/artifacts.nix {inherit lib pkgs paths;};

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
      buildLibghosttyVt
      ghostty
      writeBashApplication
      macosSdk
      appleSdkToolchain
      pins
      ;
    rustToolchainFor = languages.rust.toolchain;
  };
  rustWorkspace = rustWorkspaceFor pkgs;

  /**
  Pinned macOS SDK used to cross-compile Rust to Darwin from Linux. A
  function `{ pkgs }: derivation`; override it to supply your own SDK.
  See [`lib/darwin/macos-sdk.nix`](lib/darwin/macos-sdk.nix).
  */
  macosSdk = import ./darwin/macos-sdk.nix {inherit pins;};

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
    inherit (import ./util/endpoint.nix {inherit lib;}) endpoint endpointOf;
    inherit
      rev
      revEpoch
      agents
      artifacts
      attrs
      buildElixirCheck
      buildGradleFatJar
      buildJsSite
      buildLibghosttyVt
      buildNpmVitest
      buildPyStrictCheck
      buildSvelteSite
      buildUvApplication
      buildZigPackage
      cargoUnit
      checks
      claudePlugin
      deepMerge
      goUnit
      hermes
      languages
      lists
      mcp
      minecraft
      mkBenchSuite
      mkMinecraftLoader
      mkMinecraftNbtFormat
      wrapPackage
      mkMinecraftSyncManaged
      mutableJson
      netCidr
      paths
      pins
      publicArtifactsFor
      relativePath
      ruffAnnArgs
      rustWorkspace
      rustWorkspaceFor
      secretRefs
      selfVersionFor
      skills
      systemdHardening
      toml
      writeBashApplication
      writeNushellApplication
      writeProcessComposeApplication
      writePythonApplication
      writeRustApplication
      ;
    btopSrc = btop-src;
    drgnSrc = drgn-src;
    perftestSrc = perftest-src;
    fffSrc = fff-src;
    nuJupyterKernelSrc = nu-jupyter-kernel-src;
    launchkSrc = launchk-src;
    snixSrc = snix-src;
  };

  /**
  Cross-cutting helpers handed to every module through `specialArgs.ix`.
  Keep this surface small and stable: anything here is part of the
  cross-module contract.
  */
  ixSpecialArgs =
    sharedHelpers
    // {
      inherit buildRustPackage islandsTheme;
      packages = packageSetFor pkgs;
    };

  inherit
    (import ./image {
      inherit
        self
        lib
        nixpkgs
        paths
        system
        home-manager
        overlays
        ixSpecialArgs
        moduleList
        writeNushellApplication
        packageSetFor
        ;
    })
    evalImageConfig
    mkImage
    mkNonNixImage
    mkFleetFor
    mkFleet
    mkDevFor
    mkDev
    ;

  inherit
    (import ./discovery.nix {
      inherit
        lib
        paths
        mkFleetFor
        mkDevFor
        ixReturn
        ;
    })
    discoverTree
    discoverModules
    exampleFleetsFor
    ;

  # Self-reference (let-bindings are mutually recursive): `exampleFleetsFor`
  # passes `ixReturn` back into examples as `index.lib`. Forced only when
  # an example actually reads from it.
  ixReturn =
    sharedHelpers
    // {
      inherit
        appleSdkToolchain
        bunLockFor
        cargoUnitFor
        discoverModules
        discoverTree
        errors
        evalImageConfig
        exampleFleetsFor
        goUnitFor
        macosSdk
        mkDev
        mkDevFor
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
