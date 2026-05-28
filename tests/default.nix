# Eval tests. Each image with image-specific assertions has its own group
# below, exposed as `imageTests.<name>` so it can be attached to the image
# derivation via `passthru.tests`. `eval` aggregates them along with the
# cross-image checks (fleet, helpers).
{
  nixpkgs,
  ix,
}:
let
  inherit (nixpkgs) lib;
  inherit (ix) pkgs;
  fs = lib.fileset;
  repoPackages = ix.packageSetFor pkgs;
  packageRegistry = import ../packages/registry.nix {
    inherit lib;
    root = ../packages;
  };
  missingPackageMetadata = map (
    dir: lib.removePrefix "${builtins.toString ../packages}/" (builtins.toString dir)
  ) packageRegistry.packageDirsWithoutMetadata;

  versions = import ../images/games/minecraft/versions.nix {
    inherit lib;
    inherit (ix) artifacts;
  };
  defaultMinecraftVersion = versions.default;
  defaultMinecraftModule = versions.${defaultMinecraftVersion};
  rustToolchainFile = lib.importTOML ../rust-toolchain.toml;
  rustPinnedNightlyDate = lib.removePrefix "nightly-" rustToolchainFile.toolchain.channel;

  # Thin wrapper to keep call sites as plain lists; delegates to ix.evalImageConfig
  # so tests exercise the same evaluation path as production image builds.
  evalConfig = modules: ix.evalImageConfig { inherit modules; };
  failedAssertionsFor =
    modules:
    let
      config = evalConfig modules;
    in
    builtins.filter (assertion: !assertion.assertion) config.assertions;
  samePorts = left: right: lib.sort (a: b: a < b) left == lib.sort (a: b: a < b) right;
  # ix guest sidecars are opened by the shared platform base config.
  baseFirewallTcpPorts = [ 5001 ];
  baseFirewallUdpPorts = [ 8443 ];

  minecraft =
    let
      config = evalConfig [
        ../images/games/minecraft
        defaultMinecraftModule
      ];
    in
    {
      inherit config;
      cfg = config.services.minecraft;
      service =
        let
          unit = config.systemd.services.minecraft;
        in
        {
          inherit unit;
          config = unit.serviceConfig;
        };

      paper =
        let
          config = evalConfig [
            ../images/games/minecraft
            versions."1.21.11-paper"
          ];
        in
        {
          inherit config;
          cfg = config.services.minecraft;
          service =
            let
              unit = config.systemd.services.minecraft;
            in
            {
              inherit unit;
              config = unit.serviceConfig;
            };
          managed = {
            serverFiles = config.environment.etc."minecraft/managed-server-files".source;
            dropins = config.environment.etc."minecraft/managed-dropins".source;
          };
        };

      rcon =
        let
          config = evalConfig [
            ../images/games/minecraft
            defaultMinecraftModule
            {
              services.minecraft.rcon.enable = true;
            }
          ];
        in
        {
          inherit config;
          cfg = config.services.minecraft;
          managed.serverFiles = config.environment.etc."minecraft/managed-server-files".source;

          openFirewall =
            let
              config = evalConfig [
                ../images/games/minecraft
                defaultMinecraftModule
                {
                  services.minecraft.rcon = {
                    enable = true;
                    port = 25576;
                    openFirewall = true;
                  };
                }
              ];
            in
            {
              inherit config;
              cfg = config.services.minecraft;
            };
        };

      worldBorder =
        let
          config = evalConfig [
            ../images/games/minecraft
            defaultMinecraftModule
            {
              services.minecraft.worldBorder = {
                enable = true;
                center = {
                  x = 100;
                  z = -50;
                };
                diameter = 8000;
              };
            }
          ];
          service = config.systemd.services.minecraft-world-border;
        in
        {
          inherit config service;
          cfg = config.services.minecraft;
        };

      paperPlugins =
        let
          config = evalConfig [
            ../images/games/minecraft
            versions."26.1.2-paper"
            {
              services.minecraft.plugins = {
                pvpindex-factions = { };
                simple-voice-chat.port = 24455;
                terraformgenerator.worlds = [
                  "factions"
                  "factions_nether"
                  "factions_the_end"
                ];
                worldedit = { };
              };
              services.minecraft.properties.level-name = "factions";
            }
          ];
        in
        {
          inherit config;
          cfg = config.services.minecraft;
        };

      nestedProperties =
        let
          config = evalConfig [
            ../images/games/minecraft
            defaultMinecraftModule
            {
              services.minecraft.properties = {
                query = {
                  port = 25565;
                };
                rcon = {
                  port = 25575;
                };
              };
            }
          ];
        in
        {
          inherit config;
          managed.serverFiles = config.environment.etc."minecraft/managed-server-files".source;
        };

      access =
        let
          json = pkgs.formats.json { };
          config = evalConfig [
            ../images/games/minecraft
            defaultMinecraftModule
            {
              services.minecraft = {
                whitelist.enable = true;
                players = {
                  Alice = {
                    uuid = "00000000-0000-0000-0000-000000000001";
                    whitelist = true;
                    operator = {
                      enable = true;
                      level = 3;
                      bypassesPlayerLimit = true;
                    };
                  };

                  Bob = {
                    uuid = "00000000-0000-0000-0000-000000000002";
                    whitelist = true;
                  };
                };
              };
            }
          ];
        in
        {
          inherit config;
          cfg = config.services.minecraft;
          fixtures = {
            whitelist = {
              current = json.generate "minecraft-whitelist-current.json" [
                {
                  uuid = "00000000-0000-0000-0000-000000000001";
                  name = "OldAlice";
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000003";
                  name = "Manual";
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000004";
                  name = "Removed";
                }
              ];

              previous = json.generate "minecraft-whitelist-previous.json" [
                {
                  uuid = "00000000-0000-0000-0000-000000000001";
                  name = "OldAlice";
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000004";
                  name = "Removed";
                }
              ];
            };

            operators = {
              current = json.generate "minecraft-operators-current.json" [
                {
                  uuid = "00000000-0000-0000-0000-000000000001";
                  name = "OldAlice";
                  level = 1;
                  bypassesPlayerLimit = false;
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000005";
                  name = "ManualOp";
                  level = 4;
                  bypassesPlayerLimit = false;
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000006";
                  name = "RemovedOp";
                  level = 4;
                  bypassesPlayerLimit = false;
                }
              ];

              previous = json.generate "minecraft-operators-previous.json" [
                {
                  uuid = "00000000-0000-0000-0000-000000000001";
                  name = "OldAlice";
                  level = 1;
                  bypassesPlayerLimit = false;
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000006";
                  name = "RemovedOp";
                  level = 4;
                  bypassesPlayerLimit = false;
                }
              ];
            };
          };
          service =
            let
              unit = config.systemd.services.minecraft;
            in
            {
              inherit unit;
              config = unit.serviceConfig;
            };
          managed = {
            access = config.environment.etc."minecraft/managed-access".source;
            serverFiles = config.environment.etc."minecraft/managed-server-files".source;
          };
          syncManaged = ix.mkMinecraftSyncManaged {
            inherit pkgs;
            inherit (config.services.minecraft) dropinDir;
            dataDir = "/build/minecraft-access-data";
            managedRoot = "/build/minecraft-managed-root";
            plugmanReloadEnabled = false;
            rconEnabled = false;
            ignoredPlugins = [ ];
            datapackWorlds = [ ];
            rconPort = config.services.minecraft.rcon.port;
            rconPasswordFile = "/build/minecraft-access-data/.ix-rcon-password";
            rconBroadcastToOps = false;
          };
        };

      nbt =
        let
          tags = ix.minecraft.nbt;
          config = evalConfig [
            ../images/games/minecraft
            defaultMinecraftModule
            {
              services.minecraft = {
                serverFiles = {
                  "generated/example.snbt" = tags.compound {
                    DataVersion = tags.int 4325;
                    Enabled = tags.bool true;
                    Health = tags.short 20;
                    Angle = tags.float 0.5;
                    Precise = tags.double 12.25;
                    Flags = tags.byteArray [
                      1
                      0
                      (-1)
                    ];
                    Spawn = tags.compound {
                      Dimension = tags.string "minecraft:overworld";
                      Pos = tags.list [
                        (tags.double 1.5)
                        (tags.double 65.25)
                        (tags.double (-30.5))
                      ];
                    };
                  };

                  "generated/example.nbt" = tags.root "ix" (
                    tags.compound {
                      Name = tags.string "binary";
                      Values = tags.intArray [
                        1
                        2
                        3
                      ];
                    }
                  );

                  "generated/example.nbt.gz" = tags.compound {
                    Name = tags.string "compressed";
                  };
                };

                configFiles."generated/client.snbt" = tags.compound {
                  Side = tags.string "config";
                };
              };
            }
          ];
        in
        {
          inherit config;
          cfg = config.services.minecraft;
          managed = {
            config = config.environment.etc."minecraft/managed-config".source;
            serverFiles = config.environment.etc."minecraft/managed-server-files".source;
          };
        };

      datapacks =
        let
          config = evalConfig [
            ../images/games/minecraft
            defaultMinecraftModule
            {
              services.minecraft = {
                properties.level-name = "My World";
                datapacks."max-height".dimensionTypes.overworld = {
                  min_y = -2032;
                  height = 4064;
                  logical_height = 4064;
                };
              };
            }
          ];
        in
        {
          inherit config;
          cfg = config.services.minecraft;
          service =
            let
              unit = config.systemd.services.minecraft;
            in
            {
              inherit unit;
              config = unit.serviceConfig;
            };
          managed.datapacks = config.environment.etc."minecraft/managed-datapacks".source;
          syncManaged = ix.mkMinecraftSyncManaged {
            inherit pkgs;
            inherit (config.services.minecraft) dropinDir;
            dataDir = "/build/minecraft-datapack-data";
            managedRoot = "/build/minecraft-datapack-managed-root";
            plugmanReloadEnabled = false;
            rconEnabled = false;
            ignoredPlugins = [ ];
            datapackWorlds = config.services.minecraft.datapacks."max-height".worlds;
            rconPort = config.services.minecraft.rcon.port;
            rconPasswordFile = "/build/minecraft-datapack-data/.ix-rcon-password";
            rconBroadcastToOps = false;
          };
        };
    };

  bedrock =
    let
      config = evalConfig [ ../images/games/minecraft-bedrock ];
    in
    {
      inherit config;
      cfg = config.services.minecraft-bedrock;
      service =
        let
          unit = config.systemd.services.minecraft-bedrock;
        in
        {
          inherit unit;
          config = unit.serviceConfig;
        };
    };

  remoteDesktop =
    let
      config = evalConfig [ ../images/desktop/remote-desktop ];
    in
    {
      inherit config;
      cfg = config.services.remote-desktop;
      service =
        let
          unit = config.systemd.services.remote-desktop;
        in
        {
          inherit unit;
          config = unit.serviceConfig;
        };
    };

  remoteDesktopModuleDefault =
    let
      config = evalConfig [
        {
          services.remote-desktop.enable = true;
        }
      ];
    in
    {
      inherit config;
      cfg = config.services.remote-desktop;
    };

  resourceMonitor =
    let
      config = evalConfig [
        {
          services.resource-monitor = {
            enable = true;
            runtimeDirectory = "/run/ix/resource-monitor";
          };
        }
      ];
      unit = config.systemd.services.resource-monitor;
    in
    {
      inherit config;
      cfg = config.services.resource-monitor;
      service = {
        inherit unit;
        config = unit.serviceConfig;
      };
    };

  kernelDev =
    let
      config = evalConfig [ ../images/dev/kernel-dev ];
    in
    {
      inherit config;
      git.clone = {
        service = config.systemd.services.git-clone;
        timer = config.systemd.timers.git-clone;
      };
    };

  developmentBase =
    let
      config = evalConfig [ ../images/dev/development-base ];
    in
    {
      inherit config;
      # Outer pkgs has no allowUnfree, so forcing pkgs.claude-code here would
      # throw at eval; use lib.getName over the rendered systemPackages list.
      packageNames = map lib.getName config.environment.systemPackages;
    };

  symphonyCodex =
    let
      config = evalConfig [ ../images/dev/symphony-codex ];
    in
    {
      inherit config;
      packages = config.environment.systemPackages;
      packageNames = map lib.getName config.environment.systemPackages;
    };

  pythonAppClosureProbe = ix.writePythonApplication pkgs {
    name = "python-app-closure-probe";
    src = pkgs.writeText "python-app-closure-probe.py" ''
      print("python app source is in the runtime closure")
    '';
    check = false;
  };

  processComposeApplication = ix.writeProcessComposeApplication pkgs {
    name = "process-compose-fixture";
    processes.hello.command = "true";
  };

  zigAppFixture = fs.toSource {
    root = ./fixtures/zig-app;
    fileset = fs.unions [
      ./fixtures/zig-app/build.zig
      ./fixtures/zig-app/build.zig.zon
      ./fixtures/zig-app/src
    ];
  };

  zigApplication = ix.buildZigPackage pkgs {
    pname = "zig-app-fixture";
    version = "0.1.0";
    src = zigAppFixture;
    zig = ix.languages.zig.toolchain pkgs { version = "0.14"; };
    testSteps = {
      lib = "test-lib";
      exe = "test-exe";
    };
  };

  zigDepsFixture = fs.toSource {
    root = ./fixtures/zig-deps;
    fileset = fs.unions [
      ./fixtures/zig-deps/build.zig
      ./fixtures/zig-deps/build.zig.zon
      ./fixtures/zig-deps/src
    ];
  };

  zigDepsApplication = ix.buildZigPackage pkgs {
    pname = "zig-deps-fixture";
    version = "0.1.0";
    src = zigDepsFixture;
    zig = ix.languages.zig.toolchain pkgs { version = "0.14"; };
    zigDepsHash = "sha256-2eURmY4iF5iG5CdYiI7cKbrT3ymqb9UFUxO22LmsZ9s=";
  };

  cargoUnitFixture = fs.toSource {
    root = ./fixtures/cargo-unit-hello;
    fileset = fs.unions [
      ./fixtures/cargo-unit-hello/benches
      ./fixtures/cargo-unit-hello/build.rs
      ./fixtures/cargo-unit-hello/Cargo.lock
      ./fixtures/cargo-unit-hello/Cargo.toml
      ./fixtures/cargo-unit-hello/src
    ];
  };

  cargoUnitWorkspace = ix.cargoUnit.buildWorkspace {
    src = cargoUnitFixture;
    workspaceRoot = ./fixtures/cargo-unit-hello;
    cargoTargetNames = [
      "build"
      "test"
      "bench"
    ];
    cargoTargets = [
      [ "--workspace" ]
      [
        "--workspace"
        "--tests"
      ]
      [
        "--workspace"
        "--benches"
      ]
    ];
  };

  cargoUnitCoverageRustToolchain = ix.languages.rust.toolchain pkgs {
    channel = "nightly";
    version = rustPinnedNightlyDate;
    components = [
      "cargo"
      "llvm-tools"
      "rust-std"
      "rustc"
    ];
  };

  cargoUnitCoverageWorkspace = ix.cargoUnit.buildWorkspace {
    pname = "cargo-unit-hello-coverage";
    src = cargoUnitFixture;
    workspaceRoot = ./fixtures/cargo-unit-hello;
    rustToolchain = cargoUnitCoverageRustToolchain;
    cargoArgs = [
      "--workspace"
      "--tests"
    ];
    profile = "dev";
    extraRustcArgs = [ "-Cinstrument-coverage" ];
    policy = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      clippy.enable = false;
    };
  };

  cargoUnitHello = cargoUnitWorkspace.binaries.cargo-unit-hello;
  cargoUnitSelectedHello = ix.cargoUnit.selectBinaryWithTests cargoUnitWorkspace {
    binary = "cargo-unit-hello";
    packageName = "cargo-unit-hello";
  };
  cargoUnitTangoComparison = cargoUnitWorkspace.compareTangoBenchmarks {
    baseline = cargoUnitWorkspace;
    args = [
      "--time"
      "0.01"
      "--fail-threshold"
      "100000"
    ];
  };

  cargoUnitBinaries = {
    inherit (cargoUnitWorkspace.targetSets.build.binaries)
      cargo-unit-goodbye
      cargo-unit-hello
      ;
  };

  cargoUnitPolicyDisabledWorkspace = ix.cargoUnit.buildWorkspace {
    src = cargoUnitFixture;
    workspaceRoot = ./fixtures/cargo-unit-hello;
    policy = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      clippy.enable = false;
    };
  };

  goUnitFixture = fs.toSource {
    root = ./fixtures/go-unit-hello;
    fileset = fs.unions [
      ./fixtures/go-unit-hello/go.mod
      ./fixtures/go-unit-hello/go-modules.nix
      ./fixtures/go-unit-hello/go.sum
      ./fixtures/go-unit-hello/main.go
      ./fixtures/go-unit-hello/main_test.go
    ];
  };

  goUnitWorkspace = ix.goUnit.buildWorkspace {
    pname = "go-unit-hello";
    src = goUnitFixture;
    env.GOFLAGS = "-mod=readonly";
    packages = [ "." ];
  };

  goUnitNestedFixture = fs.toSource {
    root = ./fixtures/go-unit-nested;
    fileset = ./fixtures/go-unit-nested/module;
  };

  goUnitNestedWorkspace = ix.goUnit.buildWorkspace {
    pname = "go-unit-nested";
    src = goUnitNestedFixture;
    modRoot = "module";
    packages = [ "." ];
  };

  goUnitStdlibFixture = fs.toSource {
    root = ./fixtures/go-unit-stdlib;
    fileset = fs.unions [
      ./fixtures/go-unit-stdlib/go.mod
      ./fixtures/go-unit-stdlib/main.go
      ./fixtures/go-unit-stdlib/main_test.go
    ];
  };
  goUnitMissingGoSumFixture = fs.toSource {
    root = ./fixtures/go-unit-hello;
    fileset = fs.unions [
      ./fixtures/go-unit-hello/go.mod
      ./fixtures/go-unit-hello/main.go
      ./fixtures/go-unit-hello/main_test.go
    ];
  };
  goUnitRequireNoSpaceFixture = fs.toSource {
    root = ./fixtures/go-unit-require-nospace;
    fileset = fs.unions [
      ./fixtures/go-unit-require-nospace/go.mod
      ./fixtures/go-unit-require-nospace/main.go
    ];
  };

  goUnitStdlibWorkspace = ix.goUnit.buildWorkspace {
    pname = "go-unit-stdlib";
    src = goUnitStdlibFixture;
    vendorHash = null;
    packages = [ "." ];
  };

  goUnitDerivedStdlibSource = pkgs.runCommand "go-unit-stdlib-source" { } ''
    cp -R ${goUnitStdlibFixture}/. "$out"
  '';

  goUnitDerivedStdlibWorkspace = ix.goUnit.buildWorkspace {
    pname = "go-unit-stdlib-derived";
    src = goUnitDerivedStdlibSource;
    goMod = ./fixtures/go-unit-stdlib/go.mod;
    vendorHash = null;
    packages = [ "." ];
  };
  goUnitDerivedSource = pkgs.runCommand "go-unit-hello-source" { } ''
    cp -R ${goUnitFixture}/. "$out"
  '';
  goUnitDerivedWorkspaceWithVendorHashFile = ix.goUnit.buildWorkspace {
    pname = "go-unit-hello-derived";
    src = goUnitDerivedSource;
    goMod = ./fixtures/go-unit-hello/go.mod;
    goSum = ./fixtures/go-unit-hello/go.sum;
    vendorHashFile = ./fixtures/go-unit-hello/go-modules.nix;
    packages = [ "." ];
  };
  goUnitDerivedUnreadableNoSumEval = builtins.tryEval (
    builtins.attrNames
      (ix.goUnit.buildWorkspace {
        pname = "go-unit-hello-derived-no-sum";
        src = goUnitDerivedSource;
        vendorHash = null;
        packages = [ "." ];
      }).packages
  );
  goUnitDerivedMissingGoSumKeyEval =
    let
      workspace = ix.goUnit.buildWorkspace {
        pname = "go-unit-hello-derived-missing-go-sum";
        src = goUnitDerivedSource;
        goMod = ./fixtures/go-unit-hello/go.mod;
        vendorHashFile = ./fixtures/go-unit-hello/go-modules.nix;
        packages = [ "." ];
      };
    in
    builtins.tryEval workspace.default.drvPath;
  goUnitMissingGoModFixture = fs.toSource {
    root = ./fixtures/go-unit-hello;
    fileset = ./fixtures/go-unit-hello/main.go;
  };
  goUnitMissingGoModEval =
    builtins.tryEval
      (ix.goUnit.buildWorkspace {
        pname = "go-unit-missing-go-mod";
        src = goUnitMissingGoModFixture;
        vendorHash = null;
        packages = [ "." ];
      }).vendorHashKey;
  goUnitMissingGoModPackagesEval = builtins.tryEval (
    builtins.attrNames
      (ix.goUnit.buildWorkspace {
        pname = "go-unit-missing-go-mod";
        src = goUnitMissingGoModFixture;
        vendorHash = null;
        packages = [ "." ];
      }).packages
  );
  goUnitMissingGoSumEval = builtins.tryEval (
    builtins.attrNames
      (ix.goUnit.buildWorkspace {
        pname = "go-unit-missing-go-sum";
        src = goUnitMissingGoSumFixture;
        vendorHash = "sha256-36P4vOdzJotmVZon5Zud/d/jxzv4ad04aQT2G/EE3U8=";
        packages = [ "." ];
      }).packages
  );
  goUnitMissingGoSumNoSumEval = builtins.tryEval (
    builtins.attrNames
      (ix.goUnit.buildWorkspace {
        pname = "go-unit-missing-go-sum-no-sum";
        src = goUnitMissingGoSumFixture;
        vendorHash = null;
        packages = [ "." ];
      }).packages
  );
  goUnitRequireNoSpaceNoSumEval = builtins.tryEval (
    builtins.attrNames
      (ix.goUnit.buildWorkspace {
        pname = "go-unit-require-nospace-no-sum";
        src = goUnitRequireNoSpaceFixture;
        vendorHash = null;
        packages = [ "." ];
      }).packages
  );
  goUnitMissingExplicitGoSumEval = builtins.tryEval (
    builtins.attrNames
      (ix.goUnit.buildWorkspace {
        pname = "go-unit-missing-explicit-go-sum";
        src = goUnitMissingGoSumFixture;
        goSum = goUnitMissingGoSumFixture + "/go.sum";
        vendorHash = "sha256-36P4vOdzJotmVZon5Zud/d/jxzv4ad04aQT2G/EE3U8=";
        packages = [ "." ];
      }).packages
  );

  goUnitPackageCollisionEval =
    builtins.tryEval
      (ix.goUnit.buildWorkspace {
        pname = "go-unit-collision";
        src = goUnitFixture;
        packages = [
          "a.b"
          "a/b"
        ];
      }).packages;

  cargoUnitScopePolicy = {
    denyUnusedCrateDependencies = false;
    cargoAudit.enable = false;
    cargoMachete.enable = false;
    clippy.enable = false;
  };

  cargoUnitScopeFixture = fs.toSource {
    root = ./fixtures/cargo-unit-workspace-scope;
    fileset = fs.unions [
      ./fixtures/cargo-unit-workspace-scope/Cargo.lock
      ./fixtures/cargo-unit-workspace-scope/Cargo.toml
      ./fixtures/cargo-unit-workspace-scope/crates
    ];
  };

  cargoUnitScopeAlphaChangedFixture = fs.toSource {
    root = ./fixtures/cargo-unit-workspace-scope-alpha-changed;
    fileset = fs.unions [
      ./fixtures/cargo-unit-workspace-scope-alpha-changed/Cargo.lock
      ./fixtures/cargo-unit-workspace-scope-alpha-changed/Cargo.toml
      ./fixtures/cargo-unit-workspace-scope-alpha-changed/crates
    ];
  };

  cargoUnitScopeLockChangedFixture = pkgs.runCommand "cargo-unit-workspace-scope-lock-changed" { } ''
    cp -R ${cargoUnitScopeFixture}/. "$out"
    chmod -R u+w "$out"
    cp ${./fixtures/cargo-unit-workspace-scope/Cargo.itoa-1.0.14.lock} "$out/Cargo.lock"
  '';

  cargoUnitScopeWorkspace =
    {
      name,
      src,
      workspaceRoot ? ./fixtures/cargo-unit-workspace-scope,
    }:
    ix.cargoUnit.buildWorkspace {
      pname = "cargo-unit-workspace-scope-${name}";
      inherit src;
      inherit workspaceRoot;
      cargoArgs = [ "--workspace" ];
      policy = cargoUnitScopePolicy;
    };

  cargoUnitScopeWorkspaces = {
    base = cargoUnitScopeWorkspace {
      name = "base";
      src = cargoUnitScopeFixture;
    };
    alphaChanged = cargoUnitScopeWorkspace {
      name = "alpha-changed";
      src = cargoUnitScopeAlphaChangedFixture;
      workspaceRoot = ./fixtures/cargo-unit-workspace-scope-alpha-changed;
    };
    lockChanged = cargoUnitScopeWorkspace {
      name = "lock-changed";
      src = cargoUnitScopeLockChangedFixture;
    };
  };

  cargoUnitScopeUnit =
    workspace: prefix:
    let
      matches = lib.filterAttrs (name: _: lib.hasPrefix prefix name) workspace.units;
      names = builtins.attrNames matches;
    in
    assert lib.assertMsg (builtins.length names == 1)
      "expected exactly one cargo-unit unit with prefix ${prefix}, found ${lib.concatStringsSep ", " names}";
    matches.${builtins.head names};

  cargoUnitScope = {
    base = {
      alpha = cargoUnitScopeUnit cargoUnitScopeWorkspaces.base "scope_alpha-0.1.0-";
      bravo = cargoUnitScopeUnit cargoUnitScopeWorkspaces.base "scope_bravo-0.1.0-";
      itoa = cargoUnitScopeUnit cargoUnitScopeWorkspaces.base "itoa-1.0.18-";
      ryu = cargoUnitScopeUnit cargoUnitScopeWorkspaces.base "ryu-1.0.23-";
    };
    alphaChanged = {
      alpha = cargoUnitScopeUnit cargoUnitScopeWorkspaces.alphaChanged "scope_alpha-0.1.0-";
      bravo = cargoUnitScopeUnit cargoUnitScopeWorkspaces.alphaChanged "scope_bravo-0.1.0-";
      itoa = cargoUnitScopeUnit cargoUnitScopeWorkspaces.alphaChanged "itoa-1.0.18-";
      ryu = cargoUnitScopeUnit cargoUnitScopeWorkspaces.alphaChanged "ryu-1.0.23-";
    };
    lockChanged = {
      itoa = cargoUnitScopeUnit cargoUnitScopeWorkspaces.lockChanged "itoa-1.0.14-";
      ryu = cargoUnitScopeUnit cargoUnitScopeWorkspaces.lockChanged "ryu-1.0.23-";
    };
  };

  cargoUnitRealWorkspacePolicy = {
    denyUnusedCrateDependencies = false;
    cargoAudit.enable = false;
    cargoMachete.enable = false;
    clippy.enable = false;
  };

  cargoUnitRealWorkspaceSource =
    {
      name,
      upstream,
      lockFile,
    }:
    pkgs.runCommand "cargo-unit-${name}-source-with-lock" { } ''
      cp -R ${upstream}/. "$out"
      chmod -R u+w "$out"
      cp ${lockFile} "$out/Cargo.lock"
    '';

  cargoUnitRealWorkspace =
    {
      name,
      owner,
      repo,
      rev,
      hash,
      lockFile,
      buildArgs ? [ "--workspace" ],
      testArgs ? null,
    }:
    let
      upstream = pkgs.fetchFromGitHub {
        inherit
          owner
          repo
          rev
          hash
          ;
      };
      src = cargoUnitRealWorkspaceSource {
        inherit name upstream lockFile;
      };
      commonArgs = {
        pname = "cargo-unit-real-workspace-${name}";
        inherit src;
        cargoLock = lockFile;
        workspaceRoot = src;
        policy = cargoUnitRealWorkspacePolicy;
      };
      buildWorkspace = ix.cargoUnit.buildWorkspace (commonArgs // { cargoArgs = buildArgs; });
      testWorkspace =
        if testArgs == null then
          null
        else
          ix.cargoUnit.buildWorkspace (
            commonArgs
            // {
              pname = "cargo-unit-real-workspace-${name}-tests";
              cargoArgs = testArgs;
            }
          );
    in
    {
      inherit buildWorkspace testWorkspace;
      buildRoots = pkgs.linkFarmFromDrvs "cargo-unit-real-workspace-${name}-roots" buildWorkspace.roots;
      testRoots =
        if testWorkspace == null then
          null
        else
          pkgs.linkFarmFromDrvs "cargo-unit-real-workspace-${name}-tests" (
            # `tests.<binary>` is now `{ all; cases; }` after the per-#[test]
            # split (854b662); `.all` keeps the link-farm at one entry per
            # test binary, the same shape this script expects.
            map (entry: entry.all) (builtins.attrValues testWorkspace.tests)
          );
    };

  # These upstream workspaces currently do not commit Cargo.lock. The fixture
  # locks make the check exercise the same frozen/offline path as downstream
  # Nix packaging without vendoring forked source trees into this repo.
  cargoUnitRealWorkspaces = {
    serde = cargoUnitRealWorkspace {
      name = "serde";
      owner = "serde-rs";
      repo = "serde";
      rev = "fa7da4a93567ed347ad0735c28e439fca688ef26";
      hash = "sha256-5Ercr2dCC52VLV9dAZUsMlw+Ovup5Qui6vDQHxl70v4=";
      lockFile = ./fixtures/cargo-unit-real-workspaces/serde/Cargo.lock;
    };

    thiserror = cargoUnitRealWorkspace {
      name = "thiserror";
      owner = "dtolnay";
      repo = "thiserror";
      rev = "d4a2507576d276dbebc4be45c9b3d657216b727f";
      hash = "sha256-0DU1KSWZ+T4v9cfTfY8QQ2bMLgko9+c1dOXEk99KvUo=";
      lockFile = ./fixtures/cargo-unit-real-workspaces/thiserror/Cargo.lock;
    };

    indexmap = cargoUnitRealWorkspace {
      name = "indexmap";
      owner = "indexmap-rs";
      repo = "indexmap";
      rev = "0a5535021aec77a2c9890c0bec273fa446c6593a";
      hash = "sha256-7WBUZ1QJ6tywpdmo50QpX01fu7HMkpfoh/TC2LkPxiM=";
      lockFile = ./fixtures/cargo-unit-real-workspaces/indexmap/Cargo.lock;
      testArgs = [
        "--workspace"
        "--tests"
      ];
    };

    regex = cargoUnitRealWorkspace {
      name = "regex";
      owner = "rust-lang";
      repo = "regex";
      rev = "839d16bc65b60e2006d3599d20bfa6efc14049d8";
      hash = "sha256-9czj9Oa25H8VhMmZNyS0h9sFn6rYDrEPlOuGm9NJd9A=";
      lockFile = ./fixtures/cargo-unit-real-workspaces/regex/Cargo.lock;
      testArgs = [
        "--workspace"
        "--tests"
      ];
    };
  };

  bunSiteFixture = fs.toSource {
    root = ./fixtures/bun-site;
    fileset = fs.unions [
      ./fixtures/bun-site/bin
      ./fixtures/bun-site/bun.lock
      ./fixtures/bun-site/package.json
    ];
  };

  bunSite = ix.buildBunSite pkgs {
    pname = "bun-site-fixture";
    version = "0.1.0";
    src = bunSiteFixture;
    buildFlags = [
      "--class"
      "ix bun"
    ];
  };

  bunLockPackage = builtins.head bunSite.bunNodeModules.bunCache.lock.packages;

  npmSiteFixture = fs.toSource {
    root = ./fixtures/npm-site;
    fileset = fs.unions [
      ./fixtures/npm-site/bin
      ./fixtures/npm-site/package-lock.json
      ./fixtures/npm-site/package.json
    ];
  };

  npmSite = ix.buildNpmSite pkgs {
    pname = "npm-site-fixture";
    version = "0.1.0";
    src = npmSiteFixture;
    buildFlags = [
      "--class"
      "ix npm"
    ];
  };

  vitestWorkspaceFixture = fs.toSource {
    root = ./fixtures/vitest-workspace;
    fileset = fs.unions [
      ./fixtures/vitest-workspace/package-lock.json
      ./fixtures/vitest-workspace/package.json
      ./fixtures/vitest-workspace/src
      ./fixtures/vitest-workspace/vitest.config.js
    ];
  };

  vitestWorkspace = ix.buildNpmVitest pkgs {
    pname = "vitest-workspace-fixture";
    version = "0.1.0";
    src = vitestWorkspaceFixture;
  };
  vitestWorkspaceCases = builtins.attrValues vitestWorkspace.cases;

  svelteSite = ix.buildSvelteSite pkgs {
    pname = "svelte-site-fixture";
    version = "0.1.0";
    src = npmSiteFixture;
    buildFlags = [
      "--class"
      "ix svelte"
    ];
    serve = {
      name = "svelte-site-fixture";
      port = 8180;
      routePrefix = "/fixture";
      extraFlags = [
        "--title"
        "Svelte Site Fixture"
      ];
    };
    devServer = {
      name = "svelte-site-fixture-dev";
      checkoutSubdir = "tests/fixtures/npm-site";
      script = "build";
      port = 5177;
    };
  };

  uvAppFixture = fs.toSource {
    root = ./fixtures/uv-app;
    fileset = fs.unions [
      ./fixtures/uv-app/pyproject.toml
      ./fixtures/uv-app/src
      ./fixtures/uv-app/uv.lock
    ];
  };

  uvApplication = ix.buildUvApplication pkgs {
    pname = "uv-app-fixture";
    version = "0.1.0";
    src = uvAppFixture;
  };

  uvLockedDistribution = builtins.head uvApplication.uvWheelhouse.lock.distributions;
  uvWheelhouseDistributionNames = map (
    distribution: distribution.fileName
  ) uvApplication.uvWheelhouse.distributions;

  mcpPackage = (ix.packageSetFor pkgs).mcp;

  fleet = ix.mkFleet {
    deployment.region = "us-west-1";
    secrets = {
      provider = {
        type = "vaultwarden";
        mountRoot = "/run/secrets/fleet";
        collection = "production";
      };
      sessionKey = {
        key = "web/session-key";
        generate = true;
      };
    };

    nodes = {
      db = {
        services.ix-postgresql.enable = true;
      };

      web = {
        tags = [ "public" ];
        groups = [ "public-apps" ];
        deployment = {
          destination = "fleet-web:latest";
          ipv4 = true;
        };
        modules = [
          (
            { nodes, secretRefs, ... }:
            {
              services.remote-desktop.enable = true;
              environment.etc."db-host".text = nodes.db.config.networking.hostName;
              environment.etc."session-key-ref".text = secretRefs.sessionKey;
            }
          )
        ];
      };

      worker = {
        replicas = 2;
        dependsOn = [ "db" ];
        modules = [
          {
            services.remote-desktop.enable = true;
          }
        ];
      };
    };
  };

  fleetPlan = fleet.planValue.nodes;

  prefixedFleet = ix.mkFleet {
    nodePrefix = "tprefix-";
    nodes = {
      api = {
        services.openssh.enable = true;
      };
      worker = {
        dependsOn = [ "api" ];
        groups = [ "private-apps" ];
        modules = [
          (
            { nodes, ... }:
            {
              environment.etc."api-host".text = nodes.api.config.networking.hostName;
            }
          )
        ];
      };
    };
  };

  fleetIpv4HealthCheckEval = builtins.tryEval (
    builtins.deepSeq
      (ix.mkFleet {
        nodes.private.modules = [
          {
            ix.healthChecks."public-reachability" = {
              from = "host";
              requiresIpv4 = true;
              command = [ "true" ];
            };
          }
        ];
      }).planValue.nodes.private.healthChecks."public-reachability"
      true
  );

  fleetUnknownDependencyEval = builtins.tryEval (
    builtins.deepSeq
      (ix.mkFleet {
        nodes.web = {
          dependsOn = [ "db" ];
          modules = [
            {
              services.remote-desktop.enable = true;
            }
          ];
        };
      }).planValue.nodes.web.dependsOn
      true
  );

  fleetDependencyCycleEval = builtins.tryEval (
    builtins.deepSeq
      (ix.mkFleet {
        nodes = {
          api = {
            dependsOn = [ "worker" ];
            modules = [ { } ];
          };

          worker = {
            dependsOn = [ "api" ];
            modules = [ { } ];
          };
        };
      }).planValue.nodes
      true
  );

  factionsExample =
    let
      fleet = import ../examples/minecraft/factions {
        index = {
          lib = ix;
        };
      };
      config = fleet.nodes.factions;
      service = config.systemd.services.minecraft-world-border;
    in
    {
      inherit fleet config service;
      cfg = config.services.minecraft;
      managed = {
        config = config.environment.etc."minecraft/managed-config".source;
        datapacks = config.environment.etc."minecraft/managed-datapacks".source;
        dropins = config.environment.etc."minecraft/managed-dropins".source;
        serverFiles = config.environment.etc."minecraft/managed-server-files".source;
      };
    };

  survivalExample =
    let
      fleet = import ../examples/minecraft/survival {
        index = {
          lib = ix;
        };
      };
      config = fleet.nodes.survival;
    in
    {
      inherit fleet config;
      inherit (config.services)
        floodgate
        geyser
        minecraft
        velocity
        ;
      managed = {
        minecraftConfig = config.environment.etc."minecraft/managed-config".source;
        minecraftServerFiles = config.environment.etc."minecraft/managed-server-files".source;
        velocityConfig = config.environment.etc."velocity/managed-config".source;
        velocityPlugins = config.environment.etc."velocity/managed-plugins".source;
      };
    };

  dailyScraperExample =
    let
      fleet = import ../examples/python-daily-scraper {
        index = {
          lib = ix;
        };
      };
      config = fleet.nodes.scraper;
    in
    {
      inherit fleet config;
      plan = fleet.planValue.nodes.scraper;
      service = config.systemd.services.daily-scraper;
      timer = config.systemd.timers.daily-scraper;
    };

  nginxLifecycleExample =
    let
      fleet = import ../examples/nginx-lifecycle {
        index = {
          lib = ix;
        };
      };
      config = fleet.nodes.nginx;
    in
    {
      inherit fleet config;
      cfg = config.services.nginx;
      plan = fleet.planValue.nodes.nginx;
    };

  observabilityStackExample =
    let
      fleet = import ../examples/observability-stack {
        index = {
          lib = ix;
        };
      };
      queryTool =
        config:
        lib.findFirst (
          package: (package.meta.mainProgram or null) == "ix-observe"
        ) null config.environment.systemPackages;
    in
    {
      inherit fleet;
      observability =
        let
          config = fleet.nodes.observability;
        in
        {
          inherit config;
          cfg = config.services.ix-observability;
          collector = config.services.opentelemetry-collector.settings;
          grafana = config.services.grafana;
          plan = fleet.planValue.nodes.observability;
          queryTool = queryTool config;
          dashboardPath =
            (builtins.elemAt config.services.grafana.provision.dashboards.settings.providers 0).options.path;
        };
      app =
        let
          config = fleet.nodes.app;
        in
        {
          inherit config;
          cfg = config.services.ix-observability;
          collector = config.services.opentelemetry-collector.settings;
          plan = fleet.planValue.nodes.app;
        };
    };

  dailyScraperS3 =
    let
      config = evalConfig [
        ../examples/python-daily-scraper/service.nix
        {
          _module.args.dailyScraper = {
            s3 = {
              uri = "s3://andrew-scraper-output/github";
              deleteRemoved = true;
              awsEnvironmentFile = "/run/secrets/daily-scraper/aws.env";
            };
          };
        }
      ];
    in
    {
      inherit config;
      service = config.systemd.services.daily-scraper;
    };

  extendedAttributes =
    let
      config = evalConfig [
        {
          ix.extendedAttributes."/build/ix-xattr-test" = {
            create = true;
            attributes = {
              "user.ix.kind" = "test.path";
              "user.ix.owner" = "ix";
            };
          };
        }
      ];
    in
    {
      inherit config;
      activationScript = config.system.activationScripts.ix-extended-attributes.text;
    };

  portClaimConflictFailures = failedAssertionsFor [
    {
      services.remote-desktop = {
        enable = true;
        port = 6080;
      };

      services.resource-monitor = {
        enable = true;
        port = 6080;
      };
    }
  ];

  remoteDesktopUnauthenticatedFirewallFailures = failedAssertionsFor [
    {
      services.remote-desktop = {
        enable = true;
        openFirewall = true;
      };
    }
  ];

  remoteDesktopSettingsAuthFirewallFailures = failedAssertionsFor [
    {
      services.remote-desktop = {
        enable = true;
        openFirewall = true;
        auth = "file";
        settings.auth = "none";
      };
    }
  ];

  remoteDesktopBindTcpDriftFailures = failedAssertionsFor [
    {
      services.remote-desktop = {
        enable = true;
        bindAddress = "0.0.0.0";
        port = 6080;
        settings.bind-tcp = "0.0.0.0:6081";
      };
    }
  ];

  resourceMonitorRuntimeDirectoryFailures =
    let
      failuresFor =
        runtimeDirectory:
        failedAssertionsFor [
          {
            services.resource-monitor = {
              enable = true;
              inherit runtimeDirectory;
            };
          }
        ];
    in
    map failuresFor [
      "/var/lib/resource-monitor"
      "/run//resource-monitor"
      "/run/resource-monitor/."
      "/run/resource-monitor/../stats"
    ];

  minecraftUnsafeManagedPathFailures = failedAssertionsFor [
    ../images/games/minecraft
    defaultMinecraftModule
    {
      services.minecraft = {
        configFiles."client//bad.toml" = { };
        configFiles."/absolute/bad.toml" = { };
        properties.level-name = "../bad-world";
        serverFiles."plugins/../bukkit.yml" = { };
        serverFiles."$(bad).json" = { };
        datapacks.bad = {
          fileName = "../bad";
          files."data/../bad.json" = { };
        };
      };
    }
  ];

  velocityUnsafeManagedPathFailures = failedAssertionsFor [
    {
      services.velocity = {
        enable = true;
        configFiles."plugins/../bad.toml" = { };
        plugins.bad = {
          src = pkgs.writeText "velocity-test-plugin.jar" "";
          fileName = "nested/bad.jar";
        };
      };
    }
  ];

  velocityDuplicatePluginFileNameFailures = failedAssertionsFor [
    {
      services.velocity = {
        enable = true;
        plugins = {
          first = {
            src = pkgs.writeText "velocity-test-first-plugin.jar" "";
            fileName = "shared.jar";
          };

          second = {
            src = pkgs.writeText "velocity-test-second-plugin.jar" "";
            fileName = "shared.jar";
          };
        };
      };
    }
  ];
  velocityConcreteAddress = evalConfig [
    {
      services.velocity = {
        enable = true;
        address = "10.0.0.5";
        port = 25570;
        openFirewall = false;
      };
    }
  ];

  relativePathUnsafeShellEval = builtins.tryEval (
    builtins.deepSeq (ix.relativePath.shellPath "$out" "../bad") true
  );

  portClaimNamespaceAllowedFailures = failedAssertionsFor [
    {
      ix.networking.portClaims = {
        left = {
          protocol = "tcp";
          port = 1234;
          namespace = "left-netns";
        };

        right = {
          protocol = "tcp";
          port = 1234;
          namespace = "right-netns";
        };
      };
    }
  ];

  portClaimAddressFamilyAllowedFailures = failedAssertionsFor [
    {
      services.minecraft-bedrock = {
        enable = true;
        port = 19132;
        portv6 = 19132;
      };
    }
  ];

  base =
    let
      config = evalConfig [ ];
      imageConfig = evalConfig [
        {
          ix.image = {
            name = "ix/base";
            tag = "latest";
          };
        }
      ];
    in
    {
      inherit config imageConfig;
      cfg = config.ix.profiles.base;
    };

  # --- Language helpers -----------------------------------------------------

  languages = {
    pythonMissingVersion = builtins.tryEval (
      builtins.deepSeq (ix.languages.python.interpreter pkgs { }).pythonVersion true
    );
    pythonUnknown = builtins.tryEval (
      builtins.deepSeq (ix.languages.python.interpreter pkgs { version = "3.99"; }).pythonVersion true
    );

    rustMissingVersion = builtins.tryEval (
      builtins.deepSeq (ix.languages.rust.toolchain pkgs { channel = "nightly"; }).name true
    );
    rustPinnedNightly = ix.languages.rust.toolchain pkgs {
      channel = "nightly";
      version = rustPinnedNightlyDate;
    };
    rustExtraComponents = ix.languages.rust.toolchain pkgs {
      channel = "nightly";
      version = rustPinnedNightlyDate;
      components = [
        "cargo"
        "rust-std"
        "rustc"
        "rust-src"
        "rustfmt"
      ];
    };
    rustBadChannel = builtins.tryEval (
      builtins.deepSeq (ix.languages.rust.toolchain pkgs { channel = "nighty"; }).name true
    );
    rustBadProfile = builtins.tryEval (
      builtins.deepSeq (ix.languages.rust.toolchain pkgs { profile = "extreme"; }).name true
    );

    javaMissingDistribution = builtins.tryEval (
      builtins.deepSeq (ix.languages.java.jdk pkgs { version = "21"; }).name true
    );
    javaBadDistribution = builtins.tryEval (
      builtins.deepSeq
        (ix.languages.java.jdk pkgs {
          version = "21";
          distribution = "openjdkk";
        }).name
        true
    );
    javaBadVersion = builtins.tryEval (
      builtins.deepSeq
        (ix.languages.java.jdk pkgs {
          version = "22";
          distribution = "temurin";
        }).name
        true
    );
  };

  # --- Minestom + YourKit wiring -------------------------------------------

  minestomYourkit =
    let
      yourkitConfig = evalConfig [
        {
          services.minestom = {
            enable = true;
            serverJar = pkgs.runCommand "fake-minestom.jar" { } "touch $out";
            yourkit = {
              enable = true;
              listen = "all";
              openFirewall = true;
              sessionName = "minestom-eval-test";
            };
          };
        }
      ];
      unit = yourkitConfig.systemd.services.minestom;
    in
    {
      inherit yourkitConfig;
      execStart = unit.serviceConfig.ExecStart;
      firewallTcpPorts = yourkitConfig.networking.firewall.allowedTCPPorts;
      portClaim = yourkitConfig.ix.networking.portClaims.minestom-yourkit or null;
    };

  minestomNoYourkit =
    let
      noYourkitConfig = evalConfig [
        {
          services.minestom = {
            enable = true;
            serverJar = pkgs.runCommand "fake-minestom.jar" { } "touch $out";
          };
        }
      ];
      unit = noYourkitConfig.systemd.services.minestom;
    in
    {
      inherit noYourkitConfig;
      execStart = unit.serviceConfig.ExecStart;
      portClaim = noYourkitConfig.ix.networking.portClaims.minestom-yourkit or null;
    };

  nomadSecretRefsExample = import ../examples/nomad-secret-refs/example.nix {
    index = {
      lib = ix;
    };
  };
  invalidSecretNameEval = builtins.tryEval (
    builtins.deepSeq
      (ix.secrets.normalize {
        provider.type = "vaultwarden";
        values."../aws.env".key = "daily-scraper/aws-env";
      }).refs
      true
  );
  # --- Per-image assertion groups -------------------------------------------

  groups = {
    base = [
      {
        assertion = base.cfg.shellWorkspace.enable;
        message = "base profile should enable the ix shell workspace by default";
      }
      {
        assertion = base.config.users.users.root.shell.meta.mainProgram == "nu";
        message = "base profile should make root land in nushell (via platform users.defaultUserShell)";
      }
      {
        assertion = lib.any (
          rule: lib.hasPrefix "d ${base.cfg.shellWorkspace.directory} " rule
        ) base.config.systemd.tmpfiles.rules;
        message = "base profile should pre-create the workspace directory via systemd-tmpfiles";
      }
      {
        assertion =
          let
            firewall = base.config.networking.firewall;
          in
          builtins.elem 5001 firewall.allowedTCPPorts && builtins.elem 8443 firewall.allowedUDPPorts;
        message = "base profile should expose ix guest sidecar ports through the in-guest firewall";
      }
      {
        assertion =
          let
            claims = base.config.ix.networking.portClaims;
          in
          claims.ix-console.protocol == "tcp"
          && claims.ix-console.port == 5001
          && claims.ix-console.address == "*"
          && claims.ix-agent.protocol == "udp"
          && claims.ix-agent.port == 8443
          && claims.ix-agent.address == "*";
        message = "base profile should reserve ix guest sidecar listener ports";
      }
    ];

    factions = [
      {
        assertion =
          factionsExample.cfg.worldBorder.enable
          && factionsExample.cfg.worldBorder.diameter == 12000
          && factionsExample.cfg.properties."max-world-size" == 6000;
        message = "factions example should declare a managed world border";
      }
      {
        assertion =
          let
            ports = factionsExample.config.networking.firewall.allowedTCPPorts;
          in
          builtins.elem factionsExample.cfg.port ports
          && builtins.elem 8100 ports
          && !(builtins.elem factionsExample.cfg.rcon.port ports);
        message = "factions example should keep RCON private while exposing Minecraft and BlueMap";
      }
      {
        assertion = builtins.elem 24454 factionsExample.config.networking.firewall.allowedUDPPorts;
        message = "factions example should expose Simple Voice Chat on the default UDP port";
      }
      {
        assertion =
          let
            claims = factionsExample.config.ix.networking.portClaims;
          in
          lib.all (claim: builtins.hasAttr claim claims) [
            "minecraft"
            "minecraft-rcon"
            "bluemap"
            "simple-voice-chat"
          ]
          && claims.simple-voice-chat.protocol == "udp"
          && claims.simple-voice-chat.port == 24454;
        message = "factions example should register every service listener in ix.networking.portClaims";
      }
      {
        assertion =
          let
            checks = factionsExample.fleet.planValue.nodes.factions.healthChecks;
            mcProbe = lib.getExe repoPackages.mc-probe;
            systemctl = lib.getExe' factionsExample.config.systemd.package "systemctl";
          in
          checks.minecraft.from == "guest"
          && checks.minecraft.attempts == 30
          &&
            checks.minecraft.command == [
              systemctl
              "is-active"
              "--quiet"
              "minecraft.service"
            ]
          # The SLP check is the interesting one: it proves the Minecraft
          # protocol speaker is up (not just the unit), and asserts the MOTD
          # so a misrouted image lands as a check failure instead of silently
          # serving Survival players a Factions world.
          && checks.minecraft-status.from == "guest"
          &&
            checks.minecraft-status.command == [
              mcProbe
              "127.0.0.1:25565"
              "--motd-contains"
              "ix Factions | territory, raids, shops"
            ]
          # factions exposes Java publicly, so the host-side reachability
          # probe is what catches firewall or routing regressions.
          && checks.minecraft-reachable.from == "host"
          &&
            checks.minecraft-reachable.command == [
              "nc"
              "-z"
              "-w"
              "5"
              "$IX_NODE_IPV4"
              "25565"
            ]
          && lib.any (
            package: lib.getName package == "mc-probe"
          ) factionsExample.config.environment.systemPackages;
        message = "factions should layer systemctl + SLP-with-MOTD + host TCP probes";
      }
    ];

    survival = [
      {
        assertion =
          survivalExample.velocity.enable
          && survivalExample.velocity.servers.survival == "127.0.0.1:25566"
          && survivalExample.velocity.try == [ "survival" ]
          && survivalExample.velocity.forwarding.mode == "modern";
        message = "survival example should route Velocity to the local Paper backend";
      }
      {
        assertion =
          survivalExample.geyser.enable
          && survivalExample.geyser.remote.authType == "floodgate"
          && survivalExample.floodgate.enable;
        message = "survival example should enable Geyser with Floodgate auth";
      }
      {
        assertion =
          survivalExample.minecraft.paper.enable
          && survivalExample.minecraft.version == "26.1.2"
          && survivalExample.minecraft.port == 25566
          && !survivalExample.minecraft.openFirewall
          && !survivalExample.minecraft.properties."online-mode";
        message = "survival example should keep Paper behind the proxy";
      }
      {
        assertion =
          let
            ports = survivalExample.config.networking.firewall.allowedTCPPorts;
          in
          builtins.elem 25565 ports
          && !(builtins.elem 25566 ports)
          && !(builtins.elem survivalExample.minecraft.rcon.port ports);
        message = "survival example should expose Velocity while keeping backend and RCON private";
      }
      {
        assertion = builtins.elem 19132 survivalExample.config.networking.firewall.allowedUDPPorts;
        message = "survival example should expose Geyser's Bedrock UDP listener";
      }
      {
        assertion =
          let
            claims = survivalExample.config.ix.networking.portClaims;
          in
          lib.all (claim: builtins.hasAttr claim claims) [
            "velocity"
            "minecraft"
            "minecraft-rcon"
            "geyser"
          ]
          && claims.velocity.port == 25565
          && claims.minecraft.port == 25566
          && claims.geyser.protocol == "udp"
          && claims.geyser.port == 19132;
        message = "survival example should register proxy, backend, RCON, and Bedrock listeners";
      }
      {
        assertion =
          let
            checks = survivalExample.fleet.planValue.nodes.survival.healthChecks;
            mcProbe = lib.getExe repoPackages.mc-probe;
          in
          checks.velocity.from == "guest"
          && checks.minecraft.from == "guest"
          # Velocity faces the public network in this topology, so it gets a
          # host TCP probe. The Paper backend stays openFirewall = false, so
          # its only host-observable signal is via Velocity itself.
          && checks.velocity-reachable.from == "host"
          &&
            checks.velocity-reachable.command == [
              "nc"
              "-z"
              "-w"
              "5"
              "$IX_NODE_IPV4"
              "25565"
            ]
          && !(checks ? minecraft-reachable)
          && lib.any (
            package: lib.getName package == "mc-probe"
          ) survivalExample.config.environment.systemPackages
          # SLP checks on both: Velocity proves the proxy answers; the
          # backend SLP proves the actual game server isn't dead behind a
          # healthy proxy.
          &&
            checks.velocity-status.command == [
              mcProbe
              "127.0.0.1:25565"
              "--motd-contains"
              "ix Survival"
            ]
          &&
            checks.minecraft-status.command == [
              mcProbe
              "127.0.0.1:25566"
              "--motd-contains"
              "ix Survival"
            ];
        message = "survival should expose layered guest/host probes with MOTD-aware SLP on both proxy and backend";
      }
    ];

    python-daily-scraper = [
      {
        assertion =
          builtins.any (
            package: (package.meta.mainProgram or null) == "daily-scraper"
          ) dailyScraperExample.config.environment.systemPackages
          && lib.hasInfix "--repo indexable-inc/index" dailyScraperExample.service.serviceConfig.ExecStart;
        message = "python-daily-scraper example should package and enable the scraper";
      }
      {
        assertion =
          dailyScraperExample.service.serviceConfig.Type == "oneshot"
          && dailyScraperExample.service.serviceConfig.DynamicUser
          && dailyScraperExample.service.serviceConfig.StateDirectory == "daily-scraper"
          && dailyScraperExample.service.serviceConfig.WorkingDirectory == "/var/lib/daily-scraper";
        message = "python-daily-scraper example should render a stateful oneshot systemd service";
      }
      {
        assertion =
          builtins.elem "network-online.target" dailyScraperExample.service.after
          && builtins.elem "network-online.target" dailyScraperExample.service.wants;
        message = "python-daily-scraper service should wait for network readiness";
      }
      {
        assertion =
          lib.hasInfix "/var/lib/daily-scraper/parquet" dailyScraperExample.service.serviceConfig.ExecStart
          && lib.hasInfix "--repo indexable-inc/index" dailyScraperExample.service.serviceConfig.ExecStart;
        message = "python-daily-scraper service should pass the durable output directory and repository";
      }
      {
        assertion =
          dailyScraperExample.timer.timerConfig.OnCalendar == "*-*-* 03:17:00 UTC"
          && dailyScraperExample.timer.timerConfig.Persistent
          && dailyScraperExample.timer.timerConfig.RandomizedDelaySec == "20m"
          && dailyScraperExample.timer.timerConfig.Unit == "daily-scraper.service";
        message = "python-daily-scraper example should run from a persistent daily timer";
      }
      {
        assertion =
          let
            check = dailyScraperExample.plan.healthChecks.daily-scraper;
          in
          check.from == "guest"
          &&
            check.command == [
              (lib.getExe' dailyScraperExample.config.systemd.package "systemctl")
              "is-active"
              "--quiet"
              "daily-scraper.timer"
            ];
        # No listener for the operator to probe, so the guest unit check is
        # the whole story. The explicit `from = "guest"` rules out a future
        # default-flip accidentally turning this into an unrunnable host check.
        message = "python-daily-scraper fleet plan should include a guest-side timer health check";
      }
      {
        assertion =
          !dailyScraperExample.plan.ipv4
          && dailyScraperExample.plan.snapshot
          && dailyScraperExample.plan.replacementImage.imageTag == "daily-scraper";
        message = "python-daily-scraper fleet plan should keep the worker private with snapshots on";
      }
      {
        assertion =
          lib.hasInfix "s3 sync --only-show-errors /var/lib/daily-scraper/parquet s3://andrew-scraper-output/github --delete" dailyScraperS3.service.serviceConfig.ExecStartPost
          &&
            dailyScraperS3.service.serviceConfig.LoadCredential == [
              "aws-env:/run/secrets/daily-scraper/aws.env"
            ]
          && dailyScraperS3.service.serviceConfig.EnvironmentFile == "%d/aws-env";
        message = "python-daily-scraper service should support S3 sync through systemd credentials";
      }
    ];

    nginx-lifecycle = [
      {
        assertion = nginxLifecycleExample.plan.recreateOnUp;
        message = "nginx-lifecycle fleet plan should recreate the VM on every ix-fleet up run";
      }
      {
        assertion =
          nginxLifecycleExample.cfg.enable
          &&
            nginxLifecycleExample.cfg.virtualHosts.localhost.locations."/".return
            == "200 'ix nginx lifecycle ok\n'";
        message = "nginx-lifecycle example should serve a fixed HTTP success body";
      }
      {
        assertion =
          let
            claims = nginxLifecycleExample.config.ix.networking.portClaims;
          in
          claims.nginx.protocol == "tcp"
          && claims.nginx.port == 80
          && builtins.elem 80 nginxLifecycleExample.config.networking.firewall.allowedTCPPorts;
        message = "nginx-lifecycle example should declare and open its HTTP listener";
      }
      {
        assertion =
          let
            checks = nginxLifecycleExample.plan.healthChecks;
          in
          checks.nginx.from == "guest"
          &&
            checks.nginx.command == [
              (lib.getExe' nginxLifecycleExample.config.systemd.package "systemctl")
              "is-active"
              "--quiet"
              "nginx.service"
            ]
          && checks.nginx-http.from == "guest"
          && lib.hasSuffix "/bin/curl" (builtins.head checks.nginx-http.command)
          &&
            builtins.tail checks.nginx-http.command == [
              "--fail"
              "--silent"
              "--show-error"
              "http://127.0.0.1/"
            ];
        message = "nginx-lifecycle fleet plan should prove the service unit and HTTP loopback path";
      }
    ];

    observability-stack = [
      {
        assertion =
          observabilityStackExample.observability.cfg.stack.enable
          && observabilityStackExample.observability.cfg.agent.enable
          && observabilityStackExample.observability.config.services.clickhouse.enable
          && observabilityStackExample.observability.config.services.grafana.enable
          && observabilityStackExample.observability.config.services.opentelemetry-collector.enable;
        message = "observability-stack should enable the full local observability stack";
      }
      {
        assertion =
          observabilityStackExample.observability.config.services.opentelemetry-collector.package.pname
          == "otelcol-contrib";
        message = "ix-observability should use the contrib collector so ClickHouse export is available";
      }
      {
        assertion =
          observabilityStackExample.observability.collector.receivers.otlp.protocols.grpc.endpoint
          == "0.0.0.0:4317"
          && observabilityStackExample.observability.collector.exporters.clickhouse.database == "otel"
          &&
            observabilityStackExample.observability.collector.exporters.clickhouse.traces_table_name
            == "otel_traces"
          &&
            observabilityStackExample.observability.collector.service.pipelines.logs.exporters
            == [ "clickhouse" ];
        message = "observability-stack collector should receive OTLP and export logs/traces/metrics to ClickHouse";
      }
      {
        assertion =
          let
            datasource = builtins.head observabilityStackExample.observability.grafana.provision.datasources.settings.datasources;
          in
          datasource.uid == "ix-clickhouse"
          && datasource.type == "grafana-clickhouse-datasource"
          && datasource.jsonData.traces.defaultTable == "otel_traces"
          && datasource.jsonData.logs.defaultTable == "otel_logs";
        message = "observability-stack should provision Grafana with the ClickHouse OTel datasource";
      }
      {
        assertion =
          observabilityStackExample.observability.plan.l7ProxyPorts == [ 3000 ]
          && builtins.elem 3000 observabilityStackExample.observability.config.networking.firewall.allowedTCPPorts
          && builtins.elem 4317 observabilityStackExample.observability.config.networking.firewall.allowedTCPPorts
          && builtins.elem 9000 observabilityStackExample.observability.config.networking.firewall.allowedTCPPorts;
        message = "observability-stack should expose Grafana, OTLP, and ClickHouse for the example fleet";
      }
      {
        assertion =
          observabilityStackExample.app.cfg.stack.enable == false
          && observabilityStackExample.app.cfg.agent.enable
          && observabilityStackExample.app.cfg.agent.endpoint == "observability:4317"
          && observabilityStackExample.app.collector.exporters.otlp.endpoint == "observability:4317"
          &&
            observabilityStackExample.app.collector.receivers."filelog/app".include
            == [ "/var/log/ix-observability-demo/app.log" ]
          && observabilityStackExample.app.collector.service.pipelines.logs.exporters == [ "otlp" ];
        message = "observability-stack app node should run an agent collector that forwards file logs and OTLP";
      }
      {
        assertion =
          let
            checks = observabilityStackExample.app.plan.healthChecks;
          in
          checks.observability-demo.from == "guest"
          && checks.observability-ingested.attempts == 60
          && checks.observability-ingested.timeoutSec == 10;
        message = "observability-stack app node should prove local emission and ClickHouse ingestion";
      }
      {
        assertion = observabilityStackExample.observability.queryTool != null;
        message = "observability-stack should install the ix-observe query helper for agents";
      }
    ];

    networking = [
      {
        assertion = lib.any (
          failure: lib.hasInfix "ix.networking.portClaims has same-namespace port collisions" failure.message
        ) portClaimConflictFailures;
        message = "ix.networking.portClaims should fail eval when two services claim the same-namespace socket";
      }
      {
        assertion = portClaimNamespaceAllowedFailures == [ ];
        message = "ix.networking.portClaims should allow the same port in separate network namespaces";
      }
      {
        assertion = portClaimAddressFamilyAllowedFailures == [ ];
        message = "ix.networking.portClaims should allow the same UDP port on separate IPv4 and IPv6 bind addresses";
      }
    ];

    managed-paths = [
      {
        assertion =
          ix.relativePath.isSafe "plugins/BlueMap/core.conf"
          && !(ix.relativePath.isSafe "../core.conf")
          && !(ix.relativePath.isSafe "plugins//core.conf")
          && ix.relativePath.isSafeName "Geyser-Velocity.jar"
          && !(ix.relativePath.isSafeName "nested/Geyser-Velocity.jar");
        message = "ix.relativePath should distinguish safe managed paths from unsafe segments and names";
      }
      {
        assertion =
          ix.relativePath.shellPath "$out" "plugins/Blue Map/core.conf"
          == "\"$out\"/'plugins/Blue Map/core.conf'"
          && ix.relativePath.shellParent "$out" "plugins/Blue Map/core.conf" == "\"$out\"/'plugins/Blue Map'"
          && ix.relativePath.shellParent "$out" "server.properties" == "\"$out\""
          && !relativePathUnsafeShellEval.success;
        message = "ix.relativePath shell helpers should quote safe relative paths and reject unsafe paths";
      }
      {
        assertion =
          let
            failure = lib.findFirst (
              f: lib.hasInfix "services.minecraft managed paths must be relative paths" f.message
            ) null minecraftUnsafeManagedPathFailures;
            msg = if failure != null then failure.message else "";
          in
          failure != null
          && lib.hasInfix "services.minecraft.configFiles.client//bad.toml" msg
          && lib.hasInfix "services.minecraft.configFiles./absolute/bad.toml" msg
          && lib.hasInfix "services.minecraft.serverFiles.plugins/../bukkit.yml" msg
          && lib.hasInfix "services.minecraft.serverFiles.$(bad).json" msg
          && lib.hasInfix "services.minecraft.datapacks.bad.fileName=../bad" msg
          && lib.hasInfix "services.minecraft.datapacks.bad.files.data/../bad.json" msg
          && lib.hasInfix "services.minecraft world directory ../bad-world" msg;
        message = "minecraft managed file options should reject unsafe relative paths at eval time";
      }
      {
        assertion = lib.any (
          failure: lib.hasInfix "services.velocity.configFiles contains unsafe relative paths" failure.message
        ) velocityUnsafeManagedPathFailures;
        message = "velocity managed config files should reject unsafe relative paths at eval time";
      }
      {
        assertion = lib.any (
          failure: lib.hasInfix "services.velocity.plugins contains unsafe plugin file names" failure.message
        ) velocityUnsafeManagedPathFailures;
        message = "velocity plugin file names should reject nested or unsafe paths at eval time";
      }
      {
        assertion = lib.any (
          failure:
          lib.hasInfix "services.velocity.plugins contains duplicate plugin file names" failure.message
        ) velocityDuplicatePluginFileNameFailures;
        message = "velocity plugin file names should reject duplicate managed jar names at eval time";
      }
    ];

    extended-attributes = [
      {
        assertion = builtins.hasAttr "/build/ix-xattr-test" extendedAttributes.config.ix.extendedAttributes;
        message = "generic ix.extendedAttributes should expose absolute runtime paths";
      }
      {
        assertion = builtins.any (
          p: (p.pname or null) == "attr"
        ) extendedAttributes.config.environment.systemPackages;
        message = "generic ix.extendedAttributes should add attr tools for runtime inspection";
      }
      {
        assertion =
          lib.hasInfix "/bin/setfattr" extendedAttributes.activationScript
          && lib.hasInfix "user.ix.kind" extendedAttributes.activationScript;
        message = "generic ix.extendedAttributes should render setfattr activation commands";
      }
      {
        assertion = lib.hasInfix "refusing to set extended attributes on symlink" extendedAttributes.activationScript;
        message = "generic ix.extendedAttributes should avoid following symlinks";
      }
      {
        assertion = lib.hasInfix "filesystem does not support extended attributes" extendedAttributes.activationScript;
        message = "generic ix.extendedAttributes should warn instead of failing on unsupported filesystems";
      }
    ];

    kernel-dev = [
      {
        assertion = kernelDev.config.services.git-clone.enable;
        message = "kernel-dev image should enable first-boot git cloning";
      }
      {
        assertion = kernelDev.git.clone.service.wantedBy == [ ];
        message = "timer-activated git clone should not be wanted by multi-user.target";
      }
      {
        assertion = kernelDev.git.clone.timer.wantedBy == [ "timers.target" ];
        message = "timer-activated git clone should be started by timers.target";
      }
    ];

    development-base = [
      {
        assertion =
          builtins.elem "claude-code" developmentBase.packageNames
          && builtins.elem "codex" developmentBase.packageNames;
        message = "development-base should ship the Claude Code and Codex CLIs";
      }
      {
        # Global allowUnfree would let every unfree package slip in. The
        # image is supposed to grant exactly one exception, by name.
        assertion = !(developmentBase.config.nixpkgs.config.allowUnfree or false);
        message = "development-base should not enable allowUnfree globally; use the predicate";
      }
      {
        assertion = !(builtins.elem "cursor-cli" developmentBase.packageNames);
        message = "development-base should keep unrelated unfree CLIs out of the image";
      }
    ];

    vitest = [
      {
        assertion = builtins.length vitestWorkspaceCases == 2;
        message = "vitest workspace fixture should enumerate one case per project";
      }
      {
        assertion = lib.all (
          case:
          case.testProject != null
          && case.testFile == "src/shared.test.js"
          &&
            case.vitestArgs == [
              "src/shared.test.js"
              "--project"
              case.testProject
              "--testNamePattern"
              "^shared project case$"
            ]
        ) vitestWorkspaceCases;
        message = "vitest per-case checks should filter project-specific manifest entries by project";
      }
    ];

    symphony-codex = [
      {
        assertion = symphonyCodex.config.ix.image.name == "ix/symphony-codex";
        message = "symphony-codex image should set the expected public OCI image name";
      }
      {
        assertion = symphonyCodex.config.ix.image.tag == "2026-05-28";
        message = "symphony-codex image should publish an immutable production tag";
      }
      {
        assertion = builtins.elem "symphony-room-server" symphonyCodex.packageNames;
        message = "symphony-codex image should include the room-server binary it starts";
      }
      {
        assertion = builtins.elem pkgs.codex symphonyCodex.packages;
        message = "symphony-codex image should include codex for diagnostic shell sessions";
      }
      {
        assertion =
          builtins.elem pkgs.gh symphonyCodex.packages && builtins.elem pkgs.git symphonyCodex.packages;
        message = "symphony-codex image should include GitHub and git tooling";
      }
      {
        assertion =
          builtins.elem pkgs.direnv symphonyCodex.packages
          && builtins.elem pkgs.ripgrep symphonyCodex.packages;
        message = "symphony-codex image should include common agent workspace tools";
      }
      {
        assertion = samePorts symphonyCodex.config.networking.firewall.allowedTCPPorts (
          baseFirewallTcpPorts ++ [ 8080 ]
        );
        message = "symphony-codex image should open room-server HTTP";
      }
      {
        assertion = samePorts symphonyCodex.config.networking.firewall.allowedUDPPorts (
          baseFirewallUdpPorts ++ [ 4433 ]
        );
        message = "symphony-codex image should open room-server WebTransport";
      }
      {
        assertion =
          let
            claims = symphonyCodex.config.ix.networking.portClaims;
          in
          claims.symphony-room-http.protocol == "tcp"
          && claims.symphony-room-http.port == 8080
          && claims.symphony-room-webtransport.protocol == "udp"
          && claims.symphony-room-webtransport.port == 4433;
        message = "symphony-codex image should register Room listener port claims";
      }
    ];

    minecraft = [
      {
        assertion = minecraft.config.ix.image.tag == defaultMinecraftVersion;
        message = "default minecraft image tag should follow versions.nix default";
      }
      {
        assertion = minecraft.cfg.properties."max-players" == 100000;
        message = "default minecraft image should allow the large ix player ceiling";
      }
      {
        assertion =
          minecraft.cfg.properties."online-mode" && minecraft.cfg.properties."enforce-secure-profile";
        message = "default minecraft image should keep account authentication and secure profiles explicit";
      }
      {
        assertion =
          velocityConcreteAddress.ix.healthChecks.velocity-status.command == [
            (lib.getExe repoPackages.mc-probe)
            "10.0.0.5:25570"
          ];
        message = "velocity SLP health checks should probe concrete bind addresses";
      }
      {
        assertion =
          minecraft.cfg.properties.gamemode == "survival"
          && !minecraft.cfg.properties."force-gamemode"
          && minecraft.cfg.properties.pvp
          && !minecraft.cfg.properties.hardcore
          && minecraft.cfg.properties."spawn-protection" == 16
          && !minecraft.cfg.properties."allow-flight"
          && !minecraft.cfg.properties."enable-command-block";
        message = "default minecraft image should keep conservative gameplay and command defaults";
      }
      {
        assertion =
          minecraft.cfg.properties."view-distance" == 32
          && minecraft.cfg.properties."simulation-distance" == 32;
        message = "default minecraft image should use the high-distance template defaults";
      }
      {
        assertion = lib.all (slug: builtins.hasAttr slug minecraft.config.services.minecraft.mods) [
          "fabric-api"
          "lithium"
          "c2me-fabric"
          "spark"
          "grimac"
        ];
        message = "default minecraft image should include the 26.1.2 Fabric server mod set";
      }
      {
        assertion = lib.getName minecraft.config.services.minecraft.javaPackage == "temurin-jre-bin";
        message = "default Fabric minecraft should use Temurin";
      }
      {
        assertion = lib.hasInfix "/bin/java" minecraft.service.config.ExecStart;
        message = "minecraft ExecStart should launch Java";
      }
      {
        assertion = lib.hasInfix "-XX:MaxRAMPercentage=85" minecraft.service.config.ExecStart;
        message = "minecraft should use MaxRAMPercentage for auto-scaling heap";
      }
      {
        assertion = lib.hasInfix "-XX:+UseG1GC" minecraft.service.config.ExecStart;
        message = "minecraft should include the default modern server GC flags";
      }
      {
        assertion =
          lib.hasInfix "-jar" minecraft.service.config.ExecStart
          && lib.hasInfix "nogui" minecraft.service.config.ExecStart;
        message = "minecraft ExecStart should launch the configured server jar in nogui mode";
      }
      {
        assertion = lib.hasInfix "minecraft-hot-reload-agent.jar=socket=/run/minecraft-hot-reload/socket" minecraft.service.config.ExecStart;
        message = "Fabric minecraft should start the hot reload Java agent";
      }
      {
        assertion = builtins.length minecraft.service.unit.reloadTriggers == 3;
        message = "minecraft managed files should trigger systemd reloads rather than unit restarts";
      }
      {
        assertion = lib.hasInfix "minecraft-sync-managed" minecraft.service.unit.preStart;
        message = "minecraft preStart should sync managed files from /etc";
      }
      {
        assertion = !(lib.hasInfix "fabric-api" minecraft.service.unit.preStart);
        message = "minecraft preStart should not embed managed mod store paths in the unit";
      }
      {
        assertion =
          minecraft.config.ix.extendedAttributes."/var/lib/minecraft".attributes."user.ix.kind"
          == "minecraft.server-root";
        message = "minecraft should label its runtime data directory through the generic xattr module";
      }
      {
        assertion =
          minecraft.config.ix.extendedAttributes."/var/lib/minecraft/world/region".attributes."user.ix.kind"
          == "minecraft.region-directory"
          &&
            minecraft.config.ix.extendedAttributes."/var/lib/minecraft/world/region".attributes."user.ix.minecraft.dimension"
            == "overworld";
        message = "minecraft should label overworld region directories through the generic xattr module";
      }
      {
        assertion =
          minecraft.config.ix.extendedAttributes."/var/lib/minecraft/world/DIM-1/region".attributes."user.ix.minecraft.dimension"
          == "nether"
          &&
            minecraft.config.ix.extendedAttributes."/var/lib/minecraft/world/DIM1/region".attributes."user.ix.minecraft.dimension"
            == "end";
        message = "minecraft should label Nether and End region directories through the generic xattr module";
      }
      # rcon coverage stays on the minecraft default image because the option
      # surface lives in `services.minecraft`, not in a paper-specific module.
      {
        assertion = minecraft.rcon.cfg.rcon.enable;
        message = "minecraft RCON should be enabled through a typed option";
      }
      {
        assertion = !(minecraft.rcon.cfg.properties ? "rcon.password");
        message = "typed minecraft RCON should not put the password in Nix-managed server.properties";
      }
      {
        assertion = samePorts minecraft.rcon.config.networking.firewall.allowedTCPPorts (
          baseFirewallTcpPorts ++ [ minecraft.rcon.cfg.port ]
        );
        message = "typed minecraft RCON should keep the RCON port private by default";
      }
      {
        assertion = samePorts minecraft.rcon.openFirewall.config.networking.firewall.allowedTCPPorts (
          baseFirewallTcpPorts
          ++ [
            minecraft.rcon.openFirewall.cfg.port
            minecraft.rcon.openFirewall.cfg.rcon.port
          ]
        );
        message = "typed minecraft RCON should open the firewall only when requested";
      }
      {
        assertion = minecraft.worldBorder.cfg.worldBorder.enable;
        message = "typed minecraft world border should expose an enable flag";
      }
      {
        assertion =
          minecraft.worldBorder.cfg.worldBorder.center.x == 100
          && minecraft.worldBorder.cfg.worldBorder.center.z == -50
          && minecraft.worldBorder.cfg.worldBorder.diameter == 8000;
        message = "typed minecraft world border should keep center and diameter settings";
      }
      {
        assertion = minecraft.worldBorder.cfg.rcon.enable;
        message = "typed minecraft world border should enable local RCON by default";
      }
      {
        assertion = samePorts minecraft.worldBorder.config.networking.firewall.allowedTCPPorts (
          baseFirewallTcpPorts ++ [ minecraft.worldBorder.cfg.port ]
        );
        message = "typed minecraft world border should keep the RCON port private";
      }
      {
        assertion =
          minecraft.worldBorder.service.after == [ "minecraft.service" ]
          && minecraft.worldBorder.service.requires == [ "minecraft.service" ];
        message = "typed minecraft world border should run after the Minecraft service is required";
      }
      {
        assertion = minecraft.access.cfg.properties.white-list;
        message = "typed minecraft whitelist should enable server.properties white-list";
      }
      {
        assertion = minecraft.access.cfg.properties.enforce-whitelist;
        message = "typed minecraft whitelist should enable enforce-whitelist by default";
      }
      {
        assertion = !(minecraft.access.cfg.serverFiles ? "whitelist.json");
        message = "typed minecraft whitelist should not symlink the mutable whitelist file through serverFiles";
      }
      {
        assertion = !(minecraft.access.cfg.serverFiles ? "ops.json");
        message = "typed minecraft operators should not symlink the mutable ops file through serverFiles";
      }
      {
        assertion = builtins.elem minecraft.access.managed.access minecraft.access.service.unit.restartTriggers;
        message = "typed minecraft access changes should restart the server so Minecraft rereads mutable access files";
      }
      {
        assertion = builtins.hasAttr "generated/example.snbt" minecraft.nbt.cfg.serverFiles;
        message = "minecraft serverFiles should accept readable SNBT files";
      }
      {
        assertion = builtins.hasAttr "generated/example.nbt" minecraft.nbt.cfg.serverFiles;
        message = "minecraft serverFiles should accept binary NBT files";
      }
      {
        assertion = builtins.hasAttr "generated/client.snbt" minecraft.nbt.cfg.configFiles;
        message = "minecraft configFiles should accept readable SNBT files";
      }
      {
        assertion = minecraft.datapacks.cfg.datapacks."max-height".worlds == [ "My World" ];
        message = "minecraft datapacks should default to the configured level-name world";
      }
      {
        assertion = builtins.hasAttr "/var/lib/minecraft/My World/datapacks" minecraft.datapacks.config.ix.extendedAttributes;
        message = "minecraft datapacks should annotate target world datapack directories";
      }
      {
        assertion = builtins.elem minecraft.datapacks.managed.datapacks minecraft.datapacks.service.unit.restartTriggers;
        message = "minecraft datapack changes should restart the server so registries are reloaded";
      }
    ];

    "minecraft_1.21.11-paper" = [
      {
        assertion = builtins.length minecraft.paper.service.unit.reloadTriggers == 3;
        message = "Paper minecraft managed plugins should trigger systemd reloads";
      }
      {
        assertion = !(minecraft.paper.service.config ? RuntimeDirectory);
        message = "Paper minecraft should not start the JVM hot reload socket";
      }
      {
        assertion = !(minecraft.paper.cfg.properties ? "rcon.password");
        message = "Paper minecraft should not put the RCON password in Nix-managed server.properties";
      }
      {
        assertion = samePorts minecraft.paper.config.networking.firewall.allowedTCPPorts (
          baseFirewallTcpPorts ++ [ minecraft.paper.cfg.port ]
        );
        message = "Paper minecraft should not expose the local RCON reload port through the firewall";
      }
    ];

    "minecraft_26.1.2-paper" = [
      {
        assertion = builtins.hasAttr "pvpindex-factions" minecraft.paperPlugins.cfg.pluginCatalog;
        message = "Paper minecraft should seed pluginCatalog from the generated 26.1.2 Paper catalog";
      }
      {
        assertion = builtins.elem 24455 minecraft.paperPlugins.config.networking.firewall.allowedUDPPorts;
        message = "Simple Voice Chat should open its UDP port when installed as a Paper plugin";
      }
      {
        assertion =
          minecraft.paperPlugins.cfg.serverFiles."plugins/voicechat/voicechat-server.properties".port
          == 24455;
        message = "Simple Voice Chat should render Paper plugin config under plugins/voicechat";
      }
      {
        assertion =
          minecraft.paperPlugins.cfg.worlds.factions.generator == "TerraformGenerator"
          && minecraft.paperPlugins.cfg.worlds.factions_nether.generator == "TerraformGenerator"
          && minecraft.paperPlugins.cfg.worlds.factions_the_end.generator == "TerraformGenerator";
        message = "TerraformGenerator should bind every configured world to its generator";
      }
      {
        assertion =
          minecraft.paperPlugins.cfg.bukkit.worlds.factions.generator == "TerraformGenerator"
          && minecraft.paperPlugins.cfg.bukkit.worlds.factions_nether.generator == "TerraformGenerator"
          && minecraft.paperPlugins.cfg.bukkit.worlds.factions_the_end.generator == "TerraformGenerator";
        message = "Minecraft worlds should render to bukkit.yml world generator entries";
      }
    ];

    minecraft-bedrock = [
      {
        assertion = bedrock.cfg.enable;
        message = "minecraft-bedrock image should enable services.minecraft-bedrock";
      }
      {
        assertion =
          bedrock.cfg.settings."server-port" == bedrock.cfg.port
          && bedrock.cfg.settings."server-portv6" == bedrock.cfg.portv6;
        message = "minecraft-bedrock server.properties should follow the configured UDP ports";
      }
      {
        assertion = samePorts bedrock.config.networking.firewall.allowedUDPPorts (
          baseFirewallUdpPorts
          ++ [
            bedrock.cfg.port
            bedrock.cfg.portv6
          ]
        );
        message = "minecraft-bedrock firewall should open only the configured UDP ports plus ix sidecar ports";
      }
      {
        assertion = lib.hasInfix "/bin/bedrock_server" bedrock.service.config.ExecStart;
        message = "minecraft-bedrock ExecStart should launch bedrock_server";
      }
    ];

    remote-desktop = [
      {
        assertion = remoteDesktop.cfg.enable;
        message = "remote-desktop image should enable services.remote-desktop";
      }
      {
        assertion = lib.getName remoteDesktop.cfg.package == lib.getName pkgs.xpra;
        message = "remote-desktop should default to the nixpkgs Xpra package";
      }
      {
        assertion = remoteDesktop.cfg.openFirewall;
        message = "remote-desktop image should explicitly open the browser port";
      }
      {
        assertion = remoteDesktop.cfg.allowUnauthenticated;
        message = "remote-desktop image should explicitly allow unauthenticated browser access";
      }
      {
        assertion = !remoteDesktopModuleDefault.cfg.openFirewall;
        message = "remote-desktop module should keep the browser port closed unless callers opt in";
      }
      {
        assertion = samePorts remoteDesktopModuleDefault.config.networking.firewall.allowedTCPPorts baseFirewallTcpPorts;
        message = "remote-desktop module default should leave only ix sidecar TCP ports open";
      }
      {
        assertion = lib.any (
          failure:
          lib.hasInfix "rendered Xpra auth = \"none\" requires services.remote-desktop.allowUnauthenticated = true" failure.message
        ) remoteDesktopUnauthenticatedFirewallFailures;
        message = "remote-desktop should reject unauthenticated firewall exposure unless it is explicit";
      }
      {
        assertion = lib.any (
          failure:
          lib.hasInfix "rendered Xpra auth = \"none\" requires services.remote-desktop.allowUnauthenticated = true" failure.message
        ) remoteDesktopSettingsAuthFirewallFailures;
        message = "remote-desktop should check settings.auth overrides before opening the firewall";
      }
      {
        assertion = lib.any (
          failure:
          lib.hasInfix "settings.bind-tcp must match services.remote-desktop.bindAddress" failure.message
        ) remoteDesktopBindTcpDriftFailures;
        message = "remote-desktop should reject a bind-tcp override that disagrees with the claimed listener";
      }
      {
        assertion = remoteDesktop.config.users.users.remote-desktop.isSystemUser;
        message = "remote-desktop user should be a system user";
      }
      {
        assertion = samePorts remoteDesktop.config.networking.firewall.allowedTCPPorts (
          baseFirewallTcpPorts ++ [ remoteDesktop.cfg.port ]
        );
        message = "remote-desktop firewall should open only the configured browser port plus ix sidecar ports";
      }
      {
        assertion = !(remoteDesktop.config.systemd.services ? xvfb);
        message = "remote-desktop should not use a standalone Xvfb service";
      }
      {
        assertion = !(remoteDesktop.config.systemd.services ? x11vnc);
        message = "remote-desktop should not use x11vnc";
      }
      {
        assertion = !(remoteDesktop.config.systemd.services ? novnc);
        message = "remote-desktop should not use a separate noVNC websockify service";
      }
    ];

    resource-monitor = [
      {
        assertion = resourceMonitor.service.config.DynamicUser;
        message = "resource-monitor stats writer should run as a dynamic systemd user";
      }
      {
        assertion = resourceMonitor.service.config.NoNewPrivileges;
        message = "resource-monitor stats writer should reject new privileges";
      }
      {
        assertion = resourceMonitor.service.config.ProtectSystem == "strict";
        message = "resource-monitor stats writer should use the shared strict filesystem hardening";
      }
      {
        assertion = resourceMonitor.service.config.RuntimeDirectory == "ix/resource-monitor";
        message = "resource-monitor should preserve nested /run runtime directory paths";
      }
      {
        assertion = lib.hasInfix "/run/ix/resource-monitor" resourceMonitor.service.config.ExecStart;
        message = "resource-monitor stats writer should write to the configured runtime directory";
      }
      {
        assertion =
          resourceMonitor.config.services.nginx.virtualHosts.resource-monitor.locations."/stats.json".root
          == "/run/ix/resource-monitor";
        message = "resource-monitor nginx should serve stats from the configured runtime directory";
      }
      {
        assertion = lib.all (
          failures:
          lib.any (
            failure:
            lib.hasInfix "services.resource-monitor.runtimeDirectory must be a managed /run subdirectory" failure.message
          ) failures
        ) resourceMonitorRuntimeDirectoryFailures;
        message = "resource-monitor should reject runtime directories outside /run and unsafe /run segments";
      }
    ];

    helpers = [
      {
        assertion = missingPackageMetadata == [ ];
        message =
          "packages with default.nix should declare package.nix metadata entries: "
          + lib.concatStringsSep ", " missingPackageMetadata;
      }
      {
        assertion = cargoUnitWorkspace.policyChecks ? unusedCrateDependencies;
        message = "cargo-unit workspaces should expose an unused dependency policy check by default";
      }
      {
        assertion = lib.hasInfix "--ordered-shutdown" processComposeApplication.passthru.tests.dryRun.buildCommand;
        message = "process-compose dry-run checks should include runtime wrapper arguments";
      }
      {
        assertion = goUnitWorkspace.sourceAudit.module.lockFile == "go.sum";
        message = "go-unit workspaces should require and report the Go module lockfile";
      }
      {
        assertion = goUnitWorkspace.packages ? root;
        message = "go-unit workspaces should expose package-shaped build derivations";
      }
      {
        assertion = goUnitWorkspace.packages.root.goUnit.goSum == goUnitFixture + "/go.sum";
        message = "go-unit package derivations should pass go.sum through to buildGoModule";
      }
      {
        assertion =
          goUnitWorkspace.vendorHashKey == "946e64650b103a2fe8d7518f522acad2ba766bd2c3700066125f33206d400b66";
        message = "go-unit workspaces should derive the vendor hash key from go.mod and go.sum";
      }
      {
        assertion =
          goUnitWorkspace.packages.root.goUnit.vendorHashFile == goUnitFixture + "/go-modules.nix";
        message = "go-unit package derivations should use the module-owned vendor hash file by default";
      }
      {
        assertion =
          goUnitWorkspace.packages.root.goUnit.goToolchain
          == ix.languages.go.toolchain pkgs { version = "latest"; };
        message = "go-unit package derivations should use the selected Go toolchain";
      }
      {
        assertion = goUnitWorkspace.packages.root.goUnit.env.GOFLAGS == "-mod=readonly";
        message = "go-unit package derivations should preserve buildGoModule env values";
      }
      {
        assertion = goUnitWorkspace.tests ? root;
        message = "go-unit workspaces should expose package-shaped test derivations";
      }
      {
        assertion = goUnitNestedWorkspace.sourceAudit.module.relative == "module";
        message = "go-unit workspaces should resolve default go.mod and go.sum below modRoot";
      }
      {
        assertion = goUnitStdlibWorkspace.sourceAudit.module.lockFile == null;
        message = "go-unit workspaces should allow stdlib-only modules without go.sum";
      }
      {
        assertion = goUnitStdlibWorkspace.packages.root.goUnit.goSum == null;
        message = "go-unit package derivations should pass null goSum for modules without go.sum";
      }
      {
        assertion = goUnitDerivedStdlibWorkspace.packages.root.goUnit.goSum == null;
        message = "go-unit derivation sources should allow no-sum modules when go.mod is readable";
      }
      {
        assertion =
          goUnitDerivedWorkspaceWithVendorHashFile.packages.root.goUnit.vendorHashKey
          == goUnitWorkspace.vendorHashKey;
        message = "go-unit derivation sources should use explicit vendor hash files by key";
      }
      {
        assertion = !goUnitDerivedUnreadableNoSumEval.success;
        message = "go-unit derivation sources should reject no-sum mode when go.mod is unreadable";
      }
      {
        assertion = !goUnitDerivedMissingGoSumKeyEval.success;
        message = "go-unit derivation sources should not derive vendor keys from go.mod alone";
      }
      {
        assertion = !goUnitMissingGoModEval.success;
        message = "go-unit local sources should reject missing go.mod during eval";
      }
      {
        assertion = !goUnitMissingGoModPackagesEval.success;
        message = "go-unit local package surfaces should reject missing go.mod during eval";
      }
      {
        assertion = !goUnitMissingGoSumEval.success;
        message = "go-unit local sources should reject missing go.sum even with a direct vendor hash";
      }
      {
        assertion = !goUnitMissingGoSumNoSumEval.success;
        message = "go-unit local sources with external requirements should not use the no-sum path";
      }
      {
        assertion = !goUnitRequireNoSpaceNoSumEval.success;
        message = "go-unit no-sum detection should reject compact require blocks";
      }
      {
        assertion = !goUnitMissingExplicitGoSumEval.success;
        message = "go-unit explicit go.sum paths should reject filtered-out files during eval";
      }
      {
        assertion = !goUnitPackageCollisionEval.success;
        message = "go-unit workspaces should reject package patterns with colliding output names";
      }
      {
        assertion = zigApplication.passthru.tests ? lib && zigApplication.passthru.tests ? exe;
        message = "Zig packages should expose named test steps as separate derivations";
      }
      {
        assertion = zigApplication.passthru.testSteps.lib == "test-lib";
        message = "Zig packages should retain the build step names behind test derivations";
      }
      {
        assertion = zigDepsApplication.passthru.zigDeps != null;
        message = "Zig packages should materialize remote build.zig.zon dependencies through a cache derivation";
      }
      {
        assertion =
          let
            inherit (nomadSecretRefsExample) secretSet;
          in
          secretSet.provider.type == "vaultwarden"
          && secretSet.provider.client == "rbw"
          && secretSet.provider.folder == "production"
          && secretSet.refs."daily-scraper/aws.env" == "/run/ix-secrets/daily-scraper/aws.env"
          && secretSet.values."daily-scraper/aws.env".key == "daily-scraper/aws-env"
          && secretSet.values."daily-scraper/aws.env".field == "notes";
        message = "secret refs should normalize provider keys and consumer paths generically";
      }
      {
        assertion =
          let
            job = builtins.readFile nomadSecretRefsExample.nomadJob;
          in
          lib.hasInfix ''source      = "/run/ix-secrets/daily-scraper/aws.env"'' job
          && lib.hasInfix ''destination = "secrets/aws.env"'' job
          && lib.hasInfix "env         = true" job;
        message = "secret refs should render into a Nomad environment template";
      }
      {
        assertion =
          let
            manifest = lib.importJSON nomadSecretRefsExample.kubernetesExternalSecret;
          in
          manifest.kind == "ExternalSecret"
          && manifest.metadata.namespace == "batch"
          && manifest.spec.secretStoreRef.name == "vaultwarden"
          && builtins.length manifest.spec.data == 2;
        message = "secret refs should render into a Kubernetes external secret manifest";
      }
      {
        assertion = lib.all (passed: passed) (lib.attrValues nomadSecretRefsExample.buildChecks);
        message = "Nomad secret refs build checks should be declarative Nix assertions";
      }
      {
        assertion = !invalidSecretNameEval.success;
        message = "secret refs should reject unsafe relative names during eval";
      }
      {
        assertion = cargoUnitWorkspace.policyChecks ? cargoAudit;
        message = "cargo-unit workspaces should expose a cargo-audit policy check by default";
      }
      {
        assertion = cargoUnitWorkspace.policyChecks ? clippy;
        message = "cargo-unit workspaces should expose a clippy policy check derivation";
      }
      {
        assertion = !(cargoUnitWorkspace.policyChecks ? cargoClippy);
        message = "cargo-unit buildWorkspace should suppress the legacy workspace-level cargoClippy when per-unit clippy is on";
      }
      {
        # `policyChecks.clippy` is one aggregate derivation so
        # `withPolicyChecks` and `selectBinaryWithTests` can string-coerce it
        # like every other policy check. The aggregate transitively depends on
        # every per-unit clippy derivation.
        assertion = lib.isDerivation cargoUnitWorkspace.policyChecks.clippy;
        message = "cargo-unit policyChecks.clippy should be a single aggregate derivation";
      }
      {
        # The per-unit fan-out lives at `clippyUnits` for callers that want
        # individual unit derivations (e.g. exposing one flake check per
        # crate).
        assertion = builtins.isAttrs cargoUnitWorkspace.clippyUnits;
        message = "cargo-unit clippyUnits should be a fan-out attrset, one entry per linted unit";
      }
      {
        assertion = builtins.length (builtins.attrNames cargoUnitWorkspace.clippyUnits) >= 2;
        message = "cargo-unit clippyUnits should produce multiple per-unit derivations for a multi-target fixture";
      }
      {
        assertion = builtins.all (unit: lib.isDerivation unit) (
          builtins.attrValues cargoUnitWorkspace.clippyUnits
        );
        message = "cargo-unit clippyUnits entries should each be a derivation";
      }
      {
        assertion = cargoUnitWorkspace.policy.clippy.package.unchecked.pname == "llm-clippy";
        message = "cargo-unit clippy checks should use llm-clippy by default";
      }
      {
        assertion =
          let
            denied = cargoUnitWorkspace.policy.clippy.deniedLints;
          in
          denied == [ ];
        message = "cargo-unit clippy policy should defer default lint levels to Cargo.toml";
      }
      {
        assertion = cargoUnitWorkspace.policyChecks ? cargoMachete;
        message = "cargo-unit workspaces should expose a cargo-machete policy check by default";
      }
      {
        assertion = !(cargoUnitWorkspace.binaries.cargo-unit-hello ? unchecked);
        message = "cargo-unit package outputs should stay independent from aggregate policy checks";
      }
      {
        assertion = builtins.hasAttr "cargo_unit_hello-all" cargoUnitSelectedHello.passthru.tests;
        message = "selectBinaryWithTests should schedule package-owned test binaries";
      }
      {
        assertion = builtins.all (test: lib.isDerivation test) (
          builtins.attrValues cargoUnitSelectedHello.passthru.tests
        );
        message = "selectBinaryWithTests should expose only derivations in passthru.tests";
      }
      {
        assertion = builtins.hasAttr "cargo_unit_hello-tests-returns_greeting" cargoUnitSelectedHello.passthru.tests;
        message = "selectBinaryWithTests should expose per-case test derivations by default";
      }
      {
        assertion = builtins.all (binary: builtins.hasAttr binary cargoUnitBinaries) [
          "cargo-unit-goodbye"
          "cargo-unit-hello"
        ];
        message = "cargo-unit should build several binary roots from one workspace graph";
      }
      {
        assertion = builtins.hasAttr "cargo_unit_hello" cargoUnitWorkspace.targetSets.test.tests;
        message = "cargo-unit workspaces should expose test targets as separate checks";
      }
      {
        assertion = cargoUnitWorkspace.doctests != { };
        message = "cargo-unit workspaces should expose doctest targets as separate checks";
      }
      {
        assertion = cargoUnitWorkspace.targetSets.build.doctests != { };
        message = "cargo-unit target sets should expose doctest targets next to build roots";
      }
      {
        assertion = builtins.hasAttr "greeting" cargoUnitWorkspace.targetSets.bench.benchmarks;
        message = "cargo-unit workspaces should expose benchmark targets separately from tests";
      }
      {
        assertion = builtins.hasAttr "greeting" cargoUnitWorkspace.benchmarks;
        message = "cargo-unit workspaces should expose aggregate benchmark targets";
      }
      {
        assertion = cargoUnitWorkspace ? testPlan;
        message = "cargo-unit workspaces should expose a reusable test plan";
      }
      {
        assertion = cargoUnitWorkspace ? coverageReport;
        message = "cargo-unit workspaces should expose a reusable coverage report";
      }
      {
        assertion = cargoUnitWorkspace ? makeCoverageReport;
        message = "cargo-unit workspaces should expose a customizable coverage report builder";
      }
      {
        assertion = cargoUnitWorkspace ? benchmarkPlan;
        message = "cargo-unit workspaces should expose a reusable benchmark plan";
      }
      {
        assertion = cargoUnitWorkspace ? compareTangoBenchmarks;
        message = "cargo-unit workspaces should expose a Tango comparison builder";
      }
      {
        assertion =
          cargoUnitWorkspace.targetSets.build.binaries.cargo-unit-hello.drvPath
          == cargoUnitWorkspace.binaries.cargo-unit-hello.drvPath;
        message = "cargo-unit should expose named target-set outputs without losing aggregate outputs";
      }
      {
        assertion = cargoUnitPolicyDisabledWorkspace.policyChecks == { };
        message = "cargo-unit policy checks should be disableable for generated workspaces";
      }
      {
        assertion = cargoUnitScope.base.alpha.drvPath != cargoUnitScope.alphaChanged.alpha.drvPath;
        message = "cargo-unit should rebuild the changed workspace crate";
      }
      {
        assertion = cargoUnitScope.base.bravo.drvPath == cargoUnitScope.alphaChanged.bravo.drvPath;
        message = "cargo-unit should keep unrelated workspace crate derivations stable when one crate source changes";
      }
      {
        assertion = cargoUnitScope.base.itoa.drvPath == cargoUnitScope.alphaChanged.itoa.drvPath;
        message = "cargo-unit should keep locked transitive dependency derivations stable when workspace source changes";
      }
      {
        assertion = cargoUnitScope.base.ryu.drvPath == cargoUnitScope.alphaChanged.ryu.drvPath;
        message = "cargo-unit should keep unrelated locked dependency derivations stable when workspace source changes";
      }
      {
        assertion = cargoUnitScope.base.itoa.drvPath != cargoUnitScope.lockChanged.itoa.drvPath;
        message = "cargo-unit should rebuild the locked dependency whose Cargo.lock entry changed";
      }
      {
        assertion = cargoUnitScope.base.ryu.drvPath == cargoUnitScope.lockChanged.ryu.drvPath;
        message = "cargo-unit should keep unrelated locked dependency derivations stable when another transitive dependency changes";
      }
      {
        assertion = builtins.any (
          source: source.base == "workspace" && source.scope == "package" && source.relative == "crates/alpha"
        ) (builtins.attrValues cargoUnitScopeWorkspaces.base.sourceAudit);
        message = "cargo-unit source audit should record package-shaped workspace sources";
      }
      {
        assertion = builtins.any (
          source:
          source.base == "vendor-package"
          && source.scope == "package"
          && source.sourceKey == "registry+https://github.com/rust-lang/crates.io-index#itoa@1.0.18"
        ) (builtins.attrValues cargoUnitScopeWorkspaces.base.sourceAudit);
        message = "cargo-unit source audit should record full dependency source identity";
      }
      {
        assertion = repoPackages.minecraft-nbt.passthru.policyChecks ? cargoMachete;
        message = "repo Rust packages should expose cargo-machete policy checks by default";
      }
      {
        # Repo packages route through `cargoUnit.buildWorkspace` via
        # `ix.rustWorkspace.units`, so they pick up the aggregate per-unit
        # clippy policy check rather than the legacy workspace-level
        # `cargoClippy` single derivation.
        assertion = repoPackages.minecraft-nbt.passthru.policyChecks ? clippy;
        message = "repo Rust packages should expose per-unit clippy policy checks by default";
      }
      {
        assertion = !(repoPackages.minecraft-nbt.passthru.policyChecks ? cargoClippy);
        message = "repo Rust packages should not also expose the legacy workspace-level cargoClippy when per-unit clippy is active";
      }
      {
        assertion =
          repoPackages.minecraft-nbt.passthru.policy.clippy.package.unchecked.pname == "llm-clippy";
        message = "repo Rust clippy checks should use llm-clippy by default";
      }
      {
        assertion =
          let
            denied = repoPackages.minecraft-nbt.passthru.policy.clippy.deniedLints;
          in
          denied == [ ];
        message = "repo Rust clippy policy should defer default lint levels to Cargo.toml";
      }
      {
        assertion = repoPackages.minecraft-nbt.passthru.tests ? package;
        message = "repo Rust package builds should be exposed as flake-checkable tests";
      }
      {
        assertion = repoPackages ? ix;
        message = "repo package set should expose the ix CLI package by default";
      }
      {
        assertion = repoPackages.minecraft-nbt.passthru.tests ? cargoMachete;
        message = "repo Rust policy checks should be exposed as flake-checkable tests";
      }
      {
        assertion = !(repoPackages.dag-runner.passthru ? unchecked);
        message = "repo Rust package outputs should not wrap unrelated workspace policy checks";
      }
      {
        assertion = builtins.hasAttr "integration-all" repoPackages.dag-runner.passthru.tests;
        message = "repo Rust package tests should include package-owned integration test targets";
      }
      {
        assertion = builtins.hasAttr "minecraft_nbt-all" repoPackages.minecraft-nbt.passthru.tests;
        message = "repo Rust package tests should include package-owned library test targets";
      }
      {
        assertion = builtins.hasAttr "property-all" repoPackages.minecraft-nbt.passthru.tests;
        message = "repo Rust package tests should include package-owned property test targets";
      }
      {
        assertion = builtins.hasAttr "doctest-minecraft_nbt-all" repoPackages.minecraft-nbt.passthru.tests;
        message = "repo Rust package tests should include package-owned doctest targets";
      }
      {
        assertion = minecraft.config.ix.build.ociEfficiency.enable;
        message = "OCI image builds should check layer efficiency by default";
      }
      {
        assertion =
          bunLockPackage.name == "clsx"
          && bunLockPackage.version == "2.1.1"
          && lib.hasPrefix "sha512-" bunLockPackage.integrity;
        message = "bun lock helper should derive registry fetch metadata from bun.lock";
      }
      {
        assertion =
          uvLockedDistribution.name == "click"
          && uvLockedDistribution.version == "8.1.7"
          && lib.hasPrefix "sha256-" uvLockedDistribution.hash;
        message = "uv lock helper should derive registry fetch metadata from uv.lock";
      }
      {
        assertion =
          builtins.elem "click-8.1.7-py3-none-any.whl" uvWheelhouseDistributionNames
          && !(builtins.elem "click-8.1.7.tar.gz" uvWheelhouseDistributionNames);
        message = "uv wheelhouses should prefer compatible wheels over sdists";
      }
    ];

    languages = [
      {
        assertion = !languages.pythonMissingVersion.success;
        message = "ix.languages.python should require an explicit interpreter version";
      }
      {
        assertion = !languages.pythonUnknown.success;
        message = "ix.languages.python should throw on an unknown version instead of returning a missing-attr error";
      }
      {
        assertion = !languages.rustMissingVersion.success;
        message = "ix.languages.rust should require an explicit toolchain version";
      }
      {
        assertion = languages.rustExtraComponents.drvPath != languages.rustPinnedNightly.drvPath;
        message = "ix.languages.rust should let callers extend the component set";
      }
      {
        assertion = !languages.rustBadChannel.success;
        message = "ix.languages.rust should reject unknown channels with errors.assertEnum";
      }
      {
        assertion = !languages.rustBadProfile.success;
        message = "ix.languages.rust should reject unknown profiles with errors.assertEnum";
      }
      {
        assertion = !languages.javaMissingDistribution.success;
        message = "ix.languages.java should require an explicit JDK distribution";
      }
      {
        assertion = !languages.javaBadDistribution.success;
        message = "ix.languages.java should reject unknown distributions with errors.assertEnum";
      }
      {
        assertion = !languages.javaBadVersion.success;
        message = "ix.languages.java should reject unknown versions with errors.requireAttr";
      }
      {
        assertion = lib.hasInfix "-agentpath:" minestomYourkit.execStart;
        message = "services.minestom.yourkit.enable should inject -agentpath: into the JVM command";
      }
      {
        assertion = lib.hasInfix "port=10001" minestomYourkit.execStart;
        message = "services.minestom.yourkit should pass the default YourKit port through the agent options";
      }
      {
        assertion = lib.hasInfix "listen=all" minestomYourkit.execStart;
        message = "services.minestom.yourkit.listen = \"all\" should appear in the agent options";
      }
      {
        assertion = lib.hasInfix "sessionname=minestom-eval-test" minestomYourkit.execStart;
        message = "services.minestom.yourkit.sessionName should appear in the agent options";
      }
      {
        assertion = builtins.elem 10001 minestomYourkit.firewallTcpPorts;
        message = "services.minestom.yourkit.openFirewall should open the YourKit port in the firewall";
      }
      {
        assertion = minestomYourkit.portClaim != null && minestomYourkit.portClaim.port == 10001;
        message = "services.minestom.yourkit.enable should register a portClaim for the YourKit port";
      }
      {
        assertion = !(lib.hasInfix "-agentpath:" minestomNoYourkit.execStart);
        message = "services.minestom without yourkit.enable should NOT include -agentpath:";
      }
      {
        assertion = minestomNoYourkit.portClaim == null;
        message = "services.minestom without yourkit.enable should NOT register a yourkit portClaim";
      }
    ];

    fleet = [
      {
        assertion = fleet.nodes.db.networking.hostName == "db";
        message = "fleet nodes should default hostName to the node name";
      }
      {
        assertion = fleet.nodes.db.ix.networking.eastWest.hostName == "db";
        message = "fleet nodes should expose their east-west host name through ix.networking";
      }
      {
        assertion = fleet.nodes.web.environment.etc."db-host".text == "db";
        message = "fleet node modules should be able to reference nodes.<name>.config";
      }
      {
        assertion = fleet.nodes.db.services.ix-postgresql.enable;
        message = "fleet plain attrset nodes should be treated as modules";
      }
      {
        assertion =
          fleet.nodes.web.environment.etc."session-key-ref".text == "/run/secrets/fleet/sessionKey";
        message = "fleet node modules should be able to consume declarative secret refs";
      }
      {
        assertion =
          let
            bootstrap =
              (ix.evalImageConfig {
                modules = [ ../images/system/test-cluster-bootstrap ];
              }).ix.image;
          in
          fleetPlan.web.bootstrapImage == "registry.ix.dev/${bootstrap.name}:${bootstrap.tag}";
        message = "fleet switches should create missing nodes from the shared NixOS bootstrap image";
      }
      {
        assertion = fleetPlan.web.replacementImage.destination == "fleet-web:latest";
        message = "fleet wrapped-node deployment destination should flow into the replacement image plan";
      }
      {
        assertion = fleetPlan.web.system == "${fleet.nodes.web.system.build.toplevel}";
        message = "fleet plans should expose the NixOS system closure for switch";
      }
      {
        assertion = fleet.systemPackages.web-system == fleet.nodes.web.system.build.toplevel;
        message = "fleet system package outputs should match default source switch installables";
      }
      {
        assertion = fleet.packages.web == fleet.nodes.web.ix.build.ociImage;
        message = "fleet replacement package outputs should keep node names";
      }
      {
        assertion =
          fleetPlan.web.switch == {
            target = builtins.unsafeDiscardStringContext fleet.nodes.web.system.build.toplevel.drvPath;
            buildOn = "remote";
            buildVm = null;
            sourceInstallable = ".#web-system";
            overrideInputs = { };
          };
        message = "fleet plans should default to local eval and remote build switch metadata";
      }
      {
        assertion =
          fleetPlan.web.replacementImage.sourceDrv
          == builtins.unsafeDiscardStringContext fleet.nodes.web.ix.build.ociImage.drvPath;
        message = "fleet plans should expose replacement image derivations without forcing local image builds";
      }
      {
        assertion = fleetPlan.web.region == "us-west-1";
        message = "fleet nodes should inherit the top-level deployment region";
      }
      {
        assertion = fleetPlan.web.tags == [ "public" ];
        message = "fleet wrapped-node tags should flow into the generated plan";
      }
      {
        assertion = fleetPlan.web.groups == [ "public-apps" ];
        message = "fleet wrapped-node east-west groups should flow into the generated plan";
      }
      {
        assertion = fleetPlan.web.ipv4;
        message = "fleet wrapped-node deployment overrides should flow into the generated plan";
      }
      {
        assertion =
          let
            check = fleetPlan.db.healthChecks.ix-postgresql;
            pgIsReady = lib.getExe' fleet.nodes.db.services.postgresql.package "pg_isready";
          in
          check.from == "guest"
          &&
            check.command == [
              pgIsReady
              "--quiet"
              "--host"
              "/run/postgresql"
              "--port"
              "5432"
            ]
          && check.timeoutSec == 30;
        message = "fleet plans should carry pg_isready-backed Postgres readiness checks";
      }
      {
        assertion = !fleetIpv4HealthCheckEval.success;
        message = "fleet plans should reject host-side IPv4 checks on private nodes";
      }
      {
        assertion = !fleetUnknownDependencyEval.success;
        message = "fleet plans should reject unknown dependsOn entries during eval";
      }
      {
        assertion = !fleetDependencyCycleEval.success;
        message = "fleet plans should reject cyclic dependsOn entries during eval";
      }
      {
        assertion =
          fleet.planValue.secrets.provider.type == "vaultwarden"
          && fleet.planValue.secrets.provider.collection == "production"
          && fleet.planValue.secrets.values.sessionKey.key == "web/session-key"
          && fleet.planValue.secrets.values.sessionKey.path == "/run/secrets/fleet/sessionKey"
          && fleet.planValue.secrets.values.sessionKey.generate;
        message = "fleet plans should carry declarative secret specs";
      }
      {
        assertion = fleetPlan."worker-0".baseName == "worker" && fleetPlan."worker-1".replicaIndex == 1;
        message = "fleet replicas should expand into stable node identities";
      }
      {
        assertion = fleetPlan."worker-0".dependsOn == [ "db" ];
        message = "fleet replica dependencies should point at expanded node identities";
      }
      {
        assertion =
          prefixedFleet.planValue.order == [
            "tprefix-api"
            "tprefix-worker"
          ];
        message = "nodePrefix should rename every node in the plan order";
      }
      {
        assertion = prefixedFleet.planValue.nodes."tprefix-worker".dependsOn == [ "tprefix-api" ];
        message = "nodePrefix should rewrite dependsOn references so the prefixed graph stays connected";
      }
      {
        assertion = prefixedFleet.planValue.nodes."tprefix-worker".groups == [ "tprefix-private-apps" ];
        message = "nodePrefix should rewrite east-west group names so scratch fleets do not collide";
      }
      {
        assertion = prefixedFleet.nodes."tprefix-api".networking.hostName == "tprefix-api";
        message = "nodePrefix should flow into the deployment-level identity (hostname, image name)";
      }
      {
        assertion = prefixedFleet.nodes."tprefix-worker".environment.etc."api-host".text == "tprefix-api";
        message = "nodes module-arg should resolve by the example's base name even when prefixed";
      }
    ];
  };

  # --- Per-image build-time checks ------------------------------------------

  buildScripts = {
    factions = ''
      grep -q '^QuickShop-Hikari$' ${factionsExample.managed.dropins}/quickshop-hikari.jar.plugin-name
      grep -q '^Vault$' ${factionsExample.managed.dropins}/vaultunlocked.jar.plugin-name
      grep -q '^Essentials$' ${factionsExample.managed.dropins}/essentialsx.jar.plugin-name
      grep -q '^EssentialsSpawn$' ${factionsExample.managed.dropins}/essentialsx-spawn.jar.plugin-name
      grep -q '^CoreProtect$' ${factionsExample.managed.dropins}/coreprotect.jar.plugin-name
      grep -q '^EternalEconomy$' ${factionsExample.managed.dropins}/eternaleconomy.jar.plugin-name
      grep -q '^CombatLog$' ${factionsExample.managed.dropins}/combatlogplugin.jar.plugin-name
      grep -q '^voicechat$' ${factionsExample.managed.dropins}/simple-voice-chat.jar.plugin-name
      grep -q '^BlueMap$' ${factionsExample.managed.dropins}/bluemap.jar.plugin-name
      grep -q '^Skript$' ${factionsExample.managed.dropins}/skript.jar.plugin-name
      grep -q '^max-world-size=6000$' ${factionsExample.managed.serverFiles}/server.properties
      grep -q 'max-tnt-per-tick: -1' ${factionsExample.managed.serverFiles}/spigot.yml
      grep -q 'query-plugins: false' ${factionsExample.managed.serverFiles}/bukkit.yml
      grep -q '^port=24454$' ${factionsExample.managed.serverFiles}/plugins/voicechat/voicechat-server.properties
      grep -q '"port": 8100' ${factionsExample.managed.serverFiles}/plugins/BlueMap/webserver.conf
      grep -q '"accept-download": true' ${factionsExample.managed.serverFiles}/plugins/BlueMap/core.conf
      grep -q '"height": 4064' ${factionsExample.managed.datapacks}/max-height/data/minecraft/dimension_type/overworld.json
      grep -q '"height": 4064' ${factionsExample.managed.datapacks}/max-height/data/minecraft/dimension_type/the_end.json
      grep -q 'optimize-explosions: true' ${factionsExample.managed.config}/paper-world-defaults.yml
      grep -q 'allow-piston-duplication: true' ${factionsExample.managed.config}/paper-global.yml
      grep -q 'worldborder set 12000' ${factionsExample.service.serviceConfig.ExecStart}
    '';

    survival = ''
      test -L ${survivalExample.managed.velocityPlugins}/Geyser-Velocity.jar
      test -L ${survivalExample.managed.velocityPlugins}/floodgate-velocity.jar
      grep -q 'bind = "0.0.0.0:25565"' ${survivalExample.managed.velocityConfig}/velocity.toml
      grep -q 'player-info-forwarding-mode = "modern"' ${survivalExample.managed.velocityConfig}/velocity.toml
      grep -q 'survival = "127.0.0.1:25566"' ${survivalExample.managed.velocityConfig}/velocity.toml
      grep -q 'auth-type: floodgate' ${survivalExample.managed.velocityConfig}/plugins/geyser/config.yml
      grep -q 'port: 19132' ${survivalExample.managed.velocityConfig}/plugins/geyser/config.yml
      grep -q 'send-floodgate-data: false' ${survivalExample.managed.velocityConfig}/plugins/floodgate/proxy-config.yml
      grep -q 'enabled: true' ${survivalExample.managed.minecraftConfig}/paper-global.yml
      grep -q 'secret: ix-survival-example-forwarding-secret-change-me' ${survivalExample.managed.minecraftConfig}/paper-global.yml
      grep -q '^server-port=25566$' ${survivalExample.managed.minecraftServerFiles}/server.properties
      grep -q '^online-mode=false$' ${survivalExample.managed.minecraftServerFiles}/server.properties
    '';

    observability-stack = ''
      test -x ${observabilityStackExample.observability.queryTool}/bin/ix-observe
      grep -q '"uid": "ix-observability"' ${observabilityStackExample.observability.dashboardPath}/overview.json
      grep -q 'otel_traces' ${observabilityStackExample.observability.dashboardPath}/overview.json
      grep -q 'otel_logs' ${observabilityStackExample.observability.dashboardPath}/overview.json
    '';

    extended-attributes = ''
      rm -rf /build/ix-xattr-test
      mkdir -p /build/ix-xattr-probe
      if ${pkgs.attr}/bin/setfattr --name user.ix.probe --value yes -- /build/ix-xattr-probe; then
        ${extendedAttributes.activationScript}
        test -d /build/ix-xattr-test
        test "$(${pkgs.attr}/bin/getfattr --absolute-names --only-values -n user.ix.kind /build/ix-xattr-test)" = "test.path"
        test "$(${pkgs.attr}/bin/getfattr --absolute-names --only-values -n user.ix.owner /build/ix-xattr-test)" = "ix"
      else
        echo "xattrs are not supported by the Nix build sandbox filesystem; checked activation rendering by eval"
      fi
    '';

    vitest = lib.concatMapStringsSep "\n" (case: "test -d ${case}") vitestWorkspaceCases;

    minecraft = ''
      ! grep -R 'rcon.password' ${minecraft.rcon.managed.serverFiles}
      grep -q 'worldborder center 100 -50' ${minecraft.worldBorder.service.serviceConfig.ExecStart}
      grep -q 'worldborder set 8000' ${minecraft.worldBorder.service.serviceConfig.ExecStart}
      grep -q '^query.port=25565$' ${minecraft.nestedProperties.managed.serverFiles}/server.properties
      grep -q '^rcon.port=25575$' ${minecraft.nestedProperties.managed.serverFiles}/server.properties
      grep -q '^white-list=true$' ${minecraft.access.managed.serverFiles}/server.properties
      grep -q '^enforce-whitelist=true$' ${minecraft.access.managed.serverFiles}/server.properties
      grep -q 'factions_nether:' ${
        minecraft.paperPlugins.config.environment.etc."minecraft/managed-server-files".source
      }/bukkit.yml
      grep -q 'factions_the_end:' ${
        minecraft.paperPlugins.config.environment.etc."minecraft/managed-server-files".source
      }/bukkit.yml
      grep -q 'generator: TerraformGenerator' ${
        minecraft.paperPlugins.config.environment.etc."minecraft/managed-server-files".source
      }/bukkit.yml
      grep -q '"name": "Alice"' ${minecraft.access.managed.access}/whitelist.json
      grep -q '"name": "Bob"' ${minecraft.access.managed.access}/whitelist.json
      grep -q '"level": 3' ${minecraft.access.managed.access}/ops.json
      grep -q '"bypassesPlayerLimit": true' ${minecraft.access.managed.access}/ops.json

      rm -rf /build/minecraft-access-data /build/minecraft-managed-root
      mkdir -p /build/minecraft-access-data/.ix-managed-access /build/minecraft-managed-root
      ln -s ${minecraft.access.managed.access} /build/minecraft-managed-root/managed-access
      ln -s ${minecraft.access.managed.serverFiles} /build/minecraft-managed-root/managed-server-files
      cp ${minecraft.access.fixtures.whitelist.current} /build/minecraft-access-data/whitelist.json
      cp ${minecraft.access.fixtures.whitelist.previous} /build/minecraft-access-data/.ix-managed-access/whitelist.json
      cp ${minecraft.access.fixtures.operators.current} /build/minecraft-access-data/ops.json
      cp ${minecraft.access.fixtures.operators.previous} /build/minecraft-access-data/.ix-managed-access/ops.json

      ${lib.getExe minecraft.access.syncManaged}
      test ! -L /build/minecraft-access-data/whitelist.json
      test ! -L /build/minecraft-access-data/ops.json
      grep -q '"name": "Alice"' /build/minecraft-access-data/whitelist.json
      grep -q '"name": "Bob"' /build/minecraft-access-data/whitelist.json
      grep -q '"name": "Manual"' /build/minecraft-access-data/whitelist.json
      ! grep -q '"name": "Removed"' /build/minecraft-access-data/whitelist.json
      grep -q '"level": 3' /build/minecraft-access-data/ops.json
      grep -q '"bypassesPlayerLimit": true' /build/minecraft-access-data/ops.json
      grep -q '"name": "ManualOp"' /build/minecraft-access-data/ops.json
      ! grep -q '"name": "RemovedOp"' /build/minecraft-access-data/ops.json

      grep -q 'DataVersion: 4325' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Enabled: 1B' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Health: 20S' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Angle: 0.5F' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Precise: 12.25' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'B;' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Dimension: "minecraft:overworld"' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Side: config' ${minecraft.nbt.managed.config}/generated/client.snbt
      test "$(od -An -tx1 -N5 ${minecraft.nbt.managed.serverFiles}/generated/example.nbt | tr -d ' \n')" = "0a00026978"
      test "$(od -An -tx1 -N2 ${minecraft.nbt.managed.serverFiles}/generated/example.nbt.gz | tr -d ' \n')" = "1f8b"

      grep -q '"max_format": 101' ${minecraft.datapacks.managed.datapacks}/max-height/pack.mcmeta
      grep -q '"min_y": -2032' ${minecraft.datapacks.managed.datapacks}/max-height/data/minecraft/dimension_type/overworld.json
      grep -q '"height": 4064' ${minecraft.datapacks.managed.datapacks}/max-height/data/minecraft/dimension_type/overworld.json

      rm -rf /build/minecraft-datapack-data /build/minecraft-datapack-managed-root
      mkdir -p /build/minecraft-datapack-managed-root
      ln -s ${minecraft.datapacks.managed.datapacks} /build/minecraft-datapack-managed-root/managed-datapacks

      ${lib.getExe minecraft.datapacks.syncManaged}
      test -L "/build/minecraft-datapack-data/My World/datapacks/max-height"
      grep -q '"logical_height": 4064' "/build/minecraft-datapack-data/My World/datapacks/max-height/data/minecraft/dimension_type/overworld.json"
    '';

    "minecraft_1.21.11-paper" = ''
      grep -q 'ignored-plugins' ${minecraft.paper.managed.serverFiles}/plugins/PlugManX/config.yml
      grep -q 'PlugManX' ${minecraft.paper.managed.serverFiles}/plugins/PlugManX/config.yml
      ! grep -R 'rcon.password' ${minecraft.paper.managed.serverFiles}
      grep -q '^almanac$' ${minecraft.paper.managed.dropins}/almanac.jar.plugin-name
      grep -q '^PlugManX$' ${minecraft.paper.managed.dropins}/PlugManX.jar.plugin-name
      grep -q -- '--password-file "/var/lib/minecraft/.ix-rcon-password"' ${minecraft.paper.service.config.ExecReload}
      grep -q 'plugman $row.action $row.plugin' ${minecraft.paper.service.config.ExecReload}
      ! grep -q 'reload all' ${minecraft.paper.service.config.ExecReload}
    '';
  };

  helperScript = ''
    test -e ${nomadSecretRefsExample.buildCheck}

    ${lib.getExe pythonAppClosureProbe} > python-app-closure-probe.out
    grep -q 'python app source is in the runtime closure' python-app-closure-probe.out
    test -e ${processComposeApplication.passthru.tests.dryRun}

    ${lib.getExe zigApplication} > zig-app-fixture.out
    grep -q 'hello from zig app fixture' zig-app-fixture.out
    test -e ${zigApplication.passthru.tests.lib}/done
    test -e ${zigApplication.passthru.tests.exe}/done
    test -e ${zigDepsApplication}/bin/zig-deps-fixture
    test -e ${zigDepsApplication.passthru.tests.default}/done

    ${cargoUnitHello}/bin/cargo-unit-hello > cargo-unit-hello.out
    grep -q 'hello from cargo-unit' cargo-unit-hello.out
    ${cargoUnitBinaries.cargo-unit-goodbye}/bin/cargo-unit-goodbye > cargo-unit-goodbye.out
    grep -q 'goodbye from cargo-unit' cargo-unit-goodbye.out
    test -d ${cargoUnitWorkspace.targetSets.test.tests.cargo_unit_hello.all}
    test -d ${cargoUnitWorkspace.targetSets.test.tests.cargo_unit_hello.cases."tests::returns_greeting"}
    test -d ${(builtins.head (builtins.attrValues cargoUnitWorkspace.doctests)).all}
    test -d ${(builtins.head (builtins.attrValues (builtins.head (builtins.attrValues cargoUnitWorkspace.doctests)).cases))}
    test -s ${cargoUnitWorkspace.testPlan}/packages/cargo-unit-hello/test-binaries
    grep -q '/bin/cargo_unit_hello$' ${cargoUnitWorkspace.testPlan}/packages/cargo-unit-hello/test-binaries
    grep -qx '.' ${cargoUnitWorkspace.testPlan}/packages/cargo-unit-hello/package-root
    grep -q '^cargo-unit-source-cargo-unit-hello-0.1.0-.*	[.]$' ${cargoUnitWorkspace.testPlan}/source-roots.tsv
    test -s ${cargoUnitCoverageWorkspace.coverageReport}/lcov.info
    test -s ${cargoUnitCoverageWorkspace.coverageReport}/merged.profdata
    grep -q '^SF:src/lib.rs$' ${cargoUnitCoverageWorkspace.coverageReport}/lcov.info
    grep -q '^DA:' ${cargoUnitCoverageWorkspace.coverageReport}/lcov.info
    test -x ${cargoUnitWorkspace.benchmarkPlan}/packages/cargo-unit-hello/benchmarks/greeting
    grep -q '^cargo-unit-hello	greeting	.*/bin/greeting$' ${cargoUnitWorkspace.benchmarkPlan}/benchmarks.tsv
    test -e ${cargoUnitTangoComparison}/done
    grep -q '^cargo-unit-hello	greeting	' ${cargoUnitTangoComparison}/benchmarks.tsv
    grep -q '^greeting ' ${cargoUnitTangoComparison}/logs/cargo-unit-hello-greeting.log
    ${goUnitWorkspace.default}/bin/go-unit-hello > go-unit-hello.out
    grep -q 'hello from go-unit: Hello, world.' go-unit-hello.out
    test -e ${goUnitWorkspace.tests.root}/done
    ${goUnitNestedWorkspace.default}/bin/go-unit-nested > go-unit-nested.out
    grep -q 'hello from nested go-unit: Hello, world.' go-unit-nested.out
    test -e ${goUnitNestedWorkspace.tests.root}/done
    ${goUnitStdlibWorkspace.default}/bin/go-unit-stdlib > go-unit-stdlib.out
    grep -q 'HELLO FROM GO-UNIT STDLIB' go-unit-stdlib.out
    test -e ${goUnitStdlibWorkspace.tests.root}/done
    ${goUnitDerivedStdlibWorkspace.default}/bin/go-unit-stdlib > go-unit-stdlib-derived.out
    grep -q 'HELLO FROM GO-UNIT STDLIB' go-unit-stdlib-derived.out
    test -e ${goUnitDerivedStdlibWorkspace.tests.root}/done

    grep -q 'class="ix bun"' ${bunSite}/share/bun-site-fixture/index.html
    test -d ${bunSite.bunNodeModules}/node_modules/clsx
    test -x ${bunSite.bunNodeModules.nodeCompat}/bin/node
    grep -q 'class="ix npm"' ${npmSite}/share/npm-site-fixture/index.html
    grep -q 'class="ix svelte"' ${svelteSite}/share/svelte-site-fixture/index.html
    test ! -L ${svelteSite}/share/svelte-site-fixture
    test ! -L ${svelteSite}/share/svelte-site-fixture/index.html
    grep -q -- '--route-prefix' ${svelteSite.passthru.serve}/bin/svelte-site-fixture
    grep -q -- '/fixture' ${svelteSite.passthru.serve}/bin/svelte-site-fixture
    test -x ${svelteSite}/bin/svelte-site-fixture
    grep -q -- "Svelte Site Fixture" ${svelteSite}/bin/svelte-site-fixture
    test -x ${svelteSite.passthru.devServer}/bin/svelte-site-fixture-dev

    ${uvApplication}/bin/uv-app-fixture > uv-app-fixture.out
    grep -q 'hello from uv app fixture' uv-app-fixture.out
    test -e ${uvApplication.uvWheelhouse}/click-8.1.7-py3-none-any.whl

    ${lib.getExe mcpPackage} eval '1 + 2' > mcp-eval.out
    grep -q 'result:' mcp-eval.out
    grep -q '^3$' mcp-eval.out
  '';

  cargoUnitRealWorkspaceAssertions = [
    {
      assertion = builtins.hasAttr "serde_derive" cargoUnitRealWorkspaces.serde.buildWorkspace.libraries;
      message = "cargo-unit should build Serde's proc-macro workspace library";
    }
    {
      assertion = builtins.hasAttr "thiserror_impl" cargoUnitRealWorkspaces.thiserror.buildWorkspace.libraries;
      message = "cargo-unit should build Thiserror's derive implementation workspace member";
    }
    {
      assertion = builtins.hasAttr "indexmap" cargoUnitRealWorkspaces.indexmap.testWorkspace.tests;
      message = "cargo-unit should expose Indexmap's real workspace test binary";
    }
    {
      assertion = builtins.hasAttr "regex-cli" cargoUnitRealWorkspaces.regex.buildWorkspace.binaries;
      message = "cargo-unit should expose Regex's real workspace binary target";
    }
    {
      assertion = builtins.hasAttr "regex_syntax" cargoUnitRealWorkspaces.regex.testWorkspace.tests;
      message = "cargo-unit should expose Regex Syntax's real package tests";
    }
  ];

  cargoUnitRealWorkspaceScript = ''
    test -d ${cargoUnitRealWorkspaces.serde.buildRoots}
    test -d ${cargoUnitRealWorkspaces.thiserror.buildRoots}
    test -d ${cargoUnitRealWorkspaces.indexmap.buildRoots}
    test -d ${cargoUnitRealWorkspaces.indexmap.testRoots}
    test -d ${cargoUnitRealWorkspaces.regex.buildRoots}
    test -d ${cargoUnitRealWorkspaces.regex.testRoots}
  '';

  # --- Test derivation builder ----------------------------------------------

  mkTest =
    name: assertions: extraScript:
    let
      failures = map (a: a.message) (lib.filter (a: !a.assertion) assertions);
    in
    assert lib.assertMsg (failures == [ ]) (
      "ix-test-${name}:\n  " + lib.concatStringsSep "\n  " failures
    );
    pkgs.runCommand "ix-test-${name}" { nativeBuildInputs = [ pkgs.gnugrep ]; } ''
      ${extraScript}
      mkdir -p "$out"
    '';

  imageTests = lib.mapAttrs (name: assertions: mkTest name assertions (buildScripts.${name} or "")) (
    removeAttrs groups [ "fleet" ]
  );

  fleetTest = mkTest "fleet" groups.fleet "";

  helperTest = pkgs.runCommand "ix-test-helpers" { nativeBuildInputs = [ pkgs.gnugrep ]; } ''
    ${helperScript}
    mkdir -p "$out"
  '';

  cargoUnitRealWorkspacesTest =
    mkTest "cargo-unit-real-workspaces" cargoUnitRealWorkspaceAssertions
      cargoUnitRealWorkspaceScript;
in
{
  inherit imageTests groups cargoUnitRealWorkspaceAssertions;
  cargoUnitRealWorkspaces = cargoUnitRealWorkspacesTest;

  # Aggregate. Pulls every per-image test into one derivation so
  # `nix flake check` covers the whole suite.
  eval = pkgs.linkFarmFromDrvs "ix-images-eval-tests" (
    lib.attrValues imageTests
    ++ [
      fleetTest
      helperTest
    ]
  );
}
