# ix/images public lib. Helpers documented per binding with RFC-0145
# doc-comments below; the file's job is to wire them together.
{
  nixpkgs,
  paths,
  rust-overlay,
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
      pyrightConfig = pkgs.writeText "basedpyright-${name}.json" (
        builtins.toJSON {
          include = [ (builtins.toString src) ];
          inherit extraPaths;
          inherit typeCheckingMode pythonPlatform;
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
  overlay =
    final: _prev:
    let
      ixForOverlay = {
        buildRustPackage = pkgs: (rustFor pkgs).buildPackage;
      };
      checkedOciImageBuilder = final.callPackage paths.packages.ociImageBuilder {
        pkgs = final;
        ix = ixForOverlay;
      };
    in
    {
      minecraft-hot-reload-agent = final.callPackage paths.packages.minecraftHotReloadAgent { };
      minecraft-rcon = final.callPackage paths.packages.minecraftRcon {
        writePythonApplication = writePythonApplication final;
      };
      oci-image-builder = checkedOciImageBuilder.passthru.unchecked or checkedOciImageBuilder;
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
  bunLock = bunLockFor pkgs;
  buildBunSite = import ./build-bun-site.nix {
    inherit bunLockFor;
  };
  buildNpmSite = import ./build-npm-site.nix;
  uvLockFor =
    pkgs:
    import ./uv-lock.nix {
      inherit lib pkgs;
    };
  uvLock = uvLockFor pkgs;
  buildUvApplication = import ./build-uv-application.nix {
    inherit uvLockFor;
  };
  buildGradleFatJar = import ./build-gradle-fat-jar.nix { inherit lib; };
  rustNightlyChannel = "nightly-2026-05-17";
  pkgsWithRustOverlayFor =
    pkgs: if builtins.hasAttr "rust-bin" pkgs then pkgs else pkgs.extend rust-overlay.overlays.default;
  rustNightlyToolchainFor =
    pkgs:
    (pkgsWithRustOverlayFor pkgs).rust-bin.fromRustupToolchain {
      channel = rustNightlyChannel;
      components = [
        "cargo"
        "rust-std"
        "rustc"
      ];
      profile = "minimal";
    };
  rustNightlyClippyToolchainFor =
    pkgs:
    (pkgsWithRustOverlayFor pkgs).rust-bin.fromRustupToolchain {
      channel = rustNightlyChannel;
      components = [
        "cargo"
        "llvm-tools"
        "rust-src"
        "rust-std"
        "rustc"
        "rustc-dev"
        "rustfmt"
      ];
      profile = "minimal";
    };
  llmClippyFor =
    pkgs:
    (pkgsWithRustOverlayFor pkgs).callPackage paths.packages.llmClippy {
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
  cargoUnitFor =
    pkgs:
    let
      rust = rustFor pkgs;
      checkedNixCargoUnit = pkgs.callPackage paths.packages.nixCargoUnit {
        inherit pkgs;
        ix = {
          buildRustPackage = pkgs: (rustFor pkgs).buildPackage;
        };
      };
    in
    import ./cargo-unit.nix {
      inherit lib pkgs rust;
      nixCargoUnit = checkedNixCargoUnit.passthru.unchecked or checkedNixCargoUnit;
    };
  cargoUnit = cargoUnitFor pkgs;

  /**
    Build a repo-owned Rust package with the shared Rust policy.

    Wraps `rustPlatform.buildRustPackage`, enables parallel test execution by
    default, and attaches the repo's `llm-clippy` and unused-dependency checks
    as `passthru.tests` plus policy dependencies of the returned package.
  */
  buildRustPackage = pkgs: (rustFor pkgs).buildPackage;

  systemdHardening = import ./systemd-hardening.nix;

  mkMinecraftLoader = import ./minecraft-loader.nix;

  /**
    Nix constructors for typed Minecraft NBT values.

    Plain Nix attrsets, lists, strings, booleans, integers, and floats can be
    encoded as compound, list, string, byte, int/long, and double tags. These
    constructors are the explicit escape hatch for Minecraft's narrower tag
    types: bytes, shorts, floats, typed numeric arrays, and named roots.
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
      checkedMinecraftNbt = pkgs.callPackage paths.packages.minecraftNbt {
        inherit pkgs;
        ix = {
          buildRustPackage = pkgs: (rustFor pkgs).buildPackage;
        };
      };
      minecraftNbt = checkedMinecraftNbt.passthru.unchecked or checkedMinecraftNbt;
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
    let
      checkedPackage = pkgs.callPackage paths.packages.minecraftSyncManaged {
        inherit pkgs;
        ix = {
          buildRustPackage = pkgs: (rustFor pkgs).buildPackage;
        };
      };
    in
    import ./minecraft-sync-managed.nix (
      {
        package = checkedPackage.passthru.unchecked or checkedPackage;
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
      ixForPackages = ixSpecialArgs // {
        inherit pkgs;
      };
    in
    {
      hyperion = (pkgsWithRustOverlayFor pkgs).callPackage paths.packages.hyperion {
        ix = ixForPackages;
      };
      ix-fleet = pkgs.callPackage paths.packages.ixFleet {
        ix = ixForPackages;
      };
      minestom.helloServerJar = pkgs.callPackage paths.packages.minestom.servers.hello {
        ix = ixForPackages;
      };
      minecraft-nbt = pkgs.callPackage paths.packages.minecraftNbt {
        inherit pkgs;
        ix = ixForPackages;
      };
      llm-clippy = llmClippyFor pkgs;
      minecraft-sync-managed = pkgs.callPackage paths.packages.minecraftSyncManaged {
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
      python-mcp-server = pkgs.callPackage paths.packages.pythonMcpServer {
        ix = ixForPackages;
      };
      tonbo-artifacts = pkgs.callPackage paths.packages.tonboArtifacts { };
      ix = pkgs.callPackage paths.packages.ix { };
    };

  /**
    Cross-cutting helpers handed to every module through `specialArgs.ix`.
    Keep this surface small and stable: anything here is part of the
    cross-module contract.
  */
  ixSpecialArgs = {
    inherit
      artifacts
      buildBunSite
      buildGradleFatJar
      buildRustPackage
      buildNpmSite
      buildUvApplication
      bunLock
      bunLockFor
      cargoUnit
      cargoUnitFor
      minecraft
      mkMinecraftLoader
      mkMinecraftNbtFormat
      mkMinecraftSyncManaged
      systemdHardening
      uvLock
      uvLockFor
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
in
{
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
    artifacts
    buildBunSite
    buildGradleFatJar
    buildNpmSite
    buildUvApplication
    bunLock
    bunLockFor
    cargoUnit
    cargoUnitFor
    minecraft
    mkMinecraftLoader
    mkMinecraftNbtFormat
    mkMinecraftSyncManaged
    packageSetFor
    systemdHardening
    uvLock
    uvLockFor
    writeNushellApplication
    writePythonApplication
    ;
}
