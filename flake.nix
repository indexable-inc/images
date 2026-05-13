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
      ix = import ./lib {
        inherit nixpkgs llm-agents;
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
      perSystem = lib.genAttrs devSystems (
        system:
        import ./lib/per-system.nix {
          inherit
            self
            system
            ix
            nixpkgs
            pre-commit-hooks
            ;
          repoRoot = ./.;
          examplePaths.claudeCodeDemo = ./examples/claude-code-demo;
        }
      );
      collect = key: lib.mapAttrs (_: out: out.${key}) perSystem;
    in
    {
      lib = ix;
      modules = import ./modules;
      overlays.default = ix.overlay;
      templates.default = {
        path = ./template;
        description = "Starter ix image";
      };
      packages = collect "packages";
      apps = collect "apps";
      checks = collect "checks";
      devShells = collect "devShells";
      formatter = collect "formatter";
    };
}
