{
  description = "Index developer tools, modules, and fleet examples";

  # Keep the repo cache available for repo-owned tools that CI has already built.
  # `cache.ix.dev` is the ix public cache (ncps fronting the `ix-public` atticd
  # cache); it serves repo-owned artifacts and falls through to cache.nixos.org
  # for generic nixpkgs paths, so a single substituter covers both. Everything
  # there is signed `ix-workspace:` (atticd signs served narinfos server-side),
  # so that one trusted key verifies both ix's builds and index's published
  # packages (pushed by cache-push.yml).
  nixConfig = {
    extra-substituters = ["https://cache.ix.dev"];
    extra-trusted-public-keys = [
      "ix-workspace:JuAaeOPfR3GL3nUICpEz/88/+S3BzGF3L6bPYFy0GwI="
    ];
    # The rust workspace units default to `contentAddressed = true`
    # (lib/rust/cargo-unit.nix), so evaluating `.#checks` / `.#packages`
    # resolves floating content-addressed derivations. Without this feature the
    # evaluator aborts with "experimental Nix feature 'ca-derivations' is
    # disabled". Declared here so any eval against this flake (CI's
    # `accept-flake-config` runs, a local `nix flake check`, `nix build
    # .#checks.<sys>.<name>`) picks it up from one source of truth.
    #
    # `wasm-builtin` is DOCUMENTED here, not enabled here, and the difference
    # is load-bearing. `.ix` modules convert in-eval through `builtins.wasm`
    # over the `ix2nix-wasm` package output (IFD as of 2026-07-25, replacing
    # the committed `lib/ix2nix.wasm` of #4136), and the nix-ix client gates
    # that builtin behind this feature.
    #
    # Unlike `ca-derivations` above, a flake cannot turn this one on for its
    # own evaluation: nix builds the builtins table when it constructs the
    # evaluator, before it reads and applies a flake's `nixConfig`, so
    # `builtins.wasm` is already absent by the time this line is seen.
    # `accept-flake-config` does not change that. Measured both ways against
    # one attribute under a client whose `experimental-features` omitted it:
    # with this declaration and `accept-flake-config = true` the eval still
    # threw `importIx: this evaluator has no builtins.wasm`; naming the
    # feature on the client's own `experimental-features` line evaluated
    # (indexable-inc/ix#9288, ENG-11586).
    #
    # So a client needs it in its own nix.conf or NIX_CONFIG. index's CI gets
    # it from .github/actions/bootstrap-patched-nix. The entry stays because
    # it states the requirement in one place; it is not the mechanism.
    # Without the feature a client warns "unknown experimental feature" and
    # then throws an actionable error only if the eval actually imports a
    # `.ix` file (packages/ix2nix/import-ix.nix).
    extra-experimental-features = [
      "blake3-hashes"
      "ca-derivations"
      "wasm-builtin"
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # Relative-path ("subflake") inputs for the repo's independent data
    # subtrees. With lazy source trees a flake that reaches its whole tree via
    # `self` gives every package the entire repo as its source identity: any
    # file change anywhere re-hashes and re-copies the full tree per eval and
    # invalidates every dependent. Declaring each pure-data subtree as its own
    # `flake = false` path input scopes a consumer's source to just the subtree
    # it reads, so an edit under `packages/site/` no longer perturbs a
    # `skills` output's drvPath. nix and nox both resolve
    # these as lock nodes
    # `{ type = "path"; path = "./<dir>"; parent = []; }` against the parent
    # tree, with no separate fetch. Nix-code roots the flake itself imports
    # (`modules`, `packages`) stay ordinary relative paths: they are
    # import-time, not source identity. See ENG-2362.
    skills = {
      url = "path:./skills";
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
    site = {
      url = "path:./packages/site";
      flake = false;
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Pinned evaluation context for the prebuilt public-SDK `ix-sdk-wire` rlib
    # (packages/sdk/rust/build.nix). The artifact's manifest records the store
    # path of the toolchain it was compiled with, and that path folds the whole
    # nixpkgs + rust-overlay evaluation, so it is only reproducible from the
    # exact revs the artifact was published against. Pinned BY REV: a blanket
    # `nix flake update` re-locks the same rev, so the hourly flake bump can
    # never move this context out from under the artifact (#2131). A
    # republication of the rlib bumps these two revs together with
    # packages/sdk/rust/pins.json.
    sdk-prebuilt-nixpkgs = {
      url = "github:NixOS/nixpkgs/a799d3e3886da994fa307f817a6bc705ae538eeb";
      flake = false;
    };
    sdk-prebuilt-rust-overlay = {
      url = "github:oxalica/rust-overlay/107c334f141854f563f8adf1db781dc453d92639";
      flake = false;
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

    # Declarative disk layouts, taken for exactly one thing: `disko.lib.testLib`
    # and its `makeDiskoTest`, which installs a layout onto a blank disk in a VM
    # and then boots the installed system.
    #
    # That boot is the whole reason the dependency is here. `qemu-vm.nix` sets
    # `fileSystems = mkVMOverride cfg.fileSystems`, priority 10, which replaces
    # the attribute set rather than merging into it, so a module that declares
    # its own `fileSystems` (`modules/system/ephemeral-root` declares the
    # whitelist binds) has them dropped inside an ordinary
    # `pkgs.testers.runNixOSTest` machine. `tests/ephemeral-root-vm.nix` works
    # around that by merging `bindMounts` back into `virtualisation.fileSystems`,
    # which is fine for a layout the test itself declares and not fine for one
    # that has to exist on disk before the initrd runs. makeDiskoTest boots a
    # plain `eval-config` system off a real partition table, so `fileSystems`
    # merges the way it does on a real machine and the root really is the LV the
    # layout made. `tests/ephemeral-root-lvmthin-vm.nix` is the consumer.
    #
    # `follows` so testLib's `makeTest` / `eval-config` come from this flake's
    # nixpkgs rather than disko's own.
    disko = {
      url = "github:nix-community/disko";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    drgn-src = {
      url = "git+https://github.com/osandov/drgn?ref=refs/tags/v0.2.0&submodules=1";
      flake = false;
    };

    perftest-src = {
      url = "git+https://github.com/linux-rdma/perftest?ref=refs/tags/26.04.17";
      flake = false;
    };

    # PostgreSQL uint128 extension source. The package marks the extension trusted
    # so non-superuser database owners can run `CREATE EXTENSION uint128`.
    pg-uint128-src = {
      url = "github:pg-uint/pg-uint128/1.2.0";
      flake = false;
    };

    fff-src = {
      url = "github:dmtrKovalenko/fff/v0.9.1";
      flake = false;
    };

    nu-jupyter-kernel-src = {
      url = "github:cptpiepmatz/nu-jupyter-kernel/016d5089d9b0c66beb95311e339e252c8b9dd4e4";
      flake = false;
    };

    # pdtpartners/nix-ninja: ninja-compatible build runner that turns each
    # compilation unit of a meson/ninja graph into its own content-addressed
    # Nix derivation (the incremental build lane for the patched nix fork,
    # packages/nix-ninja-build, issue #3655). Pre-alpha upstream, so
    # pinned BY REV rather than by branch: the hourly flake-lock updater must
    # not move it; bump deliberately after revalidating the lane. Consumed as
    # a source tree (flake = false) and built with rustPlatform in
    # packages/nix-ninja, not through its flake, which would drag crane/fenix
    # inputs and only targets x86_64-linux.
    nix-ninja-src = {
      url = "github:pdtpartners/nix-ninja/f16edb417af156cdb777cc1201b67733b82b224e";
      flake = false;
    };

    # snix (Rust reimplementation of Nix; TVL-style depot, no flake.nix) consumed
    # as a source tree so `packages/snix` builds its CLI through cargo-unit
    # instead of the upstream crate2nix `Cargo.nix`. The Cargo workspace lives in
    # the repo's `snix/` subdirectory. Pinned in flake.lock; `nix flake update
    # snix-src` to bump.
    #
    # `shallow=1` is load-bearing, not cosmetic: only the source tree at the
    # pinned rev is ever used (`ix.snixSrc` -> `packages/snix`), never git
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
    # `examples/hermes/agent/`.
    hermes-agent = {
      url = "github:NousResearch/hermes-agent/v2026.5.16";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    sdk-prebuilt-nixpkgs,
    sdk-prebuilt-rust-overlay,
    home-manager,
    hermes-agent,
    disko,
    drgn-src,
    perftest-src,
    pg-uint128-src,
    fff-src,
    nu-jupyter-kernel-src,
    nix-ninja-src,
    snix-src,
    skills,
    examples,
    tests,
    site,
    ...
  }: let
    inherit (nixpkgs) lib;

    # All path literals the flake exposes. Centralized so lib/ and
    # lib/per-system.nix have a single source of truth.
    # The data-subtree entries below resolve to the `outPath` of relative-path
    # inputs (declared `flake = false` above) instead of bare `./<dir>`
    # literals. What that does NOT buy, despite what this comment used to say,
    # is per-subtree source identity. A relative-path input resolves to a
    # subpath of the whole flake source, so any commit anywhere in the repo
    # moves it. Measured on a clean tree at main, and the same store path from
    # a local checkout and from a fetched `github:indexable-inc/index/<rev>`:
    #
    #   inputs.skills.outPath
    #     -> /nix/store/v53pc7hv0h0aq3768j7zgxz0kl23a6zn-source/./skills
    #
    # Recorded because the claim it replaces would send someone here for a
    # consumer that must not rebuild on an unrelated commit, where this shape
    # would quietly do nothing. `builtins.path` is what does that: it hashes
    # the directory alone, so the store path moves only when the directory
    # does. lib/kernel/kbuild-unit.nix already used it and the vendored forks
    # in lib/default.nix now do too.
    #
    # Nix-code roots the flake imports directly (`modules`, `packagesRoot`) and
    # the whole-repo `root` (the lint source intentionally covers the entire
    # tree) stay ordinary relative paths: those are import-time / whole-repo by
    # design.
    paths = {
      root = ./.;
      skills = skills.outPath;
      modules = ./modules;
      examples = examples.outPath;
      users = ./users;
      tests = tests.outPath;
      site = site.outPath;
      pgUint128Src = pg-uint128-src;
      packagesRoot = ./packages;
      minecraftCatalogs = ./packages/minecraft/catalogs;
      minecraftMods = ./packages/minecraft/catalogs/mods;
      minecraftPaperPlugins = ./packages/minecraft/catalogs/plugins/paper;
      minecraftVelocityPlugins = ./packages/minecraft/catalogs/plugins/velocity;
      minecraftLoaders = {
        paper = ./packages/minecraft/catalogs/loaders/paper;
        velocity = ./packages/minecraft/catalogs/loaders/velocity;
        fabric = ./packages/minecraft/catalogs/loaders/fabric;
      };
      # Repo maintenance scripts and package-owned source updaters.
      tools = {
        cveScan = ./packages/cve-scan/cve-scan.py;
        ixShellSyncIgnored = ./packages/maintainers/scripts/ix-shell-sync-ignored.py;
        mcSource = ./packages/minecraft/tools/mc-source.nu;
        updateSounds = ./packages/minecraft/tools/update-sounds.nu;
        updateLoaders = ./packages/minecraft/tools/update-loaders.py;
        updateMods = ./packages/minecraft/tools/update-mods.py;
      };
    };

    ix = import ./lib {
      # The `.ix` converter, resolved through IFD instead of a committed
      # artifact. A function, not a value, so this stays lazy: `perSystem`
      # below takes `ix`, so a strict reference here would be a cycle. Nothing
      # on the ix2nix-wasm build path imports a `.ix` module, so forcing the
      # converter forces only `ix.cargoUnit` / `ix.rustWorkspace` /
      # `ix.languages`, never `importIxWasm` again.
      ix2nixWasmFor = system: perSystem.${system}.packages.ix2nix-wasm;
      inherit
        self
        nixpkgs
        paths
        rust-overlay
        sdk-prebuilt-nixpkgs
        sdk-prebuilt-rust-overlay
        home-manager
        hermes-agent
        drgn-src
        perftest-src
        fff-src
        nu-jupyter-kernel-src
        nix-ninja-src
        snix-src
        ;
    };
    devSystems = [
      "x86_64-linux"
      "aarch64-linux"
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
            rust-overlay
            home-manager
            disko
            ;
        }
    );
    # Cross-system output assembly (per-system collection, the
    # Linux-to-Darwin alias graft, the required-gate root union, the
    # security-root alias surface): lib/flake-outputs.nix.
    collected = import ./lib/flake-outputs.nix {inherit lib perSystem;};
    # homeModules / darwinModules composition and the personal profile
    # surfaces: lib/home-modules.nix (personal wiring in lib/profiles.nix).
    homeSurface = import ./lib/home-modules.nix {
      inherit lib ix paths home-manager nixpkgs;
      indexPackages = system: collected.packages."${system}";
    };
    loomConfiguration = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      specialArgs.loomPackage = perSystem.x86_64-linux.packages.loom;
      modules = [./packages/loom/nixos.nix];
    };
    # A plain ix template deliberately leaves root-device and bootloader facts
    # to the platform's injected machine profile. Extend the same configuration
    # as a container only for the flake check, so NixOS can realize its closure
    # without inventing guest hardware facts in the public template.
    loomTemplateCheck = loomConfiguration.extendModules {
      modules = [{boot.isContainer = true;}];
    };
  in {
    lib = ix;
    inherit (ix) nixosModules;
    nixosConfigurations.loom = loomConfiguration;
    inherit (homeSurface) darwinModules homeModules;
    overlays.default = ix.overlay;
    templates = {};
    inherit (collected) packages;
    checks = lib.mapAttrs (
      system: systemChecks:
        systemChecks
        // {
          personal-light-profile = (homeSurface.personalLightProfile system).activationPackage;
        }
        // lib.optionalAttrs (system == "x86_64-linux") {
          loom-template = loomTemplateCheck.config.system.build.toplevel;
        }
    ) (collected.collect "checks");
    # Sharded keying of the same check derivations for the memory-bounded CI
    # evaluator (the `.#check` gate and blast-radius); see lib/per-system.nix
    # (ENG-2201). Kept separate from `checks` because its per-package
    # `recurseForDerivations` groups are not derivations, which the flake
    # `checks` schema requires.
    inherit (collected) ciChecks;
    # Registry-derived map of package directory -> flake attr for every
    # `updateScript` package exposed on a system. update.yml's "Build changed
    # packages" step evaluates this to find which attr owns each file the
    # updaters changed, instead of deriving an attr from path segments
    # (#2036). Non-schema, so surfaced through `collect` like `ciChecks`.
    updatablePackages = collected.collect "updatablePackages";
    # CI-only view of `packages` with each NixOS image swapped for its
    # `toplevel` closure; cache-push.yml publishes this instead of the
    # monolithic `*-oci.tar` archives, which nothing substitutes. Non-schema,
    # so surfaced through `collect` like `ciChecks`. See lib/per-system.nix.
    inherit (collected) cachePushRoots;
    # Union consumed by `nix run .#check -- required`: one bounded
    # nix-fast-build evaluator replaces the former flake-check and closure-gate
    # self-hosted jobs without dropping either required status context.
    inherit (collected) requiredGateRoots;
    # Typed security exposure roots consumed as JSON by the runtime DAG scanner.
    # Unlike cachePushRoots, every entry carries policy metadata and names only
    # a shipped runtime output or an example service closure. securityRootPaths
    # carries the derivations separately so callers realize terminal store paths
    # instead of trusting content-addressed placeholders from evaluation.
    inherit (collected) securityRoots securityRootPaths;
    # Opt-in heavy roots (kbuild-unit #3411): `nix build .#kernel-unit.vmlinux`
    # resolves through legacyPackages on x86_64-linux. Deliberately not in
    # `packages`, so no CI gate closure picks up its eval-time IFD kbuild.
    legacyPackages = collected.collect "legacyPackages";
    formatter = collected.collect "formatter";
    apps = collected.collect "apps";
    devShells = collected.collect "devShells";
  };
}
