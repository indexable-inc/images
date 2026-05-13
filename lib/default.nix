# ix/images public lib.
#
# `mkImage` builds one self-contained OCI archive from a list of NixOS
# modules. Each image is independent: ix does not stack images at runtime, it
# runs one. `./ix-base.nix` is the implicit base layer (container marker, OCI
# packaging, base profile enabled by default). The module registry is
# pulled in so option declarations are available to every image, but each
# module is gated on its own `enable` flag and stays inert unless the image
# turns it on.
#
# `discoverImages` walks `images/<category>/<name>/` and turns each directory
# into a flake package. If a directory has a `versions.nix` sibling, every
# version produces `<name>_<ver>` and the `default` key picks the unsuffixed
# `<name>` alias.
#
# `mkMinecraftLoader` is one of several cross-cutting helpers exposed to
# modules via `specialArgs.ix`. Modules access them as `{ ix, ... }: ix.foo`
# instead of relative-path imports.
{
  nixpkgs,
  llm-agents,
  paths,
}:
let
  inherit (nixpkgs) lib;

  system = "x86_64-linux";
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
      meta ? { },
    }:
    let
      runtimePath = lib.makeBinPath ([ python ] ++ runtimeInputs);
      srcPath = src;
      argv = builtins.toJSON ([ "${srcPath}" ] ++ args);
      pyrightConfig = pkgs.writeText "basedpyright-${name}.json" (
        builtins.toJSON {
          include = [ (builtins.toString src) ];
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
      # `--ide-check` parses the generated script and reports diagnostics
      # without running `main`, so command wrappers are checked at build time.
      checkPhase = lib.optionalString check ''
        ${lib.getExe pkgs.nushell} --no-config-file --no-std-lib --ide-check 100 "$target"
      '';
      meta = meta // {
        mainProgram = meta.mainProgram or name;
      };
    };

  # Overlays: llm-agents provides claude-code and codex; plus repo-local
  # packages consumed by NixOS modules via `pkgs.<name>`. Packages that are
  # only exposed as flake outputs (e.g. tonbo-artifacts) stay out of the
  # overlay so they don't pollute the nixpkgs namespace inside images.
  overlay = final: _prev: {
    minecraft-hot-reload-agent = final.callPackage paths.nixPackages.minecraftHotReloadAgent { };
    minecraft-rcon = final.callPackage paths.nixPackages.minecraftRcon {
      writePythonApplication = writePythonApplication final;
    };
  };
  overlays = [
    llm-agents.overlays.default
    overlay
  ];
  pkgs = import nixpkgs { inherit system overlays; };

  # The module registry. collect picks all leaf paths from the nested attrset.
  moduleList = lib.collect builtins.isPath (import paths.modules);

  buildNpmSite = import ./build-npm-site.nix;
  buildGradleFatJar = import ./build-gradle-fat-jar.nix { inherit lib; };

  mkMinecraftLoader = import ./minecraft-loader.nix;
  mkMinecraftSyncManaged =
    args:
    import ./minecraft-sync-managed.nix (
      {
        src = paths.tools.minecraftSyncManaged;
        inherit writePythonApplication;
      }
      // args
    );

  # Fetch a static artifact (mod jar, plugin, server) by URL + SRI hash.
  # Hashes live next to URLs in the consuming catalog rather than in flake
  # inputs, so a routine mod bump touches one JSON file and not flake.lock.
  mkArtifact = { url, hash, ... }: pkgs.fetchurl { inherit url hash; };

  attachArtifactSources = lib.mapAttrs (_: entry: entry // { src = mkArtifact entry; });

  paperServer1_21_11 = mkArtifact {
    url = "https://api.papermc.io/v2/projects/paper/versions/1.21.11/builds/69/downloads/paper-1.21.11-69.jar";
    hash = "sha256-zzdPKvnXHfzHU0Pze3IqerywkcV0ExuV47E8b8LLj64=";
  };

  artifacts = {
    inherit attachArtifactSources;
    minecraft = {
      paperPluginCatalog = attachArtifactSources {
        luckperms = {
          url = "https://cdn.modrinth.com/data/Vebnzrzj/versions/OrIs0S6b/LuckPerms-Bukkit-5.5.17.jar";
          hash = "sha256-1bFgo5cag3LMWDW81VXjfBqmHp3TBVmSGl9CGhG/l90=";
          pluginName = "LuckPerms";
        };
      };
      servers =
        lib.mapAttrs (_: mkArtifact) {
          "26w17a-fabric" = {
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
          "1.21.11-paper" = paperServer1_21_11;
        };
      paperServers."1.21.11" = {
        build = 69;
        src = paperServer1_21_11;
      };
      plugins.plugmanx = mkArtifact {
        url = "https://cdn.modrinth.com/data/yro4niHu/versions/hrMAp7Ww/PlugManX-3.0.4.jar";
        hash = "sha256-LLb7Ddfm9YZ7ypv6PwN1HW2J1rlJ6LbTdAUHtVrmqcA=";
      };
    };
  };

  # x86_64-linux-only binary blob distributed by upstream Tonbo. Lives here
  # rather than in `packageSetFor` because it has no useful cross-compile.
  tonbo-artifacts = pkgs.callPackage paths.nixPackages.tonboArtifacts { };

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
    };

  # Helpers exposed to every module via specialArgs. Keep this surface small
  # and stable: anything here is part of the cross-module contract.
  ixSpecialArgs = {
    inherit
      artifacts
      buildGradleFatJar
      buildNpmSite
      mkMinecraftLoader
      mkMinecraftSyncManaged
      writeNushellApplication
      writePythonApplication
      ;
    packages = packageSetFor pkgs;
  };

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

  mkImage = args: (evalImageConfig args).ix.build.ociImage;

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

  discoverImages =
    root:
    lib.foldl' (
      acc: cat:
      lib.foldl' (acc': name: acc' // imagePackages name (root + "/${cat}/${name}")) acc (
        subdirs (root + "/${cat}")
      )
    ) { } (subdirs root);
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
    tonbo-artifacts
    writeNushellApplication
    writePythonApplication
    ;
}
