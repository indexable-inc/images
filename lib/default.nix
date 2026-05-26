# ix/images public lib. Helpers documented per binding with RFC-0145
# doc-comments below; the file's job is to wire them together.
{
  nixpkgs,
  paths,
  rust-overlay,
  determinate,
  home-manager,
  hermes-agent,
  cliArtifacts ? { },
}:
let
  inherit (nixpkgs) lib;

  system = "x86_64-linux";

  /**
    Package a Python entrypoint as a standalone executable.

    Wraps `src` in a launcher script that prepends `runtimeInputs` to PATH
    and runs the file under `python`. When `check` is true (default), the
    derivation also runs `basedpyright` over `src` in `standard` mode during
    the build, so type regressions fail the build instead of surfacing at
    runtime.

    Arguments:
    - `name`: derivation name and `/bin/<name>` executable.
    - `src`: a path or store path containing the Python entrypoint.
    - `args`: literal argv prefix prepended to user args at runtime.
    - `runtimeInputs`: extra packages prepended to PATH at runtime.
    - `python`: Python interpreter package. Defaults to `pkgs.python314`.
    - `check`, `typeCheckingMode`, `pythonPlatform`: basedpyright knobs.
    - `extraPaths`: extra import roots for basedpyright.
    - `meta`: standard derivation meta, with `mainProgram` defaulted.
  */
  writePythonApplication =
    pkgs:
    {
      name,
      src,
      args ? [ ],
      runtimeInputs ? [ ],
      python ? pkgs.python314,
      check ? true,
      typeCheckingMode ? "standard",
      pythonPlatform ? "Linux",
      extraPaths ? [ "${python}/${python.sitePackages}" ],
      meta ? { },
    }:
    let
      runtimePath = lib.makeBinPath ([ python ] ++ runtimeInputs);
      srcPath = src;
      argv = builtins.toJSON ([ "${srcPath}" ] ++ args);
      checkedTypeCheckingMode = errors.assertEnum {
        name = "writePythonApplication.typeCheckingMode";
        value = typeCheckingMode;
        valid = basedpyrightTypeCheckingModes;
      };
      # `"${src}"` (not `builtins.toString src`) so the generated JSON
      # carries Nix string context for the source derivation; otherwise
      # the file references a store path with no recorded dependency
      # and Nix prints a "without a proper context" eval warning on
      # every consumer evaluation.
      pyrightConfig = pkgs.writeText "basedpyright-${name}.json" (
        builtins.toJSON {
          include = [ "${src}" ];
          inherit extraPaths pythonPlatform;
          typeCheckingMode = checkedTypeCheckingMode;
          inherit (python) pythonVersion;
        }
      );
    in
    pkgs.writeTextFile {
      inherit name;
      executable = true;
      destination = "/bin/${name}";
      text = ''
        #!${lib.getExe python}
        import os
        import runpy
        import sys

        runtime_path = ${builtins.toJSON runtimePath}
        ambient_path = os.environ.get("PATH", "")
        os.environ["PATH"] = runtime_path + ((":" + ambient_path) if ambient_path else "")
        sys.argv = ${argv} + sys.argv[1:]
        runpy.run_path("${srcPath}", run_name="__main__")
      '';
      checkPhase = lib.optionalString check ''
        ${lib.getExe pkgs.basedpyright} --project ${pyrightConfig} --level warning --warnings ${src}
      '';
      meta = meta // {
        mainProgram = meta.mainProgram or name;
      };
    };

  /**
    Package a Nushell command as a standalone executable.

    Generates a Nu script that prepends `runtimeInputs` to PATH while
    preserving the ambient PATH, then runs `text` as the body. With
    `check` left on (default), nushell's `--ide-check` parses the
    generated script during the build so syntax errors fail the build
    rather than reaching the user.

    Arguments:
    - `name`: derivation name and `/bin/<name>` executable.
    - `runtimeInputs`: packages prepended to PATH for the script body.
    - `text`: the Nu script body. A leading `#!/usr/bin/env nu` line is
      stripped before splicing.
    - `check`: run `nu --ide-check` at build time.
    - `meta`: standard derivation meta, with `mainProgram` defaulted.
  */
  writeNushellApplication =
    pkgs:
    {
      name,
      runtimeInputs ? [ ],
      text,
      check ? true,
      meta ? { },
    }:
    let
      scriptBody = lib.removePrefix "#!/usr/bin/env nu\n" text;
      runtimePath = lib.makeBinPath ([ pkgs.nushell ] ++ runtimeInputs);
    in
    pkgs.writeTextFile {
      inherit name;
      executable = true;
      destination = "/bin/${name}";
      text = ''
        #!${lib.getExe pkgs.nushell}
        let runtime_path = "${runtimePath}" | split row ":"
        let ambient_path = $env.PATH? | default []
        $env.PATH = $runtime_path ++ (if ($ambient_path | describe) == "string" { $ambient_path | split row ":" } else { $ambient_path })

      ''
      + scriptBody;
      checkPhase = lib.optionalString check ''
        ${lib.getExe pkgs.nushell} --no-config-file --no-std-lib --ide-check 100 "$target"
      '';
      meta = meta // {
        mainProgram = meta.mainProgram or name;
      };
    };

  /**
    Repo-local nixpkgs overlay.

    Exposes the few repo-owned packages that NixOS modules expect to find
    as `pkgs.<name>`. Flake-output-only packages live in `packageSetFor`
    instead so they don't leak into the nixpkgs namespace inside images.
  */
  overlay = final: _prev: {
    drgn = final.callPackage paths.packages.drgn { };

    minecraft-hot-reload-agent = final.callPackage paths.packages.minecraft.hotReloadAgent { };
    minecraft-rcon = final.callPackage paths.packages.minecraft.rcon {
      writePythonApplication = writePythonApplication final;
    };
    oci-image-builder = buildIxRustTool final paths.packages.ociImageBuilder;
  };
  overlays = [ overlay ];

  /**
    nixpkgs instance with the repo overlay applied, evaluated for
    `x86_64-linux`. Use this when the image build needs `pkgs` directly.
  */
  pkgs = import nixpkgs { inherit system overlays; };

  # Flat list of module paths from the canonical nested registry in
  # `modules/default.nix`. Pulled in unconditionally so every option is in
  # scope; each module stays inert until its `enable` flag is set.
  moduleList = lib.collect builtins.isPath (import paths.modules);

  bunLockFor =
    pkgs:
    import ./bun-lock.nix {
      inherit lib pkgs;
    };
  buildBunSite = import ./build-bun-site.nix {
    inherit bunLockFor;
  };
  buildNpmSite = import ./build-npm-site.nix;
  buildNpmVitest = import ./build-npm-vitest.nix;
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
    elixir = import ./languages/elixir.nix { inherit errors; };
    erlang = import ./languages/erlang.nix { inherit errors; };
    gleam = import ./languages/gleam.nix { };
    go = import ./languages/go.nix { inherit errors; };
    haskell = import ./languages/haskell.nix { inherit errors; };
    java = import ./languages/java { inherit errors lib; };
    javascript = import ./languages/javascript.nix { inherit errors; };
    kotlin = import ./languages/kotlin.nix { inherit errors; };
    ocaml = import ./languages/ocaml.nix { inherit errors; };
    python = import ./languages/python.nix { inherit errors; };
    rust = import ./languages/rust.nix { inherit errors rust-overlay; };
    scala = import ./languages/scala.nix { inherit errors; };
    zig = import ./languages/zig.nix { inherit errors; };
  };
  rustNightlyToolchainFor =
    pkgs:
    languages.rust.toolchain pkgs {
      channel = "nightly";
      version = languages.rust.defaultNightlyDate;
    };
  rustNightlyClippyToolchainFor =
    pkgs:
    languages.rust.toolchain pkgs {
      channel = "nightly";
      version = languages.rust.defaultNightlyDate;
      components = [
        "cargo"
        "llvm-tools"
        "rust-src"
        "rust-std"
        "rustc"
        "rustc-dev"
        "rustfmt"
      ];
    };
  llmClippyFor =
    pkgs:
    pkgs.callPackage paths.packages.llmClippy {
      rustToolchain = rustNightlyClippyToolchainFor pkgs;
    };
  rustFor =
    pkgs:
    import ./rust.nix {
      inherit lib pkgs;
      clippyPackage = llmClippyFor pkgs;
      rustToolchain = rustNightlyToolchainFor pkgs;
      writePythonApplication = writePythonApplication pkgs;
    };
  # Build a repo-owned Rust tool whose default.nix calls `ix.buildRustPackage`.
  # Returns the policy-unchecked variant when present, so generators that
  # only need the binary do not drag the policy-check graph into their closure.
  buildIxRustTool =
    hostPkgs: path:
    let
      checked = hostPkgs.callPackage path {
        pkgs = hostPkgs;
        ix = {
          buildRustPackage = pkgs: (rustFor pkgs).buildPackage;
          inherit rustWorkspace;
        };
      };
    in
    checked.passthru.unchecked or checked;
  cargoUnitFor =
    pkgs:
    import ./cargo-unit.nix {
      inherit lib pkgs;
      rust = rustFor pkgs;
      nixCargoUnit = buildIxRustTool pkgs paths.packages.nixCargoUnit;
    };
  cargoUnit = cargoUnitFor pkgs;
  goUnitFor =
    pkgs:
    import ./go-unit.nix {
      inherit lib pkgs;
      inherit (languages) go;
    };
  goUnit = goUnitFor pkgs;

  /**
    Build a repo-owned Rust package with the shared Rust policy.

    Wraps `rustPlatform.buildRustPackage`, enables parallel test execution by
    default, and attaches the repo's `llm-clippy` and unused-dependency checks
    as `passthru.tests` plus policy dependencies of the returned package.
  */
  buildRustPackage = pkgs: (rustFor pkgs).buildPackage;

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
  relativePath =
    let
      reservedSegments = [
        ""
        "."
        ".."
      ];
      segments = path: lib.splitString "/" path;
      hasReservedSegment =
        path: lib.any (segment: builtins.elem segment reservedSegments) (segments path);
      isSafe =
        path:
        builtins.isString path && path != "" && !(lib.hasPrefix "/" path) && !(hasReservedSegment path);
      isSafeName = path: isSafe path && builtins.length (segments path) == 1;
      renderPath = path: if builtins.isString path then path else "<${builtins.typeOf path}>";
      assertSafe =
        path:
        assert lib.assertMsg (isSafe path)
          "ix.relativePath.shellPath expected a safe relative path, got ${renderPath path}";
        path;
      shellPath = root: path: ''"${root}"/${lib.escapeShellArg (assertSafe path)}'';
      shellParent =
        root: path:
        let
          parent = dirOf (assertSafe path);
        in
        if parent == "." then ''"${root}"'' else shellPath root parent;
    in
    {
      inherit
        isSafe
        isSafeName
        shellParent
        shellPath
        ;
      unsafe = paths: lib.filter (path: !(isSafe path)) paths;
      unsafeNames = paths: lib.filter (path: !(isSafeName path)) paths;
    };

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
    nbt =
      let
        tagged = tag: value: {
          __minecraftNbt = tag;
          inherit value;
        };
      in
      {
        root = name: value: {
          __minecraftNbt = "root";
          inherit name value;
        };
        byte = tagged "byte";
        short = tagged "short";
        int = tagged "int";
        long = tagged "long";
        float = tagged "float";
        double = tagged "double";
        string = tagged "string";
        bool = value: tagged "byte" (if value then 1 else 0);
        byteArray = tagged "byteArray";
        intArray = tagged "intArray";
        longArray = tagged "longArray";
        list = tagged "list";
        compound = tagged "compound";
      };

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
  mkMinecraftNbtFormat =
    pkgs:
    {
      format,
      flavor ? "uncompressed",
    }:
    let
      validFormats = [
        "nbt"
        "snbt"
      ];
      validFlavors = [
        "uncompressed"
        "gzip"
        "zlib"
      ];
      jsonFormat = pkgs.formats.json { };
      minecraftNbt = buildIxRustTool pkgs paths.packages.minecraft.nbt;
    in
    assert lib.assertMsg (builtins.elem format validFormats)
      "mkMinecraftNbtFormat: format must be one of ${lib.concatStringsSep ", " validFormats}";
    assert lib.assertMsg (builtins.elem flavor validFlavors)
      "mkMinecraftNbtFormat: flavor must be one of ${lib.concatStringsSep ", " validFlavors}";
    {
      inherit (jsonFormat) type;
      generate =
        name: value:
        let
          input = pkgs.writeText "${name}.json" (builtins.toJSON value);
        in
        pkgs.runCommand name { nativeBuildInputs = [ minecraftNbt ]; } ''
          minecraft-nbt \
            --format ${lib.escapeShellArg format} \
            --flavor ${lib.escapeShellArg flavor} \
            --input ${input} \
            --output "$out"
        '';
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
        package = buildIxRustTool pkgs paths.packages.minecraft.syncManaged;
        inherit writeNushellApplication;
      }
      // args
    );

  /**
    Fetch a static artifact (mod jar, plugin, server) by URL + SRI hash.

    Hashes live next to URLs in the consuming catalog rather than in flake
    inputs, so a routine mod bump touches one JSON file and not
    `flake.lock`. Accepts and ignores extra catalog keys.
  */
  mkArtifact = { url, hash, ... }: pkgs.fetchurl { inherit url hash; };

  /**
    Enrich every entry of a `{ slug = { url, hash, ... }; ... }` catalog
    with a `src` attribute pointing at the fetched store path.
  */
  attachArtifactSources = lib.mapAttrs (_: entry: entry // { src = mkArtifact entry; });

  paperServers = {
    "26.1.2" = {
      build = 64;
      src = mkArtifact {
        url = "https://fill-data.papermc.io/v1/objects/830d4eb5c15cbd802a9ec9f2f54eaaaeb9511958339aec983fd0c88bad21d940/paper-26.1.2-64.jar";
        hash = "sha256-gw1OtcFcvYAqnsny9U6qrrlRGVgzmuyYP9DIi60h2UA=";
      };
    };

    "1.21.11" = {
      build = 69;
      src = mkArtifact {
        url = "https://api.papermc.io/v2/projects/paper/versions/1.21.11/builds/69/downloads/paper-1.21.11-69.jar";
        hash = "sha256-zzdPKvnXHfzHU0Pze3IqerywkcV0ExuV47E8b8LLj64=";
      };
    };
  };

  velocityServers = {
    "3.4.0-SNAPSHOT" = {
      build = 559;
      src = mkArtifact {
        url = "https://api.papermc.io/v2/projects/velocity/versions/3.4.0-SNAPSHOT/builds/559/downloads/velocity-3.4.0-SNAPSHOT-559.jar";
        hash = "sha256-zMSfcXUeziZWjTR2OS1hMMi0Py5fOogxMyW5J4pSur0=";
      };
    };
  };

  /**
    Per-version Minecraft artifact catalogs generated by `tools/update-mods.py`
    from a manifest directory such as `<paths.minecraftMods>` or
    `<paths.minecraftPaperPlugins>`.

    The bare-JSON catalog (slug -> `{ url, hash }`) is enriched into
    `{ url, hash, src }` so callers can pass it straight to
    `services.minecraft.modCatalog` or `services.minecraft.pluginCatalog`.
    Presets and examples consume these catalogs by name; to add an artifact,
    edit the relevant manifest and run `nix run .#update-mods`.
  */
  generatedCatalogs =
    root:
    let
      gameVersions = lib.pipe root [
        builtins.readDir
        (lib.filterAttrs (
          name: type: type == "regular" && lib.hasSuffix ".json" name && name != "manifest.json"
        ))
        builtins.attrNames
        (map (lib.removeSuffix ".json"))
      ];
      catalogFor =
        ver: attachArtifactSources (builtins.fromJSON (builtins.readFile (root + "/${ver}.json")));
    in
    lib.genAttrs gameVersions catalogFor;

  modCatalogs = generatedCatalogs paths.minecraftMods;
  paperPluginCatalogs = generatedCatalogs paths.minecraftPaperPlugins;
  velocityPluginCatalogs = generatedCatalogs paths.minecraftVelocityPlugins;

  # Minecraft version of the default variant declared in
  # `images/games/minecraft/versions.nix`. Lets per-loader fallback catalogs
  # follow the image default instead of pinning a literal version that silently
  # rots once the default moves.
  defaultMinecraftVersion =
    let
      versionsModule = import (paths.images + "/games/minecraft/versions.nix") {
        inherit lib;
      };
    in
    versionsModule.${versionsModule.default}.services.minecraft.version;

  # Fabric meta serves every server jar from the same loader+installer pair;
  # only the Minecraft version moves. Keep the pair in one place so a Fabric
  # bump touches one field instead of every URL string.
  fabricLoaderVersion = "0.19.2";
  fabricInstallerVersion = "1.1.1";
  fabricServerUrl =
    mcVer:
    "https://meta.fabricmc.net/v2/versions/loader/${mcVer}/${fabricLoaderVersion}/${fabricInstallerVersion}/server/jar";
  fabricServerHashes = {
    "26.2-snapshot-5" = "sha256-IZctWQu9VH4Z5lU/VcEzvPGLfW8boOAXtCaQlKXyA5k=";
    "26.2-snapshot-6" = "sha256-J4zGg7YlrHmYBsagTr+x2ZcAgOvj5vr/8iVgwMVG/e0=";
    "26.1.2" = "sha256-6RvRm5/w4ExXhD5iTS9U0KPjmgSMr8pejiDrmENEXb0=";
    "1.21.11" = "sha256-xDK1HU7Xwbr0Z7pw7Dtdtob0zvlfq9pZ9J4O32u4jBc=";
  };
  fabricServers = lib.mapAttrs' (mcVer: hash: {
    name = "${mcVer}-fabric";
    value = mkArtifact {
      url = fabricServerUrl mcVer;
      inherit hash;
    };
  }) fabricServerHashes;

  /**
    Pinned artifact catalogs surfaced to images and presets by name.
    Presets must consume entries through this set (or one of the module
    options it seeds) rather than inlining URLs and hashes.
  */
  artifacts = {
    inherit attachArtifactSources;
    minecraft = {
      inherit
        modCatalogs
        paperPluginCatalogs
        paperServers
        velocityPluginCatalogs
        velocityServers
        ;
      paperPluginCatalog =
        if builtins.hasAttr defaultMinecraftVersion paperPluginCatalogs then
          paperPluginCatalogs.${defaultMinecraftVersion}
        else
          throw "ix.lib.artifacts.minecraft.paperPluginCatalog: no Paper plugin catalog generated for Minecraft ${defaultMinecraftVersion} (the default in images/games/minecraft/versions.nix). Run `nix run .#update-mods -- --manifest images/games/minecraft/plugins/paper/manifest.json --version ${defaultMinecraftVersion}` and commit the result.";
      # Velocity plugins are cross-Minecraft-version: `velocityPluginCatalog`
      # is the unversioned default surfaced to modules. Per-version overrides
      # can still come from `velocityPluginCatalogs.<version>` if added.
      velocityPluginCatalog = velocityPluginCatalogs.common or { };
      servers = fabricServers // {
        "1.21.11-paper" = paperServers."1.21.11".src;
        "26.1.2-paper" = paperServers."26.1.2".src;
      };
    };
  };

  /**
    Flake-output-only repo packages, callPackage-style.

    These are derivations that flake consumers can reach as
    `packages.<system>.<name>`, but that we don't want to inject into the
    nixpkgs namespace inside an image's evaluation. Each entry takes the
    standard `pkgs` it should build against and the cross-cutting
    `specialArgs.ix` bundle.
  */
  packageSetFor =
    pkgs:
    let
      packageSystem = pkgs.stdenv.hostPlatform.system;
      ixForPackages = ixSpecialArgs // {
        inherit pkgs;
      };
      basePackages = {
        dag-runner = pkgs.callPackage paths.packages.dagRunner {
          inherit pkgs;
          ix = ixForPackages;
        };
        room =
          let
            roomSiteSrc = lib.fileset.toSource {
              root = paths.packages.room + "/site";
              fileset = lib.fileset.intersection (lib.fileset.gitTracked (paths.packages.room + "/site")) (
                lib.fileset.unions [
                  (paths.packages.room + "/site/package.json")
                  (paths.packages.room + "/site/package-lock.json")
                  (paths.packages.room + "/site/index.html")
                  (paths.packages.room + "/site/svelte.config.js")
                  (paths.packages.room + "/site/tsconfig.json")
                  (paths.packages.room + "/site/vite.config.ts")
                  (paths.packages.room + "/site/src")
                ]
              );
            };
            site = buildNpmSite pkgs {
              pname = "room-site";
              version = "0.1.0";
              src = roomSiteSrc;
            };
          in
          pkgs.callPackage paths.packages.room {
            inherit pkgs site;
            ix = ixForPackages;
          };
        drgn = pkgs.callPackage paths.packages.drgn { };
        ix-fleet = pkgs.callPackage paths.packages.ixFleet {
          ix = ixForPackages;
        };
        ix-dev-diagnose = pkgs.callPackage paths.packages.ixDevDiagnose {
          inherit pkgs;
          ix = ixForPackages;
        };
        minestom.helloServerJar = pkgs.callPackage paths.packages.minestom.servers.hello {
          ix = ixForPackages;
        };
        minecraft-nbt = pkgs.callPackage paths.packages.minecraft.nbt {
          inherit pkgs;
          ix = ixForPackages;
        };
        llm-clippy = llmClippyFor pkgs;
        loop =
          let
            loopSiteSrc = lib.fileset.toSource {
              root = paths.packages.loop + "/site";
              fileset = lib.fileset.intersection (lib.fileset.gitTracked (paths.packages.loop + "/site")) (
                lib.fileset.unions [
                  (paths.packages.loop + "/site/package.json")
                  (paths.packages.loop + "/site/package-lock.json")
                  (paths.packages.loop + "/site/index.html")
                  (paths.packages.loop + "/site/svelte.config.js")
                  (paths.packages.loop + "/site/tsconfig.json")
                  (paths.packages.loop + "/site/vite.config.ts")
                  (paths.packages.loop + "/site/src")
                ]
              );
            };
            viewer = buildNpmSite pkgs {
              pname = "loop-viewer";
              version = "0.1.0";
              src = loopSiteSrc;
            };
          in
          pkgs.callPackage paths.packages.loop {
            inherit pkgs viewer;
            ix = ixForPackages;
          };
        mc-probe = pkgs.callPackage paths.packages.minecraft.probe {
          ix = ixForPackages;
        };
        minecraft-sync-managed = pkgs.callPackage paths.packages.minecraft.syncManaged {
          inherit pkgs;
          ix = ixForPackages;
        };
        nix-cargo-unit = pkgs.callPackage paths.packages.nixCargoUnit {
          inherit pkgs;
          ix = ixForPackages;
        };
        oci-image-builder = pkgs.callPackage paths.packages.ociImageBuilder {
          inherit pkgs;
          ix = ixForPackages;
        };
        run = pkgs.callPackage paths.packages.run {
          ix = ixForPackages;
        };
        mcp = pkgs.callPackage paths.packages.mcp {
          ix = ixForPackages;
        };
        tonbo-artifacts = pkgs.callPackage paths.packages.tonboArtifacts { };
      };
      cliPackages = lib.optionalAttrs (builtins.hasAttr packageSystem cliArtifacts) {
        ix = pkgs.callPackage paths.packages.ix {
          src = cliArtifacts.${packageSystem};
        };
      };
    in
    basePackages // cliPackages;

  /**
    Shared Rust workspace source for repo-owned crates.

    The root Cargo.toml and Cargo.lock are the source of truth for IDEs,
    dependency versions, and package builds. The filtered source keeps the Nix
    closure to Rust workspace inputs instead of the full repository.
  */
  rustWorkspace =
    let
      inherit (paths) root;
    in
    {
      inherit root;
      cargoLock = root + "/Cargo.lock";
      src = lib.fileset.toSource {
        inherit root;
        fileset = lib.fileset.intersection (lib.fileset.gitTracked root) (
          lib.fileset.unions [
            (root + "/Cargo.toml")
            (root + "/Cargo.lock")
            (paths.modules + "/services/resource-monitor/stats-writer")
            paths.packages.room
            paths.packages.dagRunner
            paths.packages.ixDevDiagnose
            paths.packages.loop
            paths.packages.mcp
            paths.packages.minecraft.nbt
            paths.packages.minecraft.syncManaged
            paths.packages.nixCargoUnit
            paths.packages.ociImageBuilder
          ]
        );
      };
    };

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
      buildUvApplication
      cargoUnit
      goUnit
      languages
      minecraft
      mkMinecraftLoader
      mkMinecraftNbtFormat
      mkMinecraftSyncManaged
      relativePath
      rustWorkspace
      secrets
      systemdHardening
      writeNushellApplication
      writePythonApplication
      ;
    packages = packageSetFor pkgs;
  };

  /**
    Run the platform config, OCI packaging, base profile, the full module
    registry, and the caller's `modules` through `lib.nixosSystem`, then
    return the evaluated `config`. This is the evaluation path every
    image build and every eval test goes through, so a test exercising it
    catches the same regressions a real build would.

    Arguments:
    - `modules`: list of additional modules layered on top of the base.
  */
  evalImageConfig =
    {
      modules ? [ ],
    }:
    (lib.nixosSystem {
      inherit system;
      specialArgs.ix = ixSpecialArgs;
      modules = [
        { nixpkgs.overlays = overlays; }
        ./ix-platform.nix
        ./ix-oci-layer.nix
        # Determinate Nix replaces the in-VM nix package and daemon. The
        # module sets nix.package, configures determinate-nixd as a systemd
        # service, and seeds nix.settings defaults. Compatible with
        # boot.isContainer = true since the daemon runs under our PID 1.
        determinate.nixosModules.default
        # Home Manager as a NixOS module. Per-tool XDG config (Nushell,
        # atuin, zoxide, starship, ...) is configured under
        # `home-manager.users.root` in the base profile; this module
        # exposes the option set and shares the system pkgs.
        home-manager.nixosModules.home-manager
        {
          home-manager = {
            useGlobalPkgs = true;
            useUserPackages = true;
            # Activation renames existing user files with this extension
            # instead of failing, so an operator who hand-edited a config
            # sees the conflict rather than losing the file.
            backupFileExtension = "hm-backup";
          };
        }
      ]
      ++ moduleList
      ++ modules;
    }).config;

  /**
    Build one self-contained OCI archive from a list of NixOS modules.

    Each image is independent: ix does not stack images at runtime, it
    runs one. Returns the OCI-archive derivation; pass it to
    `ix image push` or use it as a `packages.<system>.<name>` output.
  */
  mkImage = args: (evalImageConfig args).ix.build.ociImage;

  /**
    Build a fleet plan helper for a given host system. Returns a function
    that takes a fleet spec and produces the plan/commands tooling consumes.
    `mkFleet` is the default-system shortcut.
  */
  # Shared NixOS bootstrap image used to materialize missing fleet nodes.
  # Reads the canonical name/tag from the image module so the fleet default
  # and the image being published can't drift.
  bootstrapImage =
    (evalImageConfig {
      modules = [ (paths.images + "/system/test-cluster-bootstrap") ];
    }).ix.image;

  mkFleetFor =
    hostSystem:
    let
      hostPkgs = nixpkgs.legacyPackages.${hostSystem};
    in
    import ./fleet.nix {
      inherit
        lib
        evalImageConfig
        writeNushellApplication
        bootstrapImage
        ;
      pkgs = hostPkgs;
      secretsLib = secrets;
      ixFleet = (packageSetFor hostPkgs).ix-fleet;
    };

  mkFleet = mkFleetFor system;

  # Subdirectories of `dir`. Used to walk images/<cat>/<name>/.
  subdirs =
    dir:
    let
      entries = builtins.readDir dir;
    in
    lib.filter (n: entries.${n} == "directory") (builtins.attrNames entries);

  # One image directory -> { <name> = pkg; <name>_<ver> = pkg; ... }.
  # Without versions.nix, the dir is a single module.
  # With versions.nix, each version is layered on top of the base module and
  # the `default` key picks which version gets the unsuffixed alias.
  imagePackages =
    name: path:
    let
      versionsPath = path + "/versions.nix";
    in
    if builtins.pathExists versionsPath then
      let
        versions = import versionsPath { inherit lib artifacts; };
        defaultVer = versions.default;
        verMods = builtins.removeAttrs versions [ "default" ];
        verPkgs = lib.mapAttrs' (
          ver: mod:
          lib.nameValuePair "${name}_${ver}" (mkImage {
            modules = [
              path
              mod
            ];
          })
        ) verMods;
        defaultKey = "${name}_${defaultVer}";
      in
      assert lib.assertMsg (builtins.hasAttr defaultKey verPkgs)
        "image '${name}': versions.nix default = \"${defaultVer}\" but no version with that key";
      verPkgs // { ${name} = verPkgs.${defaultKey}; }
    else
      { ${name} = mkImage { modules = [ path ]; }; };

  /**
    Walk `images/<category>/<name>/` under `root` and expose every
    directory as a flake package. A directory with a `versions.nix`
    sibling produces `<name>_<ver>` for each version key plus a
    `<name>` alias for the `default` version.

    `imageTests` is an optional attrset keyed by image name (matching
    the discovered package names). When an image has an entry, it is
    attached to the image derivation as `passthru.tests.eval` so
    `nix build .#<image>.passthru.tests.eval` runs it (RFC 0119).
  */
  discoverImages =
    {
      root,
      imageTests ? { },
    }:
    let
      imageCategories = lib.filter (cat: cat != "presets") (subdirs root);
      raw = lib.mergeAttrsList (
        lib.concatMap (
          cat: map (name: imagePackages name (root + "/${cat}/${name}")) (subdirs (root + "/${cat}"))
        ) imageCategories
      );
      attach =
        name: pkg:
        if imageTests ? ${name} then
          pkg
          // {
            passthru = (pkg.passthru or { }) // {
              tests = (pkg.passthru.tests or { }) // {
                eval = imageTests.${name};
              };
            };
          }
        else
          pkg;
    in
    lib.mapAttrs attach raw;

  /**
    Discovered example fleets, built for a given host system. Discovery
    walks two layouts side by side: flat `examples/<name>/default.nix`
    and nested `examples/<category>/<name>/default.nix`. A directory is
    treated as a category when it has no `default.nix` of its own. Keys
    in the returned attrset are always the example's own name; the
    category is organizational, mirroring how `discoverImages` flattens
    `images/<cat>/<name>/` into bare names.

    Each fleet is imported with `{ index = { lib = ix; }; }` to match
    the contract examples already use, with `mkFleet` swapped for the
    host-system variant so the wrapper derivations under
    `.up`/`.health`/`.replace` build for the requested system rather
    than always pinning to the default.

    Adding an example is `mkdir examples/<category>/<name> + edit
    default.nix`; this aggregator picks it up on the next eval, no
    registry edits.
  */
  exampleFleetsFor =
    {
      hostSystem,
      # Prepend this to every example node name. The health-checks runner
      # uses "health-check-" so its lifecycle scripts cannot collide with
      # real production VMs that share the natural names (`nginx`,
      # `factions`, ...). Default empty so the regular
      # `packages.<example>-*` wrappers see no change.
      nodePrefix ? "",
    }:
    let
      indexShim = {
        lib = ixReturn // {
          mkFleet = spec: (mkFleetFor hostSystem) (spec // { inherit nodePrefix; });
        };
      };

      isExampleDir = path: builtins.pathExists (path + "/default.nix");

      topEntries = subdirs paths.examples;

      flatPairs = map (name: {
        inherit name;
        path = paths.examples + "/${name}";
      }) (lib.filter (name: isExampleDir (paths.examples + "/${name}")) topEntries);

      categoryDirs = lib.filter (name: !(isExampleDir (paths.examples + "/${name}"))) topEntries;

      nestedPairs = lib.concatMap (
        cat:
        let
          catPath = paths.examples + "/${cat}";
        in
        map (name: {
          inherit name;
          path = catPath + "/${name}";
        }) (lib.filter (name: isExampleDir (catPath + "/${name}")) (subdirs catPath))
      ) categoryDirs;
    in
    lib.listToAttrs (
      map (e: lib.nameValuePair e.name (import e.path { index = indexShim; })) (flatPairs ++ nestedPairs)
    );

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
      discoverImages
      exampleFleetsFor
      artifacts
      agentsMd
      buildBunSite
      buildGradleFatJar
      buildNpmSite
      buildNpmVitest
      buildUvApplication
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
      secrets
      systemdHardening
      uvLockFor
      writeNushellApplication
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
