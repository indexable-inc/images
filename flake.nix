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

    # Fork of nix-community/home-manager carrying the batched activation
    # linking series (lib/fork-packages.nix). Distinct from the
    # `home-manager` flake input above: this pins the `ix-patched` tip that
    # workstation configs also consume. Pinned BY REV (autoUpdate = false):
    # bump = merge upstream into `ix-patched` in indexable-inc/home-manager,
    # fast-forward the branch, repin here.
    home-manager-src = {
      url = "github:indexable-inc/home-manager/7d29fa5cbf4b468b7d9692cfb500cb89291fb519";
      flake = false;
    };

    # jj megamerge fork of rust-lang/rust-clippy with the restriction lints
    # tuned for LLM-assisted codebases (lib/fork-packages.nix). Pinned BY
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
    # regenerate it. Then jj-rebase the fork repo onto the new base. The rev is
    # the indexable-inc/rust-clippy `ix-patched` megamerge (upstream base
    # 512551c8 plus the lint patch DAG).
    clippy-src = {
      url = "github:indexable-inc/rust-clippy/cd551e2408a75638f6b0ac7ea88aa0dd18b8aea3";
      flake = false;
    };

    # jj megamerge fork of git/git: linked worktrees borrow the common-dir
    # submodule object store instead of re-cloning every submodule from the
    # network (#3610). The base tracks nixpkgs' git version (v2.55.0 tag)
    # because the package overlays nixpkgs' git recipe, so it never
    # free-floats: on a nixpkgs git bump, rebase the series onto the matching
    # tag, MERGE the result into ix-patched and repin. The merge is what keeps
    # revs pinned by older index commits reachable, so a per-release branch
    # (ix-patched-v<version>) is no longer minted; the one left behind,
    # ix-patched-v2.55.0, is now an ancestor of the bookmark.
    git-src = {
      url = "github:indexable-inc/git/eef38393cda1413ded72f0259d1618110cf38456";
      flake = false;
    };

    # jj megamerge fork of jj-vcs/jj. Pinned BY REV, never branch-loose: the
    # series is large and touches working-copy internals, and a conflicted jj
    # commit must never reach the bookmark (git-based readers cannot parse
    # jj's conflict encoding). The rev therefore moves only when a human
    # deliberately repins, never under the scheduled fork-sync
    # (autoUpdate = false in lib/fork-packages.nix).
    #
    # `ix-patched` is published history that flake.locks pin, so it is never
    # rebased: work lands as ordinary commits on top, and upstream arrives as a
    # two-parent merge. Bump it by hand: push the commits, wait for that
    # branch's own push-triggered CI to go green (f561bc016 is what makes a
    # pushed tip get its own verdict), mint the pin ref, repin here, then build
    # `.#jj`.
    jj-src = {
      url = "github:indexable-inc/jj/af48c0d50b540e7ff0b909e635098ffaefc2f007";
      flake = false;
    };

    # jj megamerge fork of openai/codex. Pinned BY REV: importCargoLock
    # removes the aggregate cargoHash, but git dependencies still carry fixed
    # output hashes in the package. A branch-loose URL lets a blanket
    # `nix flake update` float the source past those hashes, which broke every
    # ix prod deploy for 13h on 2026-07-07. Bump deliberately: jj-rebase
    # indexable-inc/codex, repin here, then build Codex and refresh any git
    # dependency hashes named by Nix. The scheduled content and fork updaters
    # intentionally leave this input alone.
    codex-src = {
      url = "github:indexable-inc/codex/1ca1b52d1e4e579d0fd35b17e5fd8e719b84bd33";
      flake = false;
    };

    # jj megamerge fork of NixOS/nix. The base stays at tag 2.34.7, the
    # version the hydra daemon runs (`nix store info` -> `Version: 2.34.7`):
    # nix is our daemon toolchain, so the patched package must stay a
    # protocol-compatible drop-in for the running daemon. The base moves
    # DELIBERATELY, never under a routine `nix flake update` (fork-packages
    # marks it `autoUpdate = false`): jj-rebase indexable-inc/nix only when we
    # intend to move the daemon version too, then repin here.
    nix-src = {
      # ix-patched f200a3a8d492 (72 patches on 2c6d06e9387c). This pin does NOT
      # descend from the 0f356d7c it replaces: ix-patched was reflattened and
      # reformatted after that megamerge commit, so the two share only the
      # upstream base and the old rev survives as refs/pins/2026-07-29-0f356d7cf513
      # (this one as refs/pins/2026-07-31-f200a3a8d492). The pin had drifted 16
      # patches behind the branch before this bump, so it carries more than the
      # 3 patches the bump was opened for: `nix invocation` post-hoc build
      # introspection, a 384 MiB to 8 GiB initial GC heap cap, the settled end
      # of the Darwin fast-exit pty work (drain from its own thread, reverted,
      # relanded, then handshake-fd fix), remote build machine recording
      # (ENG-11260) and the Mach-O page-hash reproducibility fix. Only nix's own
      # tests gate those here; apart from the Mach-O fix, whose NACK this repo
      # records (index#4344), they have no lib/fork-packages.nix intent entries
      # and so default to `hold`, which cannot send them upstream.
      #
      # The 3 deliberate ones: source paths under the store directory now carry
      # a fingerprint, so `fetchToStore` stops re-hashing and re-copying the
      # subtree on every eval (ENG-10821, indexable-inc/index#4323), and the jj
      # workdir accessor consumes `jj file list` as exact paths rather than
      # allow-list prefixes, so a listed entry that names a directory can no
      # longer admit the whole subtree beneath it (ENG-11616) -- which had been
      # baking a submodule's contents and its `gitdir:` pointer file into the
      # store for a `jj+file` input nobody passed `submodules=1` to.
      url = "github:indexable-inc/nix/f200a3a8d4921393547f93166cce8cebcb2b0e44";
      flake = false;
    };

    # jj megamerge fork of aristocratos/btop. Tracks upstream main
    # (autoUpdate = true in lib/fork-packages.nix): the scheduled fork-sync
    # jj-rebases the fork onto the new upstream tail, pushes `ix-patched` plus
    # a pin ref, and floats this branch-loose input with `nix flake update`.
    btop-src = {
      url = "github:indexable-inc/btop/ix-patched";
      flake = false;
    };

    # jj megamerge fork of nushell/nushell. Tracks upstream main (autoUpdate =
    # true in lib/fork-packages.nix): the scheduled fork-sync jj-rebases the
    # xattr patch onto the new tail and floats this branch-loose input.
    nushell-src = {
      url = "github:indexable-inc/nushell/ix-patched";
      flake = false;
    };

    # jj megamerge fork of Mic92/nix-fast-build. The base stays at tag 1.6.0,
    # the exact version nixpkgs packages, because the package overlays the
    # patched source onto nixpkgs' nix-fast-build recipe, so it must track
    # the nixpkgs version, never free-float (autoUpdate = false in
    # lib/fork-packages.nix). On a nixpkgs nix-fast-build bump, jj-rebase
    # indexable-inc/nix-fast-build onto the matching tag and repin.
    nix-fast-build-src = {
      url = "github:indexable-inc/nix-fast-build/6b976a8b2f8252942312599e6bfec20cec207f97";
      flake = false;
    };

    # jj megamerge fork of Gabriella439/Haskell-Nix-Derivation-Library, the
    # `nix-derivation` Haskell library nix-output-monitor parses .drv files
    # with. The upstream repo publishes no
    # tags; this rev is upstream main while the cabal version still reads
    # 1.1.3 -- the hackage release nixpkgs builds -- PLUS the post-release
    # dependency-bound relaxations (QuickCheck 2.15, filepath 1.5) hackage
    # carries as cabal revisions, so overriding the hackage sdist with this
    # tree keeps the same dependency envelope. autoUpdate = false: repin when
    # nixpkgs moves to a newer nix-derivation.
    nix-derivation-src = {
      url = "github:indexable-inc/Haskell-Nix-Derivation-Library/ba78008319f3517013a9fd70245ecee5ab2054b4";
      flake = false;
    };

    # jj megamerge forks of nix-community/rnix-parser at the release tags
    # whose crates the repo's nix tools vendor today: v0.12.0 (alejandra,
    # deadnix) and v0.14.0 (statix), one bookmark per series in
    # indexable-inc/rnix-parser (ix-patched-0.12 / ix-patched-0.14).
    # lib/util/rnix-digit-separators overlays the patched sources onto each
    # tool's cargo vendor dir at build time. autoUpdate = false: each pin
    # moves only when a nixpkgs bump moves the vendored rnix version (then
    # jj-rebase the matching bookmark and repin).
    rnix-0-12-src = {
      url = "github:indexable-inc/rnix-parser/015fe463b6e9bad73326d725e0d3fa9a61e1fbdb";
      flake = false;
    };
    rnix-0-14-src = {
      url = "github:indexable-inc/rnix-parser/3f74f857a0d2bb6715ba993f506368b8b413d0d5";
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
    # off every darwin install; the fork's "build: keep -Demit-lib-vt
    # buildable without Apple-absolute tools" patch commit carries the
    # sandbox fix. ghostty is darwin-only and darwin has no CI
    # (index-workstation-profile-no-ci-eval documents the class of
    # silent-drift risk), so nothing catches a routine bump breaking
    # this build; move this rev only via a jj rebase of indexable-inc/ghostty
    # followed by a manual `nix build .#ghostty` on darwin. The rev is the
    # `ix-patched` megamerge (upstream base 49a43bf5 plus the patch DAG).
    ghostty-src = {
      url = "github:indexable-inc/ghostty/c1b4a88a20757642c8f945f8d96c4905198158cb";
      flake = false;
    };

    # jj megamerge fork of mesa (upstream lives on gitlab.freedesktop.org;
    # indexable-inc/mesa mirrors the needed history) for the panes GPU guest:
    # the venus driver-side external-semaphore delta (index#1742) on the
    # `mesa-26.1.2` tag base. A github tarball fetch, so no history clone;
    # the old gitlab URL needed `shallow=1` to avoid one. Pinned by rev
    # (autoUpdate = false in lib/fork-packages.nix): a mesa bump must be
    # rebased AND boot-validated on a linux GPU host (the venus patch is
    # validated by running the guest, not by CI), so it moves only under a
    # deliberate manual bump, never the cron.
    mesa-src = {
      url = "github:indexable-inc/mesa/0859cf8912b7dde1cb7b06f2ed416a84a479feef";
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
    jj-src,
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
        home-manager-src
        hermes-agent
        btop-src
        nushell-src
        git-src
        jj-src
        drgn-src
        perftest-src
        fff-src
        nu-jupyter-kernel-src
        launchk-src
        nix-ninja-src
        snix-src
        clippy-src
        codex-src
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
