{
  description = "Pre-built OCI images for ix VMs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    llm-agents = {
      url = "github:numtide/llm-agents.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      llm-agents,
      ...
    }:
    let
      inherit (nixpkgs) lib;

      # All path literals the flake exposes. Centralized so lib/ and
      # lib/per-system.nix have a single source of truth.
      paths = {
        root = ./.;
        images = ./images;
        modules = ./modules;
        tests = ./tests;
        bench.filesystem = ./bench/filesystem;
        examples.claudeCodeDemo = ./examples/claude-code-demo;
        nixPackages = {
          minecraftHotReloadAgent = ./nix/packages/minecraft-hot-reload-agent.nix;
          minecraftRcon = ./nix/packages/minecraft-rcon.nix;
          tonboArtifacts = ./nix/packages/tonbo-artifacts.nix;
        };
        packages.minestom.servers.hello = ./packages/minestom/servers/hello;
        tools = {
          ixFleet = ./tools/ix-fleet.py;
          minecraftSyncManaged = ./nix/packages/minecraft-sync-managed.py;
          updateMods = ./tools/update-mods.py;
        };
      };

      ix = import ./lib {
        inherit nixpkgs llm-agents paths;
      };
      devSystems = [
        "x86_64-linux"
        "aarch64-darwin"
      ];
      perSystem = lib.genAttrs devSystems (
        system:
        import ./lib/per-system.nix {
          inherit
            system
            ix
            nixpkgs
            paths
            ;
        }
      );
      collect = key: lib.mapAttrs (_: out: out.${key}) perSystem;
    in
    {
      lib = ix;
      nixosModules = import ./modules;
      overlays.default = ix.overlay;
      templates.default = {
        path = ./template;
        description = "Starter ix image";
      };
      packages = collect "packages";
      apps = collect "apps";
      checks = collect "checks";
      formatter = collect "formatter";
    };
}
