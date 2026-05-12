{
  nixpkgs,
  ix,
}:
let
  inherit (nixpkgs) lib;
  inherit (ix) system;
  inherit (ix) pkgs;

  moduleList = lib.collect builtins.isPath (import ../modules);
  versions = import ../images/games/minecraft/versions.nix {
    inherit lib;
    inherit (ix) artifacts;
  };
  defaultMinecraftVersion = versions.default;
  defaultMinecraftModule = versions.${defaultMinecraftVersion};
  minecraftVersions = builtins.attrNames (builtins.removeAttrs versions [ "default" ]);

  evalConfig =
    modules:
    (lib.nixosSystem {
      inherit system;
      specialArgs.ix = {
        inherit (ix)
          artifacts
          mkMinecraftLoader
          mkMinecraftSyncManaged
          writeNushellApplication
          writePythonApplication
          ;
      };
      modules = [
        { nixpkgs.overlays = ix.overlays; }
        ../lib/ix-platform.nix
        ../lib/ix-oci-layer.nix
      ]
      ++ moduleList
      ++ modules;
    }).config;

  minecraftConfig = evalConfig [
    ../images/games/minecraft
    defaultMinecraftModule
  ];

  minecraftService = minecraftConfig.systemd.services.minecraft.serviceConfig;
  minecraftUnit = minecraftConfig.systemd.services.minecraft;
  minecraftExec = minecraftService.ExecStart;

  paperConfig = evalConfig [
    ../images/games/minecraft
    versions."1.21.11-paper"
  ];
  paperCfg = paperConfig.services.minecraft;
  paperService = paperConfig.systemd.services.minecraft;
  paperServiceConfig = paperService.serviceConfig;

  bedrockConfig = evalConfig [ ../images/games/minecraft-bedrock ];
  bedrockCfg = bedrockConfig.services.minecraft-bedrock;
  bedrockService = bedrockConfig.systemd.services.minecraft-bedrock;
  bedrockServiceConfig = bedrockService.serviceConfig;

  remoteDesktopConfig = evalConfig [ ../images/desktop/remote-desktop ];
  remoteDesktopCfg = remoteDesktopConfig.services.remote-desktop;
  remoteDesktopService = remoteDesktopConfig.systemd.services.remote-desktop;
  remoteDesktopServiceConfig = remoteDesktopService.serviceConfig;

  kernelDevConfig = evalConfig [ ../images/dev/kernel-dev ];

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
  claudeCodeDemo = import ../examples/claude-code-demo/default.nix {
    ix = ix // {
      lib = ix;
    };
  };

  packageNames = builtins.attrNames (ix.discoverImages ../images);

  expectedPackages = [
    "kernel-dev"
    "minecraft"
    "minecraft-bedrock"
  ]
  ++ map (version: "minecraft_${version}") minecraftVersions
  ++ [
    "minestom"
    "neovim-ci"
    "remote-desktop"
  ];

  assertions = [
    {
      assertion = packageNames == expectedPackages;
      message = "image discovery package set changed: expected ${builtins.toJSON expectedPackages}, got ${builtins.toJSON packageNames}";
    }
    {
      assertion = kernelDevConfig.ix.image.name == "linux-kernel-dev";
      message = "kernel-dev image should set the expected OCI image name";
    }
    {
      assertion = kernelDevConfig.services.git-clone.enable;
      message = "kernel-dev image should enable first-boot git cloning";
    }
    {
      assertion = kernelDevConfig.services.git-clone.url == "https://github.com/torvalds/linux.git";
      message = "kernel-dev image should clone the Linux repository";
    }
    {
      assertion = kernelDevConfig.services.git-clone.dest == "/src/linux";
      message = "kernel-dev image should clone Linux into /src/linux";
    }
    {
      assertion = minecraftConfig.ix.image.tag == defaultMinecraftVersion;
      message = "default minecraft image tag should follow versions.nix default";
    }
    {
      assertion = defaultMinecraftVersion == "26.1.2-fabric";
      message = "default minecraft image should use the stable 26.1.2 Fabric variant";
    }
    {
      assertion = lib.all (slug: builtins.hasAttr slug minecraftConfig.services.minecraft.mods) [
        "fabric-api"
        "lithium"
        "c2me-fabric"
        "spark"
        "grimac"
      ];
      message = "default minecraft image should include the 26.1.2 Fabric server mod set";
    }
    {
      assertion = minecraftConfig.services.minecraft.javaPackage == pkgs.jetbrains.jdk-no-jcef;
      message = "default Fabric minecraft should use JetBrains Runtime for enhanced class redefinition";
    }
    {
      assertion = lib.hasInfix "/bin/java" minecraftExec;
      message = "minecraft ExecStart should launch Java";
    }
    {
      assertion = lib.hasInfix "-XX:MaxRAMPercentage=85" minecraftExec;
      message = "minecraft should use MaxRAMPercentage for auto-scaling heap";
    }
    {
      assertion = lib.hasInfix "-XX:+UseG1GC" minecraftExec;
      message = "minecraft should include the default modern server GC flags";
    }
    {
      assertion = lib.hasInfix "-jar" minecraftExec && lib.hasInfix "nogui" minecraftExec;
      message = "minecraft ExecStart should launch the configured server jar in nogui mode";
    }
    {
      assertion = lib.hasInfix "minecraft-hot-reload-agent.jar=socket=/run/minecraft-hot-reload/socket" minecraftExec;
      message = "Fabric minecraft should start the hot reload Java agent";
    }
    {
      assertion = lib.hasInfix "-XX:+AllowEnhancedClassRedefinition" minecraftExec;
      message = "Fabric minecraft should enable JBR enhanced class redefinition";
    }
    {
      assertion = minecraftService.RuntimeDirectory == "minecraft-hot-reload";
      message = "Fabric minecraft should create a runtime directory for the hot reload socket";
    }
    {
      assertion = builtins.length minecraftUnit.reloadTriggers == 3;
      message = "minecraft managed files should trigger systemd reloads rather than unit restarts";
    }
    {
      assertion = lib.hasInfix "minecraft-sync-managed" minecraftUnit.preStart;
      message = "minecraft preStart should sync managed files from /etc";
    }
    {
      assertion = !(lib.hasInfix "fabric-api" minecraftUnit.preStart);
      message = "minecraft preStart should not embed managed mod store paths in the unit";
    }
    {
      assertion = paperCfg.dropDir == "plugins";
      message = "Paper minecraft should use the plugins drop directory";
    }
    {
      assertion = builtins.length paperService.reloadTriggers == 3;
      message = "Paper minecraft managed plugins should trigger systemd reloads";
    }
    {
      assertion = !(paperServiceConfig ? RuntimeDirectory);
      message = "Paper minecraft should not start the JVM hot reload socket";
    }
    {
      assertion = paperCfg.autoReload.rconPasswordFile == "/var/lib/minecraft/.ix-rcon-password";
      message = "Paper minecraft should use a state-local RCON password file";
    }
    {
      assertion = !(paperCfg.serverFiles."server.properties" ? "rcon.password");
      message = "Paper minecraft should not put the RCON password in Nix-managed server.properties";
    }
    {
      assertion = paperConfig.networking.firewall.allowedTCPPorts == [ paperCfg.port ];
      message = "Paper minecraft should not expose the local RCON reload port through the firewall";
    }
    {
      assertion = bedrockConfig.ix.image.name == "minecraft-bedrock";
      message = "minecraft-bedrock image should set the expected OCI image name";
    }
    {
      assertion = bedrockConfig.ix.image.tag == "1.26.14.1";
      message = "minecraft-bedrock image tag should follow the pinned Bedrock server version";
    }
    {
      assertion = bedrockCfg.enable;
      message = "minecraft-bedrock image should enable services.minecraft-bedrock";
    }
    {
      assertion = bedrockCfg.settings."server-name" == "ix-powered Bedrock";
      message = "minecraft-bedrock should set the expected default server name";
    }
    {
      assertion =
        bedrockCfg.settings."server-port" == bedrockCfg.port
        && bedrockCfg.settings."server-portv6" == bedrockCfg.portv6;
      message = "minecraft-bedrock server.properties should follow the configured UDP ports";
    }
    {
      assertion =
        bedrockConfig.networking.firewall.allowedUDPPorts == [
          bedrockCfg.port
          bedrockCfg.portv6
        ];
      message = "minecraft-bedrock firewall should open only the configured UDP ports";
    }
    {
      assertion = bedrockService.description == "Minecraft Bedrock server";
      message = "minecraft-bedrock should run a dedicated systemd service";
    }
    {
      assertion = lib.hasInfix "/bin/bedrock_server" bedrockServiceConfig.ExecStart;
      message = "minecraft-bedrock ExecStart should launch bedrock_server";
    }
    {
      assertion = bedrockServiceConfig.StateDirectory == "minecraft-bedrock";
      message = "minecraft-bedrock service should get a managed state directory";
    }
    {
      assertion = remoteDesktopConfig.ix.image.name == "ix-remote-desktop";
      message = "remote-desktop image should set the expected OCI image name";
    }
    {
      assertion = remoteDesktopCfg.enable;
      message = "remote-desktop image should enable services.remote-desktop";
    }
    {
      assertion = remoteDesktopCfg.package == pkgs.xpra;
      message = "remote-desktop should default to the nixpkgs Xpra package";
    }
    {
      assertion = remoteDesktopCfg.port == 6080;
      message = "remote-desktop should expose the Xpra HTML5 client on port 6080";
    }
    {
      assertion = remoteDesktopCfg.bindAddress == "0.0.0.0";
      message = "remote-desktop should bind browser clients on all interfaces by default";
    }
    {
      assertion = remoteDesktopCfg.display == ":100";
      message = "remote-desktop should let Xpra own a deterministic virtual display";
    }
    {
      assertion = remoteDesktopCfg.resolution == "1920x1080";
      message = "remote-desktop should default to a browser-friendly 1080p display";
    }
    {
      assertion = remoteDesktopCfg.auth == "none";
      message = "remote-desktop should keep the current unauthenticated ix image contract explicit";
    }
    {
      assertion = remoteDesktopService.description == "Xpra remote desktop";
      message = "remote-desktop should run a single Xpra service";
    }
    {
      assertion = remoteDesktopServiceConfig.User == "remote-desktop";
      message = "remote-desktop service should run as its dedicated system user";
    }
    {
      assertion = remoteDesktopServiceConfig.StateDirectory == "remote-desktop";
      message = "remote-desktop service should get a managed state directory";
    }
    {
      assertion = remoteDesktopServiceConfig.RuntimeDirectory == "remote-desktop";
      message = "remote-desktop service should get a managed runtime directory";
    }
    {
      assertion = remoteDesktopConfig.users.users.remote-desktop.isSystemUser;
      message = "remote-desktop user should be a system user";
    }
    {
      assertion = remoteDesktopConfig.networking.firewall.allowedTCPPorts == [ remoteDesktopCfg.port ];
      message = "remote-desktop firewall should open only the configured browser port";
    }
    {
      assertion = !(remoteDesktopConfig.systemd.services ? xvfb);
      message = "remote-desktop should not use a standalone Xvfb service";
    }
    {
      assertion = !(remoteDesktopConfig.systemd.services ? x11vnc);
      message = "remote-desktop should not use x11vnc";
    }
    {
      assertion = !(remoteDesktopConfig.systemd.services ? novnc);
      message = "remote-desktop should not use a separate noVNC websockify service";
    }
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
        fleetPlan.web.bootstrapImage == "registry.ix.dev/ix/test-cluster-bootstrap:zstd-tools-2026-05-12";
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
    {
      assertion =
        claudeCodeDemo.planValue.order == [
          "demo"
          "minecraft"
        ];
      message = "Claude Code demo should evaluate to the demo and Minecraft nodes";
    }
    {
      assertion =
        claudeCodeDemo.planValue.nodes.demo.l7ProxyPorts == [ 80 ]
        && claudeCodeDemo.nodes.demo.services.nginx.enable
        && claudeCodeDemo.nodes.demo.services.git-clone.dest == "/src/linux";
      message = "Claude Code demo should host the Svelte stats page and Linux source on the demo VM";
    }
    {
      assertion =
        claudeCodeDemo.nodes.minecraft.services.minecraft.fabric.enable
        && claudeCodeDemo.nodes.minecraft.services.minecraft.fabric.version == "26.2-snapshot-6"
        &&
          claudeCodeDemo.nodes.minecraft.services.minecraft.serverFiles."server.properties".gamemode
          == "creative"
        &&
          claudeCodeDemo.nodes.minecraft.services.minecraft.serverFiles."server.properties".level-type
          == "minecraft:flat"
        && claudeCodeDemo.planValue.nodes.minecraft.ipv4;
      message = "Claude Code demo should use the current pinned Fabric snapshot for the TNT world";
    }
  ];

  failures = map (test: test.message) (lib.filter (test: !test.assertion) assertions);
in
assert lib.assertMsg (failures == [ ]) (lib.concatStringsSep "\n" failures);
pkgs.runCommand "ix-images-eval-tests" { nativeBuildInputs = [ pkgs.gnugrep ]; } ''
  grep -q 'ignored-plugins' ${
    paperConfig.environment.etc."minecraft/managed-server-files".source
  }/plugins/PlugManX/config.yml
  grep -q 'PlugManX' ${
    paperConfig.environment.etc."minecraft/managed-server-files".source
  }/plugins/PlugManX/config.yml
  ! grep -R 'rcon.password' ${paperConfig.environment.etc."minecraft/managed-server-files".source}
  grep -q '^almanac$' ${
    paperConfig.environment.etc."minecraft/managed-dropins".source
  }/almanac.jar.plugin-name
  grep -q '^PlugManX$' ${
    paperConfig.environment.etc."minecraft/managed-dropins".source
  }/PlugManX.jar.plugin-name

  grep -q -- '--password-file "/var/lib/minecraft/.ix-rcon-password"' ${paperServiceConfig.ExecReload}
  grep -q 'plugman $row.action $row.plugin' ${paperServiceConfig.ExecReload}
  ! grep -q 'reload all' ${paperServiceConfig.ExecReload}

  mkdir -p "$out"
''
