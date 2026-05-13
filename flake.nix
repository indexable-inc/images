{
  description = "Pre-built OCI images for ix VMs";

  inputs = {
    artifact-tonbo-artifacts = {
      url = "https://artifacts.tonbo.dev/release/e16636b0e5ce/artifacts";
      flake = false;
    };

    artifact-minecraft-server-26w17a-fabric = {
      url = "https://meta.fabricmc.net/v2/versions/loader/26.2-snapshot-5/0.19.2/1.1.1/server/jar";
      flake = false;
    };
    artifact-minecraft-server-26-2-snapshot-6-fabric = {
      url = "https://meta.fabricmc.net/v2/versions/loader/26.2-snapshot-6/0.19.2/1.1.1/server/jar";
      flake = false;
    };
    artifact-minecraft-server-26-1-2-fabric = {
      url = "https://meta.fabricmc.net/v2/versions/loader/26.1.2/0.19.2/1.1.1/server/jar";
      flake = false;
    };
    artifact-minecraft-server-1-21-11-fabric = {
      url = "https://meta.fabricmc.net/v2/versions/loader/1.21.11/0.19.2/1.1.1/server/jar";
      flake = false;
    };
    artifact-minecraft-server-1-21-11-paper = {
      url = "https://api.papermc.io/v2/projects/paper/versions/1.21.11/builds/69/downloads/paper-1.21.11-69.jar";
      flake = false;
    };

    artifact-minecraft-mod-almanac = {
      url = "https://cdn.modrinth.com/data/Gi02250Z/versions/7IRzJzBP/almanac-1.26.x-fabric-1.6.2.1.jar";
      flake = false;
    };
    artifact-minecraft-mod-bluemap = {
      url = "https://cdn.modrinth.com/data/swbUV1cr/versions/D9j76thC/bluemap-5.20-fabric.jar";
      flake = false;
    };
    artifact-minecraft-mod-c2me-fabric-26-2-snapshot-5 = {
      url = "https://cdn.modrinth.com/data/VSNURh3q/versions/h0G6V9wK/c2me-fabric-mc26.2-snapshot-5-0.3.7%2Balpha.0.68.jar";
      flake = false;
    };
    artifact-minecraft-mod-c2me-fabric-26-1-2 = {
      url = "https://cdn.modrinth.com/data/VSNURh3q/versions/utLSz8Lf/c2me-fabric-mc26.1.2-0.3.7%2Balpha.0.68.jar";
      flake = false;
    };
    artifact-minecraft-mod-chunky = {
      url = "https://cdn.modrinth.com/data/fALzjamp/versions/4Eotm6ov/Chunky-Fabric-1.5.3.jar";
      flake = false;
    };
    artifact-minecraft-mod-clumps = {
      url = "https://cdn.modrinth.com/data/Wnxd13zP/versions/RXNrUIjA/Clumps-fabric-26.1.2-26.1.2.1.jar";
      flake = false;
    };
    artifact-minecraft-mod-distanthorizons = {
      url = "https://cdn.modrinth.com/data/uCdwusMi/versions/FJrLlu3p/DistantHorizons-3.0.3-b-26.1.2-fabric-neoforge.jar";
      flake = false;
    };
    artifact-minecraft-mod-fabric-api-26-1-2 = {
      url = "https://cdn.modrinth.com/data/P7dR8mSH/versions/dZsorAUN/fabric-api-0.147.0%2B26.1.2.jar";
      flake = false;
    };
    artifact-minecraft-mod-fabric-api-1-21-11 = {
      url = "https://cdn.modrinth.com/data/P7dR8mSH/versions/i5tSkVBH/fabric-api-0.141.3%2B1.21.11.jar";
      flake = false;
    };
    artifact-minecraft-mod-fabric-api-26-2 = {
      url = "https://cdn.modrinth.com/data/P7dR8mSH/versions/kw0Rlte8/fabric-api-0.147.1%2B26.2.jar";
      flake = false;
    };
    artifact-minecraft-mod-ferrite-core = {
      url = "https://cdn.modrinth.com/data/uXXizFIs/versions/d5ddUdiB/ferritecore-9.0.0-fabric.jar";
      flake = false;
    };
    artifact-minecraft-mod-grimac = {
      url = "https://cdn.modrinth.com/data/LJNGWSvH/versions/65YzWD8i/grimac-fabric-2.3.74-ce86075.jar";
      flake = false;
    };
    artifact-minecraft-mod-krypton = {
      url = "https://cdn.modrinth.com/data/fQEb0iXm/versions/kYAGItyj/krypton-0.3.0.jar";
      flake = false;
    };
    artifact-minecraft-mod-ksyxis = {
      url = "https://cdn.modrinth.com/data/2ecVyZ49/versions/kL32PN9Q/Ksyxis-1.4.3.jar";
      flake = false;
    };
    artifact-minecraft-mod-lithium = {
      url = "https://cdn.modrinth.com/data/gvQqBUqZ/versions/R7MxYvuW/lithium-fabric-0.24.2%2Bmc26.1.2.jar";
      flake = false;
    };
    artifact-minecraft-mod-lithostitched = {
      url = "https://cdn.modrinth.com/data/XaDC71GB/versions/cHH1mPJL/lithostitched-1.7.2-fabric-26.1.jar";
      flake = false;
    };
    artifact-minecraft-mod-lmd = {
      url = "https://cdn.modrinth.com/data/vE2FN5qn/versions/eW5P1rHo/letmedespawn-1.26.x-fabric-1.6.2.1.jar";
      flake = false;
    };
    artifact-minecraft-mod-luckperms = {
      url = "https://cdn.modrinth.com/data/Vebnzrzj/versions/fTIdfb46/LuckPerms-Fabric-5.5.42.jar";
      flake = false;
    };
    artifact-minecraft-plugin-luckperms-bukkit = {
      url = "https://cdn.modrinth.com/data/Vebnzrzj/versions/OrIs0S6b/LuckPerms-Bukkit-5.5.17.jar";
      flake = false;
    };
    artifact-minecraft-mod-servercore = {
      url = "https://cdn.modrinth.com/data/4WWQxlQP/versions/P8k080Af/servercore-fabric-1.5.16%2B26.1.jar";
      flake = false;
    };
    artifact-minecraft-mod-simple-voice-chat = {
      url = "https://cdn.modrinth.com/data/9eGKb6K1/versions/gVPjsMto/voicechat-fabric-2.6.17%2B26.1.2.jar";
      flake = false;
    };
    artifact-minecraft-mod-spark = {
      url = "https://cdn.modrinth.com/data/l6YH9Als/versions/J1GUYyGQ/spark-1.10.172-fabric.jar";
      flake = false;
    };
    artifact-minecraft-mod-tectonic = {
      url = "https://cdn.modrinth.com/data/lWDHr9jE/versions/jL2ZsTzx/tectonic-3.0.22-fabric-26.1.jar";
      flake = false;
    };
    artifact-minecraft-mod-terrain-diffusion = {
      url = "https://github.com/xandergos/terrain-diffusion-mc/releases/download/v2.1.0/terrain-diffusion-mc-2.1.0-cpu%2B1.21.11.jar";
      flake = false;
    };
    artifact-minecraft-mod-terralith = {
      url = "https://cdn.modrinth.com/data/8oi3bsk5/versions/FCzSjHeG/Terralith_26.1_v2.6.2_Fabric.jar";
      flake = false;
    };
    artifact-minecraft-mod-vmp-fabric = {
      url = "https://cdn.modrinth.com/data/wnEe9KBa/versions/9f7J0dAp/vmp-fabric-mc26.1.2-0.2.0%2Bbeta.7.234-all.jar";
      flake = false;
    };
    artifact-minecraft-plugin-plugmanx = {
      url = "https://cdn.modrinth.com/data/yro4niHu/versions/hrMAp7Ww/PlugManX-3.0.4.jar";
      flake = false;
    };

    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    llm-agents = {
      url = "github:numtide/llm-agents.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    pre-commit-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{
      self,
      nixpkgs,
      llm-agents,
      pre-commit-hooks,
      ...
    }:
    let
      inherit (nixpkgs) lib;
      fs = lib.fileset;
      ix = import ./lib {
        inherit
          nixpkgs
          llm-agents
          ;
        artifactInputs = inputs;
        paths = {
          modules = ./modules;
          nixPackages = {
            minecraftHotReloadAgent = ./nix/packages/minecraft-hot-reload-agent.nix;
            minecraftRcon = ./nix/packages/minecraft-rcon.nix;
            tonboArtifacts = ./nix/packages/tonbo-artifacts.nix;
          };
          packages.minestom.servers.hello = ./packages/minestom/servers/hello;
          tools.ixFleet = ./tools/ix-fleet.py;
          tools.minecraftSyncManaged = ./nix/packages/minecraft-sync-managed.py;
        };
      };
      devSystems = [
        "x86_64-linux"
        "aarch64-darwin"
      ];
      imagePackages = (ix.discoverImages ./images) // {
        inherit (ix.pkgs) tonbo-artifacts;
      };
      lintSource = fs.toSource {
        root = ./.;
        fileset = fs.gitTracked ./.;
      };
      # Evaluated once per host system so packages and apps share the derivations.
      claudeCodeDemoFor =
        hostSystem:
        import ./examples/claude-code-demo/default.nix {
          ix = ix // {
            lib = ix;
          };
          inherit hostSystem;
        };
      claudeCodeDemos = lib.genAttrs devSystems claudeCodeDemoFor;
      preCommitCheckFor =
        system:
        pre-commit-hooks.lib.${system}.run {
          src = ./.;
          hooks.ix-lint = {
            enable = true;
            name = "ix lint";
            entry = self.apps.${system}.lint.program;
            pass_filenames = false;
            always_run = true;
          };
        };
    in
    {
      lib = ix;
      modules = import ./modules;
      overlays.default = ix.overlay;

      packages = lib.genAttrs devSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          claudeCodeDemo = claudeCodeDemos.${system};
          claudeCodeDemoImages = lib.mapAttrs' (
            name: package: lib.nameValuePair "claude-code-demo-${name}-image" package
          ) claudeCodeDemo.packages;
          repoPackages = ix.packageSetFor pkgs;
          claudeCodeDemoLinuxUp = ix.writeNushellApplication pkgs {
            name = "claude-code-demo-linux-up";
            runtimeInputs = [
              claudeCodeDemo.up
            ];
            text = ''
              def --wrapped main [...args] {
                exec ix-fleet-up --on linux ...$args
              }
            '';
          };
          claudeCodeDemoMinecraftUp = ix.writeNushellApplication pkgs {
            name = "claude-code-demo-minecraft-up";
            runtimeInputs = [
              claudeCodeDemo.up
            ];
            text = ''
              def --wrapped main [...args] {
                exec ix-fleet-up --on minecraft ...$args
              }
            '';
          };
        in
        imagePackages
        // claudeCodeDemo.systemPackages
        // claudeCodeDemoImages
        // {
          claude-code-demo-command = claudeCodeDemo.command;
          claude-code-demo-diff = claudeCodeDemo.diff;
          claude-code-demo-plan = claudeCodeDemo.planCommand;
          claude-code-demo-replace = claudeCodeDemo.replace;
          claude-code-demo-switch = claudeCodeDemo.switch;
          claude-code-demo-up = claudeCodeDemo.up;
          claude-code-demo-linux-up = claudeCodeDemoLinuxUp;
          claude-code-demo-minecraft-up = claudeCodeDemoMinecraftUp;
          minestom-hello-server-jar = repoPackages.minestom.helloServerJar;
        }
      );

      checks = lib.genAttrs devSystems (
        system:
        {
          pre-commit = preCommitCheckFor system;
        }
        // lib.optionalAttrs (system == ix.system) (
          let
            lint = self.apps.${ix.system}.lint.program;
          in
          {
            eval = import ./tests { inherit nixpkgs ix; };
            lint = ix.pkgs.runCommand "ix-images-lint" { nativeBuildInputs = [ ix.pkgs.coreutils ]; } ''
              cp -R ${lintSource} source
              chmod -R u+w source
              cd source
              ${lint}
              mkdir -p "$out"
            '';
          }
        )
      );

      formatter = lib.genAttrs devSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);

      templates.default = {
        path = ./template;
        description = "Starter ix image";
      };

      # Developer tooling. Exposed for both Linux CI and macOS dev machines.
      devShells = lib.genAttrs devSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          preCommitCheck = self.checks.${system}.pre-commit;
        in
        {
          default = pkgs.mkShell {
            packages = [
              pkgs.ast-grep
              pkgs.deadnix
              pkgs.gradle_9
              pkgs.jdk25
              pkgs.nixfmt
              pkgs.statix
            ]
            ++ preCommitCheck.enabledPackages;

            JAVA_HOME = pkgs.jdk25.home;
            inherit (preCommitCheck) shellHook;
          };
        }
      );

      apps = lib.genAttrs devSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          # Reuse derivations already built for packages to avoid evaluating them twice.
          claudeCodeDemo = claudeCodeDemos.${system};
          benchFilesystem = import ./bench/filesystem { inherit ix pkgs; };
          updateMods = ix.writeNushellApplication pkgs {
            name = "update-mods";
            runtimeInputs = [ pkgs.python3 ];
            text = ''
              def main [...args] {
                exec python3 ${./tools/update-mods.py} ...$args
              }
            '';
          };
          python = pkgs.python3.withPackages (ps: [ ps.pydantic ]);
          ixFleet = ix.writeNushellApplication pkgs {
            name = "ix-fleet";
            runtimeInputs = [ python ];
            text = ''
              def main [...args] {
                exec python3 ${./tools/ix-fleet.py} ...$args
              }
            '';
          };
          lint = ix.writeNushellApplication pkgs {
            name = "lint";
            runtimeInputs = [
              pkgs.ast-grep
              pkgs.deadnix
              pkgs.fd
              pkgs.nixfmt
              pkgs.statix
            ];
            text = ''
              def main [] {
                let nix_files = (fd --extension nix | lines)

                print "nixfmt"
                nixfmt --check ...$nix_files

                print "statix"
                statix check .

                print "deadnix"
                deadnix --fail --no-lambda-pattern-names .

                print "ast-grep"
                ast-grep scan --error .
              }
            '';
          };
        in
        {
          lint = {
            type = "app";
            program = lib.getExe lint;
            meta.description = "Run all Nix formatting and lint checks";
          };

          bench-filesystem = {
            type = "app";
            program = lib.getExe benchFilesystem;
            meta.description = "Benchmark file-system behavior from inside an ix VM";
          };

          update-mods = {
            type = "app";
            program = lib.getExe updateMods;
            meta.description = "Regenerate Minecraft mod catalogs";
          };

          ix-fleet = {
            type = "app";
            program = lib.getExe ixFleet;
            meta.description = "Render ix fleet plans and commands";
          };

          claude-code-demo-diff = {
            type = "app";
            program = lib.getExe claudeCodeDemo.diff;
            meta.description = "Diff the Claude Code demo fleet against live VMs";
          };

          claude-code-demo-plan = {
            type = "app";
            program = lib.getExe claudeCodeDemo.planCommand;
            meta.description = "Render the Claude Code demo fleet plan";
          };

          claude-code-demo-replace = {
            type = "app";
            program = lib.getExe claudeCodeDemo.replace;
            meta.description = "Build replacement images for the Claude Code demo fleet";
          };

          claude-code-demo-up = {
            type = "app";
            program = lib.getExe claudeCodeDemo.up;
            meta.description = "Build and upload demo OCI images, then create or start VMs from them";
          };

          # Reuse the wrappers already built for packages rather than rebuilding them.
          claude-code-demo-linux-up = {
            type = "app";
            program = lib.getExe self.packages.${system}.claude-code-demo-linux-up;
            meta.description = "Build and upload the Claude Code demo Linux image, then create or start only the Linux VM";
          };

          claude-code-demo-minecraft-up = {
            type = "app";
            program = lib.getExe self.packages.${system}.claude-code-demo-minecraft-up;
            meta.description = "Build and upload the Claude Code demo Minecraft image, then create or start only the Minecraft VM";
          };

          claude-code-demo-switch = {
            type = "app";
            program = lib.getExe claudeCodeDemo.switch;
            meta.description = "Switch the Claude Code demo fleet";
          };
        }
      );
    };
}
