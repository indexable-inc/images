{
  description = "Pre-built OCI images for ix VMs";

  # Keep the repo cache available for image closures and repo-owned tools that
  # CI has already built. Generic nixpkgs paths still substitute from
  # cache.nixos.org.
  nixConfig = {
    extra-substituters = [ "https://indexable-inc.cachix.org" ];
    extra-trusted-public-keys = [
      "indexable-inc.cachix.org-1:HQ5mjdOyhgNjLVhjv0qgVMJ5YiO1zEEVMAtF9mTcpiI="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    # Determinate Nix replaces the in-VM Nix CLI and daemon with the
    # Determinate Systems distribution: faster eval (lazy-trees), JSON
    # logging, and hash-mismatch auto-fixes. Pinned to major version 3;
    # routine bumps come from `nix flake update`.
    determinate.url = "https://flakehub.com/f/DeterminateSystems/determinate/3";

    # Home Manager wired in via its NixOS module for per-tool XDG-shaped
    # config (Nushell, atuin, zoxide, starship, ...). Tracks master so it
    # stays on the same release as nixpkgs-unstable; the per-release
    # branches lag (no release-26.05 exists at the time of writing) and
    # the mismatch fires a noisy `enableNixpkgsReleaseCheck` warning on
    # every eval.
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Nous Research's Hermes agent ships its own NixOS module
    # (`nixosModules.default`) and uv2nix-built Python closure. Pinned to
    # a release tag so routine bumps are review events; `nix flake update
    # hermes-agent` after bumping the tag is the supported intake path.
    # Surfaced through `ix.hermesAgent` and consumed by
    # `examples/hermes-agent/`.
    hermes-agent = {
      url = "github:NousResearch/hermes-agent/v2026.5.16";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      determinate,
      home-manager,
      hermes-agent,
      ...
    }:
    let
      inherit (nixpkgs) lib;

      # All path literals the flake exposes. Centralized so lib/ and
      # lib/per-system.nix have a single source of truth.
      paths = {
        root = ./.;
        agentsMd = ./agents-md;
        images = ./images;
        modules = ./modules;
        examples = ./examples;
        tests = ./tests;
        bench.filesystem = ./bench/filesystem;
        site = ./site;
        minecraftMods = ./images/games/minecraft/mods;
        minecraftPaperPlugins = ./images/games/minecraft/plugins/paper;
        minecraftVelocityPlugins = ./images/games/minecraft/plugins/velocity;
        packages = {
          agentsMd = ./packages/agents-md;
          ix = ./packages/ix;
          ixFleet = ./packages/ix-fleet;
          ixDevDiagnose = ./packages/ix-dev-diagnose;
          minecraft = {
            hotReloadAgent = ./packages/minecraft/hot-reload-agent;
            nbt = ./packages/minecraft/nbt;
            probe = ./packages/minecraft/probe;
            rcon = ./packages/minecraft/rcon;
            syncManaged = ./packages/minecraft/sync-managed;
          };
          dagRunner = ./packages/dag-runner;
          drgn = ./packages/drgn;
          llmClippy = ./packages/llm-clippy;
          minestom.servers.hello = ./packages/minestom/servers/hello;
          nixCargoUnit = ./packages/nix-cargo-unit;
          ociImageBuilder = ./packages/oci-image-builder;
          run = ./packages/run;
          mcp = ./packages/mcp;
          tonboArtifacts = ./packages/tonbo-artifacts;
          vineflower = ./packages/vineflower;
        };
        tools = {
          ixShellSyncIgnored = ./tools/ix-shell-sync-ignored.py;
          mcSource = ./tools/mc-source.nu;
          updateIxCli = ./tools/update-ix-cli.py;
          updateMods = ./tools/update-mods.py;
        };
      };

      ix = import ./lib {
        inherit
          nixpkgs
          paths
          rust-overlay
          determinate
          home-manager
          hermes-agent
          ;
      };
      devSystems = [
        "x86_64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      perSystem = lib.genAttrs devSystems (
        system:
        import ./lib/per-system.nix {
          inherit
            system
            ix
            nixpkgs
            paths
            rust-overlay
            ;
        }
      );
      collect = key: lib.mapAttrs (_: out: out.${key}) perSystem;
    in
    {
      lib = ix;
      inherit (ix) nixosModules;
      overlays.default = ix.overlay;
      packages = collect "packages";
      checks = collect "checks";
      formatter = collect "formatter";
    };
}
