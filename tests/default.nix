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

  versions = import ../images/games/minecraft/versions.nix {
    inherit lib;
    inherit (ix) artifacts;
  };
  defaultMinecraftVersion = versions.default;
  defaultMinecraftModule = versions.${defaultMinecraftVersion};

  # Thin wrapper to keep call sites as plain lists; delegates to ix.evalImageConfig
  # so tests exercise the same evaluation path as production image builds.
  evalConfig = modules: ix.evalImageConfig { inherit modules; };
  failedAssertionsFor =
    modules:
    let
      config = evalConfig modules;
    in
    builtins.filter (assertion: !assertion.assertion) config.assertions;

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
                properties.level-name = "custom";
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

  hyperion =
    let
      config = evalConfig [ ../images/games/hyperion ];
    in
    {
      inherit config;
      cfg = config.services.hyperion;
      service =
        let
          unit = config.systemd.services.hyperion;
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

  pythonAppClosureProbe = ix.writePythonApplication pkgs {
    name = "python-app-closure-probe";
    src = pkgs.writeText "python-app-closure-probe.py" ''
      print("python app source is in the runtime closure")
    '';
    check = false;
  };

  cargoUnitFixture = fs.toSource {
    root = ./fixtures/cargo-unit-hello;
    fileset = fs.unions [
      ./fixtures/cargo-unit-hello/Cargo.lock
      ./fixtures/cargo-unit-hello/Cargo.toml
      ./fixtures/cargo-unit-hello/src
    ];
  };

  cargoUnitWorkspace = ix.cargoUnit.buildWorkspace {
    src = cargoUnitFixture;
    workspaceRoot = ./fixtures/cargo-unit-hello;
    cargoArgs = [
      "--bin"
      "cargo-unit-hello"
    ];
  };

  cargoUnitHello = cargoUnitWorkspace.binaries.cargo-unit-hello;

  cargoUnitBinaries = ix.cargoUnit.buildBinaries {
    src = cargoUnitFixture;
    workspaceRoot = ./fixtures/cargo-unit-hello;
    binaries = [
      "cargo-unit-goodbye"
      "cargo-unit-hello"
    ];
  };

  cargoUnitTestWorkspace = ix.cargoUnit.buildWorkspace {
    src = cargoUnitFixture;
    workspaceRoot = ./fixtures/cargo-unit-hello;
    cargoArgs = [
      "--workspace"
      "--tests"
    ];
  };

  cargoUnitPolicyDisabledWorkspace = ix.cargoUnit.buildWorkspace {
    src = cargoUnitFixture;
    workspaceRoot = ./fixtures/cargo-unit-hello;
    cargoArgs = [
      "--bin"
      "cargo-unit-hello"
    ];
    policy = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      clippy.enable = false;
    };
  };

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
            builtins.attrValues testWorkspace.tests
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
        "-p"
        "regex-syntax"
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
  };

  bunLockPackage = builtins.head bunSite.bunNodeModules.bunCache.lock.packages;

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

  pythonMcpServerPackage = (ix.packageSetFor pkgs).python-mcp-server;

  fleet = ix.mkFleet {
    deployment.region = "hil-1";
    secrets.sessionKey.generate = true;

    nodes = {
      db = {
        services.ix-postgresql.enable = true;
      };

      web = {
        tags = [ "public" ];
        deployment = {
          destination = "fleet-web:latest";
          ipv4 = true;
        };
        modules = [
          (
            { nodes, ... }:
            {
              services.remote-desktop.enable = true;
              environment.etc."db-host".text = nodes.db.config.networking.hostName;
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

  factionsExample =
    let
      fleet = import ../examples/factions-server {
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
      fleet = import ../examples/survival-server {
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
      cfg = config.services.daily-scraper;
      plan = fleet.planValue.nodes.scraper;
      service = config.systemd.services.daily-scraper;
      timer = config.systemd.timers.daily-scraper;
    };

  dailyScraperS3 =
    let
      config = evalConfig [
        ../examples/python-daily-scraper/service.nix
        {
          services.daily-scraper = {
            enable = true;
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
      cfg = config.services.daily-scraper;
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

  # --- Per-image assertion groups -------------------------------------------

  groups = {
    base = [
      {
        assertion = base.cfg.shellWorkspace.enable;
        message = "base profile should enable the ix shell workspace by default";
      }
      {
        assertion = base.cfg.shellWorkspace.directory == "/work/ix";
        message = "base profile should use /work/ix as the default shell workspace";
      }
      {
        assertion = base.config.users.users.root.shell.meta.mainProgram == "ix-workspace-shell";
        message = "base profile should make root enter the workspace shell wrapper";
      }
      {
        assertion = base.config.users.users.root.shell.shellPath == "/bin/ix-workspace-shell";
        message = "base profile workspace shell wrapper should be accepted as a NixOS shell package";
      }
      {
        assertion = builtins.elem base.cfg.shellWorkspace.shell base.config.environment.systemPackages;
        message = "base profile should install the configured workspace shell";
      }
    ];

    factions-server = [
      {
        assertion = factionsExample.config.ix.image.tag == "factions-server";
        message = "factions-server example should set a stable replacement image tag";
      }
      {
        assertion =
          factionsExample.cfg.worldBorder.enable
          && factionsExample.cfg.worldBorder.diameter == 12000
          && factionsExample.cfg.properties."max-world-size" == 6000;
        message = "factions-server example should declare a managed world border";
      }
      {
        assertion =
          let
            ports = factionsExample.config.networking.firewall.allowedTCPPorts;
          in
          builtins.elem factionsExample.cfg.port ports
          && builtins.elem 8100 ports
          && !(builtins.elem factionsExample.cfg.rcon.port ports);
        message = "factions-server example should keep RCON private while exposing Minecraft and BlueMap";
      }
      {
        assertion = builtins.elem 24454 factionsExample.config.networking.firewall.allowedUDPPorts;
        message = "factions-server example should expose Simple Voice Chat on the default UDP port";
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
        message = "factions-server example should register every service listener in ix.networking.portClaims";
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
        message = "factions-server should layer systemctl + SLP-with-MOTD + host TCP probes";
      }
    ];

    survival-server = [
      {
        assertion = survivalExample.config.ix.image.tag == "survival-server";
        message = "survival-server example should set a stable replacement image tag";
      }
      {
        assertion =
          survivalExample.velocity.enable
          && survivalExample.velocity.servers.survival == "127.0.0.1:25566"
          && survivalExample.velocity.try == [ "survival" ]
          && survivalExample.velocity.forwarding.mode == "modern";
        message = "survival-server example should route Velocity to the local Paper backend";
      }
      {
        assertion =
          survivalExample.geyser.enable
          && survivalExample.geyser.remote.authType == "floodgate"
          && survivalExample.floodgate.enable;
        message = "survival-server example should enable Geyser with Floodgate auth";
      }
      {
        assertion =
          survivalExample.minecraft.paper.enable
          && survivalExample.minecraft.version == "26.1.2"
          && survivalExample.minecraft.port == 25566
          && !survivalExample.minecraft.openFirewall
          && !survivalExample.minecraft.properties."online-mode";
        message = "survival-server example should keep Paper behind the proxy";
      }
      {
        assertion =
          let
            ports = survivalExample.config.networking.firewall.allowedTCPPorts;
          in
          builtins.elem 25565 ports
          && !(builtins.elem 25566 ports)
          && !(builtins.elem survivalExample.minecraft.rcon.port ports);
        message = "survival-server example should expose Velocity while keeping backend and RCON private";
      }
      {
        assertion = builtins.elem 19132 survivalExample.config.networking.firewall.allowedUDPPorts;
        message = "survival-server example should expose Geyser's Bedrock UDP listener";
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
        message = "survival-server example should register proxy, backend, RCON, and Bedrock listeners";
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
        message = "survival-server should expose layered guest/host probes with MOTD-aware SLP on both proxy and backend";
      }
    ];

    python-daily-scraper = [
      {
        assertion = dailyScraperExample.config.ix.image.tag == "daily-scraper";
        message = "python-daily-scraper example should set a stable replacement image tag";
      }
      {
        assertion =
          dailyScraperExample.cfg.enable
          && dailyScraperExample.cfg.package.meta.mainProgram == "daily-scraper"
          && dailyScraperExample.cfg.repository == "indexable-inc/index";
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
          dailyScraperS3.cfg.s3.uri == "s3://andrew-scraper-output/github"
          && lib.hasInfix "s3 sync --only-show-errors /var/lib/daily-scraper/parquet s3://andrew-scraper-output/github --delete" dailyScraperS3.service.serviceConfig.ExecStartPost
          &&
            dailyScraperS3.service.serviceConfig.LoadCredential == [
              "aws-env:/run/secrets/daily-scraper/aws.env"
            ]
          && dailyScraperS3.service.serviceConfig.EnvironmentFile == "%d/aws-env";
        message = "python-daily-scraper service should support S3 sync through systemd credentials";
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
    ];

    kernel-dev = [
      {
        assertion = kernelDev.config.ix.image.name == "linux-kernel-dev";
        message = "kernel-dev image should set the expected OCI image name";
      }
      {
        assertion = kernelDev.config.services.git-clone.enable;
        message = "kernel-dev image should enable first-boot git cloning";
      }
      {
        assertion = kernelDev.config.services.git-clone.url == "https://github.com/torvalds/linux.git";
        message = "kernel-dev image should clone the Linux repository";
      }
      {
        assertion = kernelDev.config.services.git-clone.dest == "/src/linux";
        message = "kernel-dev image should clone Linux into /src/linux";
      }
      {
        assertion = kernelDev.config.services.git-clone.activation == "timer";
        message = "kernel-dev image should clone Linux after boot readiness";
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

    minecraft = [
      {
        assertion = minecraft.config.ix.image.tag == defaultMinecraftVersion;
        message = "default minecraft image tag should follow versions.nix default";
      }
      {
        assertion = defaultMinecraftVersion == "26.1.2-fabric";
        message = "default minecraft image should use the stable 26.1.2 Fabric variant";
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
        assertion = minecraft.service.config.RuntimeDirectory == "minecraft-hot-reload";
        message = "Fabric minecraft should create a runtime directory for the hot reload socket";
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
        assertion = minecraft.rcon.cfg.rcon.passwordFile == "/var/lib/minecraft/.ix-rcon-password";
        message = "minecraft RCON should default to a state-local password file";
      }
      {
        assertion = !(minecraft.rcon.cfg.properties ? "rcon.password");
        message = "typed minecraft RCON should not put the password in Nix-managed server.properties";
      }
      {
        assertion =
          minecraft.rcon.config.networking.firewall.allowedTCPPorts == [ minecraft.rcon.cfg.port ];
        message = "typed minecraft RCON should keep the RCON port private by default";
      }
      {
        assertion =
          minecraft.rcon.openFirewall.config.networking.firewall.allowedTCPPorts == [
            minecraft.rcon.openFirewall.cfg.port
            minecraft.rcon.openFirewall.cfg.rcon.port
          ];
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
        assertion =
          minecraft.worldBorder.config.networking.firewall.allowedTCPPorts == [
            minecraft.worldBorder.cfg.port
          ];
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
        assertion = minecraft.datapacks.cfg.datapacks."max-height".worlds == [ "custom" ];
        message = "minecraft datapacks should default to the configured level-name world";
      }
      {
        assertion = builtins.hasAttr "/var/lib/minecraft/custom/datapacks" minecraft.datapacks.config.ix.extendedAttributes;
        message = "minecraft datapacks should annotate target world datapack directories";
      }
      {
        assertion = builtins.elem minecraft.datapacks.managed.datapacks minecraft.datapacks.service.unit.restartTriggers;
        message = "minecraft datapack changes should restart the server so registries are reloaded";
      }
    ];

    "minecraft_1.21.11-paper" = [
      {
        assertion = minecraft.paper.cfg.dropinDir == "plugins";
        message = "Paper minecraft should use the plugins drop directory";
      }
      {
        assertion = builtins.length minecraft.paper.service.unit.reloadTriggers == 3;
        message = "Paper minecraft managed plugins should trigger systemd reloads";
      }
      {
        assertion = !(minecraft.paper.service.config ? RuntimeDirectory);
        message = "Paper minecraft should not start the JVM hot reload socket";
      }
      {
        assertion =
          minecraft.paper.cfg.autoReload.rconPasswordFile == "/var/lib/minecraft/.ix-rcon-password";
        message = "Paper minecraft should use a state-local RCON password file";
      }
      {
        assertion = !(minecraft.paper.cfg.properties ? "rcon.password");
        message = "Paper minecraft should not put the RCON password in Nix-managed server.properties";
      }
      {
        assertion =
          minecraft.paper.config.networking.firewall.allowedTCPPorts == [ minecraft.paper.cfg.port ];
        message = "Paper minecraft should not expose the local RCON reload port through the firewall";
      }
    ];

    "minecraft_26.1.2-paper" = [
      {
        assertion = builtins.hasAttr "pvpindex-factions" minecraft.paperPlugins.cfg.pluginCatalog;
        message = "Paper minecraft should seed pluginCatalog from the generated 26.1.2 Paper catalog";
      }
      {
        assertion =
          minecraft.paperPlugins.cfg.pluginCatalog.pvpindex-factions.pluginName == "PvPIndexFactions";
        message = "Generated Paper plugin catalog should preserve Bukkit plugin names";
      }
      {
        assertion = minecraft.paperPlugins.cfg.pluginCatalog.worldedit.pluginName == "WorldEdit";
        message = "Paper minecraft should expose the generated WorldEdit plugin entry";
      }
      {
        assertion = minecraft.paperPlugins.cfg.pluginCatalog.simple-voice-chat.pluginName == "voicechat";
        message = "Generated Simple Voice Chat plugin entry should use its Bukkit runtime name";
      }
      {
        assertion = minecraft.paperPlugins.cfg.pluginCatalog.vaultunlocked.pluginName == "Vault";
        message = "Generated VaultUnlocked plugin entry should use the Vault runtime name";
      }
      {
        assertion =
          minecraft.paperPlugins.cfg.pluginCatalog.quickshop-hikari.pluginName == "QuickShop-Hikari";
        message = "Generated QuickShop-Hikari plugin entry should preserve its Bukkit runtime name";
      }
      {
        assertion = minecraft.paperPlugins.cfg.pluginCatalog.tradepost.pluginName == "TradePost";
        message = "Generated TradePost plugin entry should preserve its Bukkit runtime name";
      }
      {
        assertion = minecraft.paperPlugins.cfg.pluginCatalog.combatlogplugin.pluginName == "CombatLog";
        message = "Generated CombatLog plugin entry should preserve its Bukkit runtime name";
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
        assertion = bedrock.config.ix.image.name == "minecraft-bedrock";
        message = "minecraft-bedrock image should set the expected OCI image name";
      }
      {
        assertion = bedrock.config.ix.image.tag == "1.26.14.1";
        message = "minecraft-bedrock image tag should follow the pinned Bedrock server version";
      }
      {
        assertion = bedrock.cfg.enable;
        message = "minecraft-bedrock image should enable services.minecraft-bedrock";
      }
      {
        assertion = bedrock.cfg.settings."server-name" == "ix-powered Bedrock";
        message = "minecraft-bedrock should set the expected default server name";
      }
      {
        assertion =
          bedrock.cfg.settings."server-port" == bedrock.cfg.port
          && bedrock.cfg.settings."server-portv6" == bedrock.cfg.portv6;
        message = "minecraft-bedrock server.properties should follow the configured UDP ports";
      }
      {
        assertion =
          bedrock.config.networking.firewall.allowedUDPPorts == [
            bedrock.cfg.port
            bedrock.cfg.portv6
          ];
        message = "minecraft-bedrock firewall should open only the configured UDP ports";
      }
      {
        assertion = bedrock.service.unit.description == "Minecraft Bedrock server";
        message = "minecraft-bedrock should run a dedicated systemd service";
      }
      {
        assertion = lib.hasInfix "/bin/bedrock_server" bedrock.service.config.ExecStart;
        message = "minecraft-bedrock ExecStart should launch bedrock_server";
      }
      {
        assertion = bedrock.service.config.StateDirectory == "minecraft-bedrock";
        message = "minecraft-bedrock service should get a managed state directory";
      }
    ];

    hyperion = [
      {
        assertion = hyperion.config.ix.image.name == "hyperion";
        message = "hyperion image should set the expected OCI image name";
      }
      {
        assertion = hyperion.config.ix.image.tag == repoPackages.hyperion.version;
        message = "hyperion image tag should follow the pinned package version";
      }
      {
        assertion = hyperion.cfg.enable;
        message = "hyperion image should enable services.hyperion";
      }
      {
        assertion =
          lib.getName hyperion.cfg.package == lib.getName repoPackages.hyperion
          && hyperion.cfg.package.version == repoPackages.hyperion.version;
        message = "hyperion service should default to the repo-packaged Hyperion build";
      }
      {
        assertion =
          hyperion.service.unit.environment.BEDWARS_PROXY_ADDR == "0.0.0.0:25565"
          && hyperion.service.unit.environment.BEDWARS_IP == "127.0.0.1"
          && hyperion.service.unit.environment.BEDWARS_PORT == "35565";
        message = "hyperion service should wire the embedded proxy and Bedwars bind addresses";
      }
      {
        assertion =
          lib.hasInfix "/bin/bedwars" hyperion.service.config.ExecStart
          && lib.hasInfix "--root-ca-cert /var/lib/hyperion/root_ca.crt" hyperion.service.config.ExecStart;
        message = "hyperion service should launch bedwars with generated mTLS files";
      }
      {
        assertion =
          lib.hasInfix "subjectAltName=IP:127.0.0.1,DNS:localhost" hyperion.service.unit.preStart
          && lib.hasInfix "proxy_private_key.pem" hyperion.service.unit.preStart;
        message = "hyperion service should generate local CA, server, and proxy certificates";
      }
      {
        assertion = hyperion.config.networking.firewall.allowedTCPPorts == [ hyperion.cfg.proxy.port ];
        message = "hyperion image should expose the proxy port and keep the game port private";
      }
      {
        assertion =
          hyperion.config.ix.networking.portClaims.hyperion-proxy.port == hyperion.cfg.proxy.port
          && hyperion.config.ix.networking.portClaims.hyperion-bedwars.port == hyperion.cfg.game.port;
        message = "hyperion service should register proxy and game-server listener claims";
      }
    ];

    remote-desktop = [
      {
        assertion = remoteDesktop.config.ix.image.name == "ix-remote-desktop";
        message = "remote-desktop image should set the expected OCI image name";
      }
      {
        assertion = remoteDesktop.cfg.enable;
        message = "remote-desktop image should enable services.remote-desktop";
      }
      {
        assertion = lib.getName remoteDesktop.cfg.package == lib.getName pkgs.xpra;
        message = "remote-desktop should default to the nixpkgs Xpra package";
      }
      {
        assertion = remoteDesktop.cfg.port == 6080;
        message = "remote-desktop should expose the Xpra HTML5 client on port 6080";
      }
      {
        assertion = remoteDesktop.cfg.bindAddress == "0.0.0.0";
        message = "remote-desktop should bind browser clients on all interfaces by default";
      }
      {
        assertion = remoteDesktop.cfg.display == ":100";
        message = "remote-desktop should let Xpra own a deterministic virtual display";
      }
      {
        assertion = remoteDesktop.cfg.resolution == "1920x1080";
        message = "remote-desktop should default to a browser-friendly 1080p display";
      }
      {
        assertion = remoteDesktop.cfg.auth == "none";
        message = "remote-desktop should keep the current unauthenticated ix image contract explicit";
      }
      {
        assertion = remoteDesktop.service.unit.description == "Xpra remote desktop";
        message = "remote-desktop should run a single Xpra service";
      }
      {
        assertion = remoteDesktop.service.config.User == "remote-desktop";
        message = "remote-desktop service should run as its dedicated system user";
      }
      {
        assertion = remoteDesktop.service.config.StateDirectory == "remote-desktop";
        message = "remote-desktop service should get a managed state directory";
      }
      {
        assertion = remoteDesktop.service.config.RuntimeDirectory == "remote-desktop";
        message = "remote-desktop service should get a managed runtime directory";
      }
      {
        assertion = remoteDesktop.config.users.users.remote-desktop.isSystemUser;
        message = "remote-desktop user should be a system user";
      }
      {
        assertion = remoteDesktop.config.networking.firewall.allowedTCPPorts == [ remoteDesktop.cfg.port ];
        message = "remote-desktop firewall should open only the configured browser port";
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

    helpers = [
      {
        assertion = cargoUnitWorkspace.policyChecks ? unusedCrateDependencies;
        message = "cargo-unit workspaces should expose an unused dependency policy check by default";
      }
      {
        assertion = cargoUnitWorkspace.policyChecks ? cargoAudit;
        message = "cargo-unit workspaces should expose a cargo-audit policy check by default";
      }
      {
        assertion = cargoUnitWorkspace.policyChecks ? cargoClippy;
        message = "cargo-unit workspaces should expose a clippy policy check by default";
      }
      {
        assertion = cargoUnitWorkspace.policy.clippy.package.pname == "llm-clippy";
        message = "cargo-unit clippy checks should use llm-clippy by default";
      }
      {
        assertion =
          let
            denied = cargoUnitWorkspace.policy.clippy.deniedLints;
          in
          builtins.all (lint: builtins.elem lint denied) [
            "warnings"
            "clippy::all"
            "clippy::pedantic"
            "clippy::nursery"
            "clippy::cargo"
          ];
        message = "cargo-unit clippy checks should deny the shared strict lint set by default";
      }
      {
        assertion = cargoUnitWorkspace.policyChecks ? cargoMachete;
        message = "cargo-unit workspaces should expose a cargo-machete policy check by default";
      }
      {
        assertion = cargoUnitWorkspace.binaries.cargo-unit-hello ? unchecked;
        message = "cargo-unit package outputs should be wrapped by policy checks by default";
      }
      {
        assertion = builtins.all (binary: builtins.hasAttr binary cargoUnitBinaries) [
          "cargo-unit-goodbye"
          "cargo-unit-hello"
        ];
        message = "cargo-unit should build several binary roots from one workspace graph";
      }
      {
        assertion = builtins.hasAttr "cargo_unit_hello" cargoUnitTestWorkspace.tests;
        message = "cargo-unit workspaces should expose test targets as separate checks";
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
        assertion = repoPackages.minecraft-nbt.passthru.policyChecks ? cargoClippy;
        message = "repo Rust packages should expose clippy policy checks by default";
      }
      {
        assertion = repoPackages.minecraft-nbt.passthru.policy.clippy.package.pname == "llm-clippy";
        message = "repo Rust clippy checks should use llm-clippy by default";
      }
      {
        assertion =
          let
            denied = repoPackages.minecraft-nbt.passthru.policy.clippy.deniedLints;
          in
          builtins.all (lint: builtins.elem lint denied) [
            "warnings"
            "clippy::all"
            "clippy::pedantic"
            "clippy::nursery"
            "clippy::cargo"
          ];
        message = "repo Rust clippy checks should deny the shared strict lint set by default";
      }
      {
        assertion = repoPackages.minecraft-nbt.passthru.tests ? package;
        message = "repo Rust package builds should be exposed as flake-checkable tests";
      }
      {
        assertion =
          minecraft.config.ix.build.ociEfficiency.enable
          && minecraft.config.ix.build.ociEfficiency.minEfficiency == 0.95
          && minecraft.config.ix.build.ociEfficiency.maxWastedBytes == 20 * 1024 * 1024
          && minecraft.config.ix.build.ociEfficiency.maxWastedPercent == 0.20
          && minecraft.config.ix.build.ociEfficiency.reportTopPaths == 10;
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
      {
        assertion = pythonMcpServerPackage.meta.mainProgram == "ix-python-mcp";
        message = "python MCP server package should expose ix-python-mcp as its main program";
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
        assertion = fleetPlan.web.region == "hil-1";
        message = "fleet nodes should inherit the top-level deployment region";
      }
      {
        assertion = fleetPlan.web.tags == [ "public" ];
        message = "fleet wrapped-node tags should flow into the generated plan";
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
        assertion = fleet.health.meta.mainProgram == "ix-fleet-health";
        message = "fleet wrappers should expose an ix-fleet health command";
      }
      {
        assertion = !fleetIpv4HealthCheckEval.success;
        message = "fleet plans should reject host-side IPv4 checks on private nodes";
      }
      {
        assertion = fleet.planValue.secrets.sessionKey.generate;
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
    ];
  };

  # --- Per-image build-time checks ------------------------------------------

  buildScripts = {
    factions-server = ''
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

    survival-server = ''
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
      test -L /build/minecraft-datapack-data/custom/datapacks/max-height
      grep -q '"logical_height": 4064' /build/minecraft-datapack-data/custom/datapacks/max-height/data/minecraft/dimension_type/overworld.json
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
    ${lib.getExe pythonAppClosureProbe} > python-app-closure-probe.out
    grep -q 'python app source is in the runtime closure' python-app-closure-probe.out

    ${cargoUnitHello}/bin/cargo-unit-hello > cargo-unit-hello.out
    grep -q 'hello from cargo-unit' cargo-unit-hello.out
    ${cargoUnitBinaries.cargo-unit-goodbye}/bin/cargo-unit-goodbye > cargo-unit-goodbye.out
    grep -q 'goodbye from cargo-unit' cargo-unit-goodbye.out
    test -d ${cargoUnitTestWorkspace.tests.cargo_unit_hello}

    grep -q 'class="ix bun"' ${bunSite}/share/bun-site-fixture/index.html
    test -d ${bunSite.bunNodeModules}/node_modules/clsx
    test -x ${bunSite.bunNodeModules.nodeCompat}/bin/node

    ${uvApplication}/bin/uv-app-fixture > uv-app-fixture.out
    grep -q 'hello from uv app fixture' uv-app-fixture.out
    test -e ${uvApplication.uvWheelhouse}/click-8.1.7-py3-none-any.whl

    ${lib.getExe pythonMcpServerPackage} eval '1 + 2' > python-mcp-eval.out
    grep -q 'result:' python-mcp-eval.out
    grep -q '^3$' python-mcp-eval.out
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
