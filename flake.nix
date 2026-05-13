{
  description = "Pre-built OCI images for ix VMs";

  inputs = {
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
    {
      self,
      nixpkgs,
      llm-agents,
      pre-commit-hooks,
    }:
    let
      inherit (nixpkgs) lib;
      fs = lib.fileset;
      ix = import ./lib {
        inherit
          nixpkgs
          llm-agents
          ;
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
