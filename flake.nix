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

    # Relative-path ("subflake") inputs for the repo's independent data
    # subtrees. With lazy source trees a flake that reaches its whole tree via
    # `self` gives every package the entire repo as its source identity: any
    # file change anywhere re-hashes and re-copies the full tree per eval and
    # invalidates every dependent. Declaring each pure-data subtree as its own
    # `flake = false` path input scopes a consumer's source to just the subtree
    # it reads, so an edit under `site/` no longer perturbs a `skills`
    # package's drvPath. nix and nox both resolve these as lock nodes
    # `{ type = "path"; path = "./<dir>"; parent = []; }` against the parent
    # tree, with no separate fetch. Nix-code roots the flake itself imports
    # (`modules`, `packages`) stay ordinary relative paths: they are
    # import-time, not source identity. See ENG-2362.
    skills = {
      url = "path:./skills";
      flake = false;
    };
    agents = {
      url = "path:./agents";
      flake = false;
    };
    images = {
      url = "path:./images";
      flake = false;
    };
    examples = {
      url = "path:./examples";
      flake = false;
    };
    tests = {
      url = "path:./tests";
      flake = false;
    };
    bench-filesystem = {
      url = "path:./bench/filesystem";
      flake = false;
    };
    site = {
      url = "path:./site";
      flake = false;
    };

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

    btop-src = {
      url = "github:indexable-inc/btop/711f4a128b1b7009ee9cf0fa179a586c82586613";
      flake = false;
    };

    drgn-src = {
      url = "git+https://github.com/osandov/drgn?ref=refs/tags/v0.2.0&submodules=1";
      flake = false;
    };

    perftest-src = {
      url = "git+https://github.com/linux-rdma/perftest?ref=refs/tags/26.04.17";
      flake = false;
    };

    fff-src = {
      url = "github:dmtrKovalenko/fff/v0.9.1";
      flake = false;
    };

    launchk-src = {
      url = "github:mach-kernel/launchk/6f5f09e0dfa3fea662e859de5d7d49ac09a9dbe6";
      flake = false;
    };

    # snix (Rust reimplementation of Nix; TVL-style depot, no flake.nix) consumed
    # as a source tree so `packages/snix` builds its CLI through cargo-unit
    # instead of the upstream crate2nix `Cargo.nix`. The Cargo workspace lives in
    # the repo's `snix/` subdirectory. Pinned in flake.lock; `nix flake update
    # snix-src` to bump.
    #
    # `shallow=1` is load-bearing, not cosmetic: only the source tree at the
    # pinned rev is ever used (`ix.snixSrc` -> `packages/nix/snix`), never git
    # history or `revCount`. Without it the lock records `revCount`, which forces
    # Nix to clone snix's entire ~22k-commit history (~500 MB) to materialize the
    # input. nix-direnv's `use flake` then runs `nix flake archive` on every cold
    # load (it gc-roots every input), so that full clone ran on each fresh
    # `direnv` load and hung the shell for minutes. git.snix.dev serves an
    # arbitrary SHA at depth 1, so the shallow fetch grabs just the pinned commit
    # (~2 s) even after `canon` has moved ahead of the pin.
    snix-src = {
      url = "git+https://git.snix.dev/snix/snix?ref=canon&shallow=1";
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

    # TODO: re-add the `symphony` flake input that provided
    # `pkgs.symphony-room-server` for images/dev/symphony-codex. room-server's
    # real home is the ix monorepo (`crates/room`,
    # `ix#packages.x86_64-linux.room-server`), but ix already inputs index
    # (`ix/flake.nix`), so index cannot source it from ix without a circular
    # flake dependency. Pin removed for now; re-add once that cycle is resolved
    # or the symphony-codex image moves into ix.

    # Ghostty's terminal VT engine, consumed as a source tree (not a flake) so
    # `packages/tui/vt/libghostty-vt` owns the build. Pinned to the commit the
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
      self,
      nixpkgs,
      rust-overlay,
      home-manager,
      hermes-agent,
      btop-src,
      drgn-src,
      perftest-src,
      fff-src,
      launchk-src,
      snix-src,
      clippy-fork,
      ghostty,
      skills,
      agents,
      images,
      examples,
      tests,
      bench-filesystem,
      site,
      ...
    }:
    let
      inherit (nixpkgs) lib;

      # The flake's own source revision, threaded into `ix` so packages can
      # stamp the running build (e.g. the MCP server reports it as its
      # `serverInfo.version`). Clean tree -> the commit hash; dirty tree ->
      # `<commit>-dirty`; neither (eval from a non-git source) -> "dev".
      rev = self.rev or self.dirtyRev or "dev";

      # Commit time of that revision as unix epoch seconds, threaded alongside
      # `rev` so a build can stamp a human date and relative age. Under
      # reproducible builds there is no wall-clock compile time; this is the
      # source date Nix already records (`self.lastModified`): the commit time
      # on a clean tree, the working-tree mtime on a dirty one. `0` when
      # evaluated from a non-git source.
      revEpoch = self.lastModified or 0;

      # All path literals the flake exposes. Centralized so lib/ and
      # lib/per-system.nix have a single source of truth.
      #
      # The data-subtree entries below resolve to the `outPath` of a
      # relative-path input (declared `flake = false` above) instead of a
      # bare `./<dir>` literal, so each consumer's source identity is scoped to
      # just that subtree. The shape and names of `paths` are unchanged, so no
      # downstream lib code needs editing. Nix-code roots the flake imports
      # directly (`modules`, `packagesRoot`) and the whole-repo `root` (the lint
      # source intentionally covers the entire tree) stay ordinary relative
      # paths: those are import-time / whole-repo by design, not per-subtree
      # source identity. The minecraft sub-paths are projections of the `images`
      # subtree, so they ride the same `images` input rather than each opening a
      # new whole-repo dependency.
      paths = {
        root = ./.;
        skills = skills.outPath;
        agents = agents.outPath;
        images = images.outPath;
        modules = ./modules;
        examples = examples.outPath;
        tests = tests.outPath;
        bench.filesystem = bench-filesystem.outPath;
        site = site.outPath;
        packagesRoot = ./packages;
        minecraftMods = images.outPath + "/games/minecraft/mods";
        minecraftPaperPlugins = images.outPath + "/games/minecraft/plugins/paper";
        minecraftVelocityPlugins = images.outPath + "/games/minecraft/plugins/velocity";
        minecraftLoaders = {
          paper = images.outPath + "/games/minecraft/loaders/paper";
          velocity = images.outPath + "/games/minecraft/loaders/velocity";
          fabric = images.outPath + "/games/minecraft/loaders/fabric";
        };
        tools = {
          ixShellSyncIgnored = ./tools/ix-shell-sync-ignored.py;
          mcSource = ./tools/mc-source.nu;
          updateSounds = ./tools/update-sounds.nu;
          updateLoaders = ./tools/update-loaders.py;
          updateMods = ./tools/update-mods.py;
        };
      };

      ix = import ./lib {
        inherit
          rev
          revEpoch
          nixpkgs
          paths
          rust-overlay
          home-manager
          hermes-agent
          btop-src
          drgn-src
          perftest-src
          fff-src
          launchk-src
          snix-src
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
        # Reusable workstation module (macOS): declare Raycast Focus session
        # defaults (title, filter mode, duration) and have them written to the
        # com.raycast.macos defaults domain at switch time. Import it and set
        # `programs.raycast.focus = { enable = true; ... }`. See
        # modules/home/raycast.nix.
        raycast = ./modules/home/raycast.nix;
        # Personal-but-shareable workstation module for github:andrewgazelka: the
        # ix.dev downtime watcher + boss bar overlay + the shared say-detached
        # sound helper, all as portable services. Closed over the per-system
        # flake packages so it resolves bossbar / minecraft-sound for the host it
        # runs on. See users/andrewgazelka/home.nix.
        andrewgazelka = import ./users/andrewgazelka/home.nix {
          indexPackages = system: (collect "packages").${system};
          portableServicesModule = ix.portableServices.homeModule;
          inherit ix;
        };
        # Reusable workstation module: draw one Minecraft boss bar per in-flight
        # GitHub Actions run across a set of repos (green = running, filled by
        # elapsed / average duration; purple = queued/unpicked). Import it and set
        # `services.ciBars = { enable = true; repos = [ ... ]; }`. Closed over the
        # per-system packages so it resolves the `bossbar` CLI for the host. See
        # packages/minecraft/bossbar-overlay/ci-bars-home-module.nix.
        ci-bars = import ./packages/minecraft/bossbar-overlay/ci-bars-home-module.nix {
          indexPackages = system: (collect "packages").${system};
          portableServicesModule = ix.portableServices.homeModule;
          inherit ix;
        };
        # Workstation-facing module to sync corpus sources (agent/shell history,
        # Slack/Linear exports, git repos) to an S3/R2 parquet archive and/or
        # Mixedbread, as a portable timer service. Closed over the per-system
        # packages so it resolves the `indexer` for the host. See
        # packages/search/indexer/home-module.nix.
        indexer = import ./packages/search/indexer/home-module.nix {
          indexPackages = system: (collect "packages").${system};
          portableServicesModule = ix.portableServices.homeModule;
        };
      };
      overlays.default = ix.overlay;
      templates.dev = {
        path = ./templates/dev;
        description = "Forkable ix dev environment: one dev.nix for a default VM, a fleet, and shared Claude/ix auth (RFC 0007)";
      };
      packages = collect "packages";
      checks = collect "checks";
      # Sharded keying of the same check derivations for the memory-bounded CI
      # evaluator (the `.#check` gate and blast-radius); see lib/per-system.nix
      # (ENG-2201). Kept separate from `checks` because its per-package
      # `recurseForDerivations` groups are not derivations, which the flake
      # `checks` schema requires.
      ciChecks = collect "ciChecks";
      formatter = collect "formatter";
      apps = collect "apps";
      devShells = collect "devShells";
    };
}
