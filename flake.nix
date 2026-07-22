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
    extra-experimental-features = ["ca-derivations"];
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
    # `packages/agent/skills` package's drvPath. nix and nox both resolve
    # these as lock nodes
    # `{ type = "path"; path = "./<dir>"; parent = []; }` against the parent
    # tree, with no separate fetch. Nix-code roots the flake itself imports
    # (`modules`, `packages`) stay ordinary relative paths: they are
    # import-time, not source identity. See ENG-2362.
    skills = {
      url = "path:./packages/agent/skills";
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

    # Upstream nix-community/home-manager, patched in-repo
    # (packages/home-manager/patches) with the batched activation linking
    # series. Distinct from the `home-manager` flake input above: this is the
    # de-forked SOURCE base (lib/fork-packages.nix), consumed by the
    # `patched-src-home-manager` check and mirrored to the
    # indexable-inc/home-manager `ix-patched` branch that workstation configs
    # consume as their home-manager flake input. Pinned BY REV (autoUpdate =
    # false): bump by hand with `nix flake update home-manager-src` + `nix run
    # .#rebase-patches -- home-manager`, then re-push the fork branch
    # (`mirror fork-branch --name home-manager --push`).
    home-manager-src = {
      url = "github:nix-community/home-manager/f4d01c1d87c7c2ec909549165d5a8338f1bd3315";
      flake = false;
    };

    # Upstream rust-lang/rust-clippy, patched in-repo with the restriction lints
    # tuned for LLM-assisted codebases (packages/llm-clippy/patches). Pinned BY
    # REV, not a floating branch: clippy-driver links rustc_private and must match
    # the repo's pinned nightly (root rust-toolchain.toml) exactly, or every
    # per-unit clippy gate fails with E0514 "compiled by an incompatible version
    # of rustc". Never free-float under a blanket `nix flake update`.
    #
    # The repo nightly (2026-05-27) sits between
    # upstream clippy's autogenerated sync points (05-13 -> 05-28), so no
    # upstream rev compiles against it as-is. The series' first patch bridges
    # that gap (toolchain file + rustc_private adaptations). Bump this rev only
    # inside the same change that bumps the repo rust toolchain; if the new
    # nightly matches an upstream sync commit, drop the bridge patch, otherwise
    # regenerate it. Then `nix run .#rebase-patches -- clippy` for the rest.
    clippy-src = {
      url = "github:rust-lang/rust-clippy/512551c839fc711fc925c8a862a9abd4bde0812f";
      flake = false;
    };

    # Upstream git/git, patched in-repo (packages/git/patches): linked
    # worktrees borrow the common-dir submodule object store instead of
    # re-cloning every submodule from the network (#3610). Pinned by rev to
    # the v2.54.0 tag because the package overlays nixpkgs' git recipe, so
    # the base must track nixpkgs' git version, never free-float: on a
    # nixpkgs git bump, repin to the matching tag and run
    # `nix run .#rebase-patches -- dag git`.
    git-src = {
      url = "github:git/git/94f057755b7941b321fd11fec1b2e3ca5313a4e0";
      flake = false;
    };

    # Upstream openai/codex, patched in-repo (packages/agent/codex/patches).
    # Pinned BY REV: importCargoLock removes the aggregate cargoHash, but git
    # dependencies still carry fixed output hashes in the package. A
    # branch-loose URL lets a blanket `nix flake update` float the source past
    # those hashes, which broke every ix prod deploy for 13h on 2026-07-07.
    # Bump this rev deliberately, run `nix run .#rebase-patches -- codex`, then
    # build Codex and refresh any git dependency hashes named by Nix. The
    # scheduled content and fork updaters intentionally leave this input alone.
    codex-src = {
      url = "github:openai/codex/1f0566d3f59298d1bb88820a0d35294f1eeb07ea";
      flake = false;
    };

    # The maintained fork is the application source. Its own flake owns the
    # Rust lock, toolchain, and platform build.
    zed-src.url = "github:indexable-inc/zed/ix-patched";

    # Unmodified upstream base for validating and regenerating the patch series
    # that produces zed-src's ix-patched branch.
    zed-upstream = {
      url = "github:zed-industries/zed/v1.10.x";
      flake = false;
    };

    # Upstream NixOS/nix, patched in-repo (packages/nix/nix/patches). Pinned BY
    # REV at tag 2.34.7, the version the hydra daemon runs (`nix store info` ->
    # `Version: 2.34.7`): nix is our daemon toolchain, so the patched package
    # must stay a protocol-compatible drop-in for the running daemon. The base
    # moves DELIBERATELY, never under a routine `nix flake update`
    # (fork-packages.nix marks it `autoUpdate = false`, so the scheduled
    # fork-sync leaves it alone): bump this rev only when we intend to move the
    # daemon version too, then `nix run .#rebase-patches -- nix` to regenerate
    # the series on the new base.
    nix-src = {
      url = "github:NixOS/nix/2c6d06e9387cf58167cb5a7ab91cee7333d8d17c";
      flake = false;
    };

    # Upstream aristocratos/btop, patched in-repo (packages/terminal/btop/patches).
    # Tracks upstream main (autoUpdate = true in lib/fork-packages.nix): the base
    # free-floats under the scheduled fork-sync, which runs `nix flake update
    # btop-src` + `nix run .#rebase-patches -- btop` to advance the two patches
    # (macOS disk IO sorting, cwd detail box) onto the new tail.
    btop-src = {
      url = "github:aristocratos/btop";
      flake = false;
    };

    # Upstream nushell/nushell, patched in-repo (packages/nushell/patches).
    # Tracks upstream main (autoUpdate = true in lib/fork-packages.nix): the
    # scheduled fork-sync bumps nushell-src and rebases the xattr patch.
    nushell-src = {
      url = "github:nushell/nushell";
      flake = false;
    };

    # Upstream Mic92/nix-fast-build, patched in-repo
    # (packages/nix/nix-fast-build/patches). Pinned to the rev of tag 1.6.0 --
    # the exact version nixpkgs packages -- because the package overlays the
    # patched source onto nixpkgs' nix-fast-build recipe, so the base must
    # track the nixpkgs version, never free-float (autoUpdate = false in
    # lib/fork-packages.nix). On a nixpkgs nix-fast-build bump, repin to the
    # matching tag and run `nix run .#rebase-patches -- nix-fast-build`.
    nix-fast-build-src = {
      url = "github:Mic92/nix-fast-build/a28921953d962c6c2527108a6be4062eb6dc2f51";
      flake = false;
    };

    # Upstream Gabriella439/Haskell-Nix-Derivation-Library, the `nix-derivation`
    # Haskell library nix-output-monitor parses .drv files with, patched
    # in-repo (packages/nix/nix-output-monitor/patches). The repo publishes no
    # tags; this rev is upstream main while the cabal version still reads
    # 1.1.3 -- the hackage release nixpkgs builds -- PLUS the post-release
    # dependency-bound relaxations (QuickCheck 2.15, filepath 1.5) hackage
    # carries as cabal revisions, so overriding the hackage sdist with this
    # tree keeps the same dependency envelope. autoUpdate = false: repin when
    # nixpkgs moves to a newer nix-derivation.
    nix-derivation-src = {
      url = "github:Gabriella439/Haskell-Nix-Derivation-Library/f1f5d5a2270b5ee23dfad40fee385cf4e94d6cea";
      flake = false;
    };

    # Upstream nix-community/rnix-parser at the release tags whose crates the
    # repo's nix tools vendor today: v0.12.0 (alejandra, deadnix) and v0.14.0
    # (statix). lib/util/rnix-digit-separators patches the *vendored* rnix
    # crate inside each tool's cargo vendor dir at build time; these pinned
    # sources give the same patch series a registry-grade
    # `patched-src-rnix-0-1{2,4}` apply gate in flake checks, so tokenizer
    # drift surfaces in CI instead of a consumer build. autoUpdate = false:
    # each pin moves only when a nixpkgs bump moves the vendored rnix version
    # (then repin to the matching tag and rerun `rebase-patches`).
    rnix-0-12-src = {
      url = "github:nix-community/rnix-parser/d521c438acfa9383646f9c4af9d10bbb02df0f78";
      flake = false;
    };
    rnix-0-14-src = {
      url = "github:nix-community/rnix-parser/0472081214c24b1ab4d34f7bf544284ed4e45ad3";
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

    launchk-src = {
      url = "github:mach-kernel/launchk/6f5f09e0dfa3fea662e859de5d7d49ac09a9dbe6";
      flake = false;
    };

    # pdtpartners/nix-ninja: ninja-compatible build runner that turns each
    # compilation unit of a meson/ninja graph into its own content-addressed
    # Nix derivation (the incremental build lane for the patched nix fork,
    # packages/nix/nix-ninja-build, issue #3655). Pre-alpha upstream, so
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
    # `examples/hermes/agent/`.
    hermes-agent = {
      url = "github:NousResearch/hermes-agent/v2026.5.16";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Ghostty's full application source, patched in-repo
    # (packages/ghostty/patches, the vendored fork of index#3768). The one
    # ghostty input: `packages/ghostty` and `packages/tui/vt/libghostty-vt`
    # (plus the Rust workspace's ix-vt link) all build the VT-engine-only
    # `-Demit-lib-vt` subtree from this tree WITH the patch series applied --
    # the series includes C-API additions (per-cell hyperlink URIs,
    # index#3835) that ix-vt binds, so an unpatched build no longer links.
    # The rest of ghostty's darwin core is not buildable in the sandbox (see
    # `lib/build/libghostty-vt.nix`'s doc comment). `requireZig` in
    # `build.zig.zon` is exact minor (0.15.x), so builds use `pkgs.zig_0_15`;
    # the pinned tree ships `build.zig.zon.nix` (zon2nix output), which
    # vendors every lazy Zig dependency with SRI hashes for a network-free
    # build.
    #
    # Pinned BY REV (autoUpdate = false in lib/fork-packages.nix), same as
    # `nix-src`/`clippy-src`. This rev matches indexable-inc/ix's ghostty-src
    # pin (one rev across both repos) and is the earliest surface carrying
    # the terminal C API's write_pty/query-response callbacks
    # (`ghostty_terminal_set`, GHOSTTY_TERMINAL_OPT_WRITE_PTY) that ix-vt's
    # `drain_responses` binds (ix#8117). At this rev the VT static lib
    # vendors SIMD deps and combines archives via hardcoded
    # `/usr/bin/ranlib`/`/bin/cp` (absolute paths the Nix darwin sandbox
    # correctly denies) and build.zig hangs a vt xcframework (xcodebuild)
    # off every darwin install; fork patch
    # 0004-build-keep-Demit-lib-vt-buildable-without-Apple-abso carries the
    # sandbox fix. ghostty is darwin-only and darwin has no CI
    # (index-workstation-profile-no-ci-eval,
    # zed-src-patch-lock-drift-darwin-only-guard document the same class of
    # silent-drift risk for zed), so nothing catches a routine bump breaking
    # this build; move this rev only with `nix run .#rebase-patches --
    # ghostty` followed by a manual `nix build .#ghostty` on darwin.
    ghostty-src = {
      url = "github:ghostty-org/ghostty/49a43bf560322eac0ba5d30c20a8b212106e3883";
      flake = false;
    };

    # Upstream mesa (gitlab.freedesktop.org), patched in-repo for the panes GPU
    # guest (packages/vm/panes/guest-image/mesa/patches): the venus driver-side
    # external-semaphore delta (index#1742). De-forking replacement for the old
    # `indexable-inc/mesa` snapshot fork tarball; pinned by the `mesa-26.1.2`
    # tag, so the patched tree is the upstream tag tree plus the venus commits.
    #
    # `shallow=1` is load-bearing (same reason as snix-src): mesa's git history
    # is huge, and only the source tree at the pinned tag is ever used (through
    # `ix.mesaSrc` -> patchedSrc), never history or revCount. Without it the
    # lock records `revCount`, forcing a full-history clone on every cold
    # `nix flake archive` / direnv load. `flake.lock` still records the rev, so
    # `rebase-patches` can read the base rev; the URL is a real git remote so
    # its scratch-clone fetch works. Pinned by rev (autoUpdate = false in
    # lib/fork-packages.nix): a mesa bump must be rebased AND boot-validated on
    # a linux GPU host (the venus patch is validated by running the guest, not
    # by CI), so it moves only under a deliberate manual bump, never the cron.
    mesa-src = {
      url = "git+https://gitlab.freedesktop.org/mesa/mesa?ref=refs/tags/mesa-26.1.2&shallow=1";
      flake = false;
    };
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    sdk-prebuilt-nixpkgs,
    sdk-prebuilt-rust-overlay,
    home-manager,
    home-manager-src,
    hermes-agent,
    btop-src,
    nushell-src,
    git-src,
    drgn-src,
    perftest-src,
    pg-uint128-src,
    fff-src,
    nu-jupyter-kernel-src,
    launchk-src,
    nix-ninja-src,
    snix-src,
    clippy-src,
    codex-src,
    zed-src,
    zed-upstream,
    nix-src,
    nix-fast-build-src,
    nix-derivation-src,
    rnix-0-12-src,
    rnix-0-14-src,
    ghostty-src,
    mesa-src,
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
    # literals, so each consumer's source identity is scoped to just that
    # subtree. Nix-code roots the flake imports directly (`modules`,
    # `packagesRoot`) and the whole-repo `root` (the lint source intentionally
    # covers the entire tree) stay ordinary relative paths: those are
    # import-time / whole-repo by design, not per-subtree source identity.
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
      inherit
        self
        nixpkgs
        paths
        rust-overlay
        sdk-prebuilt-nixpkgs
        sdk-prebuilt-rust-overlay
        home-manager
        home-manager-src
        hermes-agent
        btop-src
        nushell-src
        git-src
        drgn-src
        perftest-src
        fff-src
        nu-jupyter-kernel-src
        launchk-src
        nix-ninja-src
        snix-src
        clippy-src
        codex-src
        zed-src
        zed-upstream
        nix-src
        nix-fast-build-src
        nix-derivation-src
        rnix-0-12-src
        rnix-0-14-src
        ghostty-src
        mesa-src
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
  in {
    lib = ix;
    inherit (ix) nixosModules;
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
    # Per-attempt-patch closure build gates (RFC 0010 A3, #2098), keyed
    # `<system>.<fork>.<patch>`: the fork package rebuilt with the series
    # restricted to that patch's dag.json closure. Deliberately NOT under
    # `checks` (per-PR flake-check cost stays flat): built post-merge by the
    # scheduled fork-closure-gates workflow and by the `upstream-sync --open`
    # preflight. Non-schema, so surfaced through `collect` like `ciChecks`.
    forkClosureGates = collected.collect "forkClosureGates";
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
