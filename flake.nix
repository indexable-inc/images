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
    # The rust workspace units default to `contentAddressed = true`
    # (lib/rust/cargo-unit.nix), so evaluating `.#checks` / `.#packages`
    # resolves floating content-addressed derivations. Without this feature the
    # evaluator aborts with "experimental Nix feature 'ca-derivations' is
    # disabled". Declared here so any eval against this flake (CI's
    # `accept-flake-config` runs, a local `nix flake check`, `nix build
    # .#checks.<sys>.<name>`) picks it up from one source of truth.
    extra-experimental-features = [ "ca-derivations" ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

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
        agentContext = ./agent-context;
        skills = ./skills;
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
        "aarch64-linux"
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
      darwinModules = {
        # Personal-but-shareable nix-darwin module for github:andrewgazelka: the
        # Homebrew package set (GUI casks, the `mas` brew, Mac App Store apps).
        # Companion to homeModules.andrewgazelka (which owns the home-manager
        # services); import it from a darwin host to get the casks merged in. See
        # users/andrewgazelka/darwin.nix.
        andrewgazelka = ./users/andrewgazelka/darwin.nix;
      };
      homeModules = {
        # Workstation-facing home-manager module: declare a service once, get a
        # native launchd agent on macOS and native systemd user units on Linux.
        portable-services = ix.portableServices.homeModule;
        # Declarative-but-writable JSON config files (last-applied 3-way merge),
        # for config an app rewrites at runtime. See lib/mutable-json.nix.
        mutable-json = ix.mutableJson.homeModule;
        # Personal-but-shareable workstation module for github:andrewgazelka: the
        # ix.dev downtime watcher + boss bar overlay + the shared say-detached
        # sound helper, all as portable services. Closed over the per-system
        # flake packages so it resolves bossbar / minecraft-sound for the host it
        # runs on. See users/andrewgazelka/home.nix.
        andrewgazelka = import ./users/andrewgazelka/home.nix {
          indexPackages = system: (collect "packages").${system};
          portableServicesModule = ix.portableServices.homeModule;
        };
        # Reusable workstation module: draw one Minecraft boss bar per in-flight
        # GitHub Actions run across a set of repos (green = running, filled by
        # elapsed / average duration; purple = queued/unpicked). Import it and set
        # `services.ciBars = { enable = true; repos = [ ... ]; }`. Closed over the
        # per-system packages so it resolves the `bossbar` CLI for the host. See
        # packages/bossbar-overlay/ci-bars-home-module.nix.
        ci-bars = import ./packages/bossbar-overlay/ci-bars-home-module.nix {
          indexPackages = system: (collect "packages").${system};
          portableServicesModule = ix.portableServices.homeModule;
        };
        # Workstation-facing module to sync corpus sources (agent/shell history,
        # Slack/Linear exports, git repos) to an S3/R2 parquet archive and/or
        # Mixedbread, as a portable timer service. Closed over the per-system
        # packages so it resolves the `indexer` for the host. See
        # packages/indexer/home-module.nix.
        indexer = import ./packages/indexer/home-module.nix {
          indexPackages = system: (collect "packages").${system};
          portableServicesModule = ix.portableServices.homeModule;
        };
      };
      overlays.default = ix.overlay;
      packages = collect "packages";
      checks = collect "checks";
      formatter = collect "formatter";
      apps = collect "apps";
      devShells = collect "devShells";
    };
}
