# ix/images public lib. Helpers documented per binding with RFC-0145
# doc-comments below; the file's job is to wire them together.
{
  nixpkgs,
  paths,
}:
let
  inherit (nixpkgs) lib;

  system = "x86_64-linux";

  /**
    Package a Python entrypoint as a standalone executable.

    Wraps `src` in a launcher script that prepends `runtimeInputs` to PATH
    and runs the file under `python`. When `check` is true (default), the
    derivation also runs `basedpyright` over `src` with `typeCheckingMode`
    enforcement during the build, so type regressions fail the build instead
    of surfacing at runtime.

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
      typeCheckingMode ? "all",
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
  overlay = final: _prev: {
    minecraft-hot-reload-agent = final.callPackage paths.packages.minecraftHotReloadAgent { };
    minecraft-rcon = final.callPackage paths.packages.minecraftRcon {
      writePythonApplication = writePythonApplication final;
    };
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

  buildNpmSite = import ./build-npm-site.nix;
  buildGradleFatJar = import ./build-gradle-fat-jar.nix { inherit lib; };

  systemdHardening = import ./systemd-hardening.nix;

  mkMinecraftLoader = import ./minecraft-loader.nix;
  mkMinecraftSyncManaged =
    args:
    import ./minecraft-sync-managed.nix (
      {
        src = paths.packages.minecraftSyncManaged + "/minecraft-sync-managed.py";
        inherit writePythonApplication;
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
    "1.21.11" = {
      build = 69;
      src = mkArtifact {
        url = "https://api.papermc.io/v2/projects/paper/versions/1.21.11/builds/69/downloads/paper-1.21.11-69.jar";
        hash = "sha256-zzdPKvnXHfzHU0Pze3IqerywkcV0ExuV47E8b8LLj64=";
      };
    };
  };

  /**
    Per-version Fabric/NeoForge/Sponge mod catalogs generated by
    `tools/update-mods.py` from `<paths.minecraftMods>/manifest.json`.

    The bare-JSON catalog (slug -> `{ url, hash }`) is enriched into
    `{ url, hash, src }` so callers can pass it straight to
    `services.minecraft.modCatalog`. Examples and images consume
    `modCatalogs.<gameVersion>` by name; to add a mod, edit the manifest
    and run `nix run .#update-mods` and do not inline URLs anywhere
    downstream.
  */
  modCatalogs =
    let
      gameVersions = lib.pipe paths.minecraftMods [
        builtins.readDir
        (lib.filterAttrs (
          name: type: type == "regular" && lib.hasSuffix ".json" name && name != "manifest.json"
        ))
        builtins.attrNames
        (map (lib.removeSuffix ".json"))
      ];
      catalogFor =
        ver:
        attachArtifactSources (
          builtins.fromJSON (builtins.readFile (paths.minecraftMods + "/${ver}.json"))
        );
    in
    lib.genAttrs gameVersions catalogFor;

  /**
    Pinned artifact catalogs surfaced to images and examples by name.
    Examples must consume entries through this set (or one of the module
    options it seeds) rather than inlining URLs and hashes.
  */
  artifacts = {
    inherit attachArtifactSources;
    minecraft = {
      inherit paperServers modCatalogs;
      paperPluginCatalog = attachArtifactSources {
        luckperms = {
          url = "https://cdn.modrinth.com/data/Vebnzrzj/versions/OrIs0S6b/LuckPerms-Bukkit-5.5.17.jar";
          hash = "sha256-1bFgo5cag3LMWDW81VXjfBqmHp3TBVmSGl9CGhG/l90=";
          pluginName = "LuckPerms";
        };
      };
      servers =
        lib.mapAttrs (_: mkArtifact) {
          "26.2-snapshot-5-fabric" = {
            url = "https://meta.fabricmc.net/v2/versions/loader/26.2-snapshot-5/0.19.2/1.1.1/server/jar";
            hash = "sha256-IZctWQu9VH4Z5lU/VcEzvPGLfW8boOAXtCaQlKXyA5k=";
          };
          "26.2-snapshot-6-fabric" = {
            url = "https://meta.fabricmc.net/v2/versions/loader/26.2-snapshot-6/0.19.2/1.1.1/server/jar";
            hash = "sha256-J4zGg7YlrHmYBsagTr+x2ZcAgOvj5vr/8iVgwMVG/e0=";
          };
          "26.1.2-fabric" = {
            url = "https://meta.fabricmc.net/v2/versions/loader/26.1.2/0.19.2/1.1.1/server/jar";
            hash = "sha256-6RvRm5/w4ExXhD5iTS9U0KPjmgSMr8pejiDrmENEXb0=";
          };
          "1.21.11-fabric" = {
            url = "https://meta.fabricmc.net/v2/versions/loader/1.21.11/0.19.2/1.1.1/server/jar";
            hash = "sha256-xDK1HU7Xwbr0Z7pw7Dtdtob0zvlfq9pZ9J4O32u4jBc=";
          };
        }
        // {
          "1.21.11-paper" = paperServers."1.21.11".src;
        };
      plugins.plugmanx = mkArtifact {
        url = "https://cdn.modrinth.com/data/yro4niHu/versions/hrMAp7Ww/PlugManX-3.0.4.jar";
        hash = "sha256-LLb7Ddfm9YZ7ypv6PwN1HW2J1rlJ6LbTdAUHtVrmqcA=";
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
      minestom.helloServerJar = pkgs.callPackage paths.packages.minestom.servers.hello {
        ix = ixForPackages;
      };
      tonbo-artifacts = pkgs.callPackage paths.packages.tonboArtifacts { };
    };

  /**
    Cross-cutting helpers handed to every module through `specialArgs.ix`.
    Keep this surface small and stable: anything here is part of the
    cross-module contract.
  */
  ixSpecialArgs = {
    inherit
      artifacts
      buildGradleFatJar
      buildNpmSite
      mkMinecraftLoader
      mkMinecraftSyncManaged
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
  mkFleetFor =
    hostSystem:
    import ./fleet.nix {
      inherit
        lib
        evalImageConfig
        writeNushellApplication
        ;
      ixFleetScript = paths.tools.ixFleet;
      pkgs = nixpkgs.legacyPackages.${hostSystem};
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
      raw = lib.mergeAttrsList (
        lib.concatMap (
          cat: map (name: imagePackages name (root + "/${cat}/${name}")) (subdirs (root + "/${cat}"))
        ) (subdirs root)
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
    buildGradleFatJar
    buildNpmSite
    mkMinecraftLoader
    mkMinecraftSyncManaged
    packageSetFor
    systemdHardening
    writeNushellApplication
    writePythonApplication
    ;
}
