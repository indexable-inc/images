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
  artifactInputs,
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
  # packages used by images.
  overlay = final: _prev: {
    minecraft-hot-reload-agent = final.callPackage paths.nixPackages.minecraftHotReloadAgent { };
    minecraft-rcon = final.callPackage paths.nixPackages.minecraftRcon {
      writePythonApplication = writePythonApplication final;
    };
    tonbo-artifacts = final.callPackage paths.nixPackages.tonboArtifacts {
      src = artifactInputs.artifact-tonbo-artifacts;
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

  artifactByUrl = {
    "https://cdn.modrinth.com/data/Gi02250Z/versions/7IRzJzBP/almanac-1.26.x-fabric-1.6.2.1.jar" =
      artifactInputs.artifact-minecraft-mod-almanac;
    "https://cdn.modrinth.com/data/swbUV1cr/versions/D9j76thC/bluemap-5.20-fabric.jar" =
      artifactInputs.artifact-minecraft-mod-bluemap;
    "https://cdn.modrinth.com/data/VSNURh3q/versions/h0G6V9wK/c2me-fabric-mc26.2-snapshot-5-0.3.7%2Balpha.0.68.jar" =
      artifactInputs.artifact-minecraft-mod-c2me-fabric-26-2-snapshot-5;
    "https://cdn.modrinth.com/data/VSNURh3q/versions/utLSz8Lf/c2me-fabric-mc26.1.2-0.3.7%2Balpha.0.68.jar" =
      artifactInputs.artifact-minecraft-mod-c2me-fabric-26-1-2;
    "https://cdn.modrinth.com/data/fALzjamp/versions/4Eotm6ov/Chunky-Fabric-1.5.3.jar" =
      artifactInputs.artifact-minecraft-mod-chunky;
    "https://cdn.modrinth.com/data/Wnxd13zP/versions/RXNrUIjA/Clumps-fabric-26.1.2-26.1.2.1.jar" =
      artifactInputs.artifact-minecraft-mod-clumps;
    "https://cdn.modrinth.com/data/uCdwusMi/versions/FJrLlu3p/DistantHorizons-3.0.3-b-26.1.2-fabric-neoforge.jar" =
      artifactInputs.artifact-minecraft-mod-distanthorizons;
    "https://cdn.modrinth.com/data/P7dR8mSH/versions/dZsorAUN/fabric-api-0.147.0%2B26.1.2.jar" =
      artifactInputs.artifact-minecraft-mod-fabric-api-26-1-2;
    "https://cdn.modrinth.com/data/P7dR8mSH/versions/i5tSkVBH/fabric-api-0.141.3%2B1.21.11.jar" =
      artifactInputs.artifact-minecraft-mod-fabric-api-1-21-11;
    "https://cdn.modrinth.com/data/P7dR8mSH/versions/kw0Rlte8/fabric-api-0.147.1%2B26.2.jar" =
      artifactInputs.artifact-minecraft-mod-fabric-api-26-2;
    "https://cdn.modrinth.com/data/uXXizFIs/versions/d5ddUdiB/ferritecore-9.0.0-fabric.jar" =
      artifactInputs.artifact-minecraft-mod-ferrite-core;
    "https://cdn.modrinth.com/data/LJNGWSvH/versions/65YzWD8i/grimac-fabric-2.3.74-ce86075.jar" =
      artifactInputs.artifact-minecraft-mod-grimac;
    "https://cdn.modrinth.com/data/fQEb0iXm/versions/kYAGItyj/krypton-0.3.0.jar" =
      artifactInputs.artifact-minecraft-mod-krypton;
    "https://cdn.modrinth.com/data/2ecVyZ49/versions/kL32PN9Q/Ksyxis-1.4.3.jar" =
      artifactInputs.artifact-minecraft-mod-ksyxis;
    "https://cdn.modrinth.com/data/gvQqBUqZ/versions/R7MxYvuW/lithium-fabric-0.24.2%2Bmc26.1.2.jar" =
      artifactInputs.artifact-minecraft-mod-lithium;
    "https://cdn.modrinth.com/data/XaDC71GB/versions/cHH1mPJL/lithostitched-1.7.2-fabric-26.1.jar" =
      artifactInputs.artifact-minecraft-mod-lithostitched;
    "https://cdn.modrinth.com/data/vE2FN5qn/versions/eW5P1rHo/letmedespawn-1.26.x-fabric-1.6.2.1.jar" =
      artifactInputs.artifact-minecraft-mod-lmd;
    "https://cdn.modrinth.com/data/Vebnzrzj/versions/fTIdfb46/LuckPerms-Fabric-5.5.42.jar" =
      artifactInputs.artifact-minecraft-mod-luckperms;
    "https://cdn.modrinth.com/data/Vebnzrzj/versions/OrIs0S6b/LuckPerms-Bukkit-5.5.17.jar" =
      artifactInputs.artifact-minecraft-plugin-luckperms-bukkit;
    "https://cdn.modrinth.com/data/4WWQxlQP/versions/P8k080Af/servercore-fabric-1.5.16%2B26.1.jar" =
      artifactInputs.artifact-minecraft-mod-servercore;
    "https://cdn.modrinth.com/data/9eGKb6K1/versions/gVPjsMto/voicechat-fabric-2.6.17%2B26.1.2.jar" =
      artifactInputs.artifact-minecraft-mod-simple-voice-chat;
    "https://cdn.modrinth.com/data/l6YH9Als/versions/J1GUYyGQ/spark-1.10.172-fabric.jar" =
      artifactInputs.artifact-minecraft-mod-spark;
    "https://cdn.modrinth.com/data/lWDHr9jE/versions/jL2ZsTzx/tectonic-3.0.22-fabric-26.1.jar" =
      artifactInputs.artifact-minecraft-mod-tectonic;
    "https://github.com/xandergos/terrain-diffusion-mc/releases/download/v2.1.0/terrain-diffusion-mc-2.1.0-cpu%2B1.21.11.jar" =
      artifactInputs.artifact-minecraft-mod-terrain-diffusion;
    "https://cdn.modrinth.com/data/8oi3bsk5/versions/FCzSjHeG/Terralith_26.1_v2.6.2_Fabric.jar" =
      artifactInputs.artifact-minecraft-mod-terralith;
    "https://cdn.modrinth.com/data/wnEe9KBa/versions/9f7J0dAp/vmp-fabric-mc26.1.2-0.2.0%2Bbeta.7.234-all.jar" =
      artifactInputs.artifact-minecraft-mod-vmp-fabric;
  };

  attachArtifactSources =
    catalog:
    lib.mapAttrs (
      slug: entry:
      entry
      // {
        src =
          artifactByUrl.${entry.url} or (throw "mod '${slug}': no flake artifact input for ${entry.url}");
      }
    ) catalog;

  paperPluginCatalog = attachArtifactSources {
    luckperms = {
      url = "https://cdn.modrinth.com/data/Vebnzrzj/versions/OrIs0S6b/LuckPerms-Bukkit-5.5.17.jar";
      pluginName = "LuckPerms";
    };
  };

  artifacts = {
    inherit attachArtifactSources;
    minecraft = {
      inherit paperPluginCatalog;
      servers = {
        "26w17a-fabric" = artifactInputs.artifact-minecraft-server-26w17a-fabric;
        "26.2-snapshot-6-fabric" = artifactInputs.artifact-minecraft-server-26-2-snapshot-6-fabric;
        "26.1.2-fabric" = artifactInputs.artifact-minecraft-server-26-1-2-fabric;
        "1.21.11-fabric" = artifactInputs.artifact-minecraft-server-1-21-11-fabric;
        "1.21.11-paper" = artifactInputs.artifact-minecraft-server-1-21-11-paper;
      };
      paperServers."1.21.11" = {
        build = 69;
        src = artifactInputs.artifact-minecraft-server-1-21-11-paper;
      };
      plugins = {
        plugmanx = artifactInputs.artifact-minecraft-plugin-plugmanx;
      };
    };
  };

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
    writeNushellApplication
    writePythonApplication
    ;
}
