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

    # Fork of rust-lang/rust-clippy with extra restriction lints tuned for
    # LLM-assisted codebases. Pinned in flake.lock so `nix flake update`
    # bumps it; consumed as the source tree for `packages/llm-clippy`.
    clippy-fork = {
      url = "github:indexable-inc/clippy";
      flake = false;
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

    symphony = {
      url = "github:indexable-inc/symphony/main";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rust-overlay.follows = "rust-overlay";
    };

    # Ghostty's terminal VT engine, consumed as a source tree (not a flake) so
    # `packages/vt/libghostty-vt` owns the build. Pinned to the commit the
    # local clone validated against; `requireZig` in `build.zig.zon` is exact
    # minor (0.15.x), so the build uses `pkgs.zig_0_15`. The pinned tree ships
    # `build.zig.zon.nix` (zon2nix output), which vendors every lazy Zig
    # dependency with SRI hashes for a network-free build. Bump by repointing
    # this rev and running `nix flake update ghostty`.
    ghostty = {
      url = "github:ghostty-org/ghostty/fd49716ea2084108aa098db390931c007495a1ab";
      flake = false;
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      home-manager,
      hermes-agent,
      symphony,
      clippy-fork,
      ghostty,
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
        packagesRoot = ./packages;
        minecraftMods = ./images/games/minecraft/mods;
        minecraftPaperPlugins = ./images/games/minecraft/plugins/paper;
        minecraftVelocityPlugins = ./images/games/minecraft/plugins/velocity;
        minecraftLoaders = {
          paper = ./images/games/minecraft/loaders/paper;
          velocity = ./images/games/minecraft/loaders/velocity;
          fabric = ./images/games/minecraft/loaders/fabric;
        };
        tools = {
          ixShellSyncIgnored = ./tools/ix-shell-sync-ignored.py;
          mcSource = ./tools/mc-source.nu;
          updateSounds = ./tools/update-sounds.nu;
          updateIxCli = ./tools/update-ix-cli.py;
          updateLoaders = ./tools/update-loaders.py;
          updateMods = ./tools/update-mods.py;
        };
      };

      ix = import ./lib {
        inherit
          nixpkgs
          paths
          rust-overlay
          home-manager
          hermes-agent
          symphony
          clippy-fork
          ghostty
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
