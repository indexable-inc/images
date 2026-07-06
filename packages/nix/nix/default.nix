{
  ix,
  lib,
}:
# Upstream NixOS/nix pinned at tag 2.34.7 (the `nix-src` input, surfaced as
# `ix.nixSrc`) with the in-repo patch series (./patches) applied, built through
# nixpkgs' own modular nix packaging so the result is a protocol-compatible
# drop-in for the daemon the fleet runs.
#
# De-forking replacement for a standalone `indexable-inc/nix` fork checkout:
# instead of tracking a whole fork branch, the delta lives as an ordered
# `patches/` series applied on top of the exact upstream rev the daemon runs.
# The current series carries the GC-roots client-interrupt daemon crash fix
# (extracted from the fork's `fix-gc-roots-client-interrupt-crash` branch,
# complete and clean against 2.34.7), an eval fix treating inaccessible
# default lookup-path entries as absent (found while validating this build:
# the macOS sandbox denies the host's root-channels dir with EPERM, which
# aborted the C API unit tests and the recursive-nix functional test on any
# darwin host with root channels; clean CI builders lack the path, which is
# why caches carry the stock drvs green), and the `build-status-dir` global
# build-observability series (patches 0003..0009): behind an experimental
# feature of that name, every active build/substitution goal writes a JSON
# status file under `<nixStateDir>/status/`, readable daemonlessly via the
# new `nix store builds [--json]` command. The fork's
# `codex/flake-check-eval-cache` branch (draft PR indexable-inc/nix#1) is
# deliberately excluded: it is self-declared WIP, untested, and incomplete.
let
  # Read `pkgs` from `ix` rather than a `pkgs` callPackage formal: a `src`/`pkgs`
  # formal is fragile against `callPackage` auto-binding, and the rest of the
  # nix/* packages read `pkgs` off their argument the same way.
  inherit (ix) pkgs;

  # The patched upstream tree: the ./patches series applied 0001..NNNN on top of
  # the pinned 2.34.7 source, via the shared de-fork util. This doubles as the
  # `checks.<system>.patched-src-nix` conflict gate (per-system wiring), so a
  # patch that stops applying fails there in seconds.
  patchedSrc = ix.patchedSrc {
    name = "nix";
    src = ix.nixSrc;
    patchDir = ./patches;
  };

  # nixpkgs builds `nixVersions.nix_2_34` as
  # `(nixComponents_2_34.overrideSource fetchedSrc).appendPatches patches_common`
  # then takes `.nix-everything` (pkgs/tools/package-management/nix/default.nix).
  # The pinned rev IS the tag 2.34.7 nixpkgs itself fetches (byte-identical
  # narHash), so swapping the fetched source for our patched tree via the same
  # modular `overrideSource` handle rebuilds every component from the patched
  # tree while keeping nixpkgs' interdependency scope and build wiring intact.
  base = pkgs.nixVersions.nixComponents_2_34;

  # nixpkgs' own whole-source patches for this version: currently just the
  # aarch64-darwin flaky-test skip (empty on every other system). `overrideSource`
  # resets the scope's `patches` to `[]`, so re-apply them here to match a stock
  # `nix_2_34` build; our own delta rides in `patchedSrc`, not here.
  patchesCommon = lib.optional pkgs.stdenv.hostPlatform.isDarwin (
    pkgs.path + "/pkgs/tools/package-management/nix/patches/skip-flaky-darwin-tests.patch"
  );

  # Identify a patched daemon by version: `nix --version` (and
  # `builtins.nixVersion`) report the version each *component* was compiled
  # with -- the modular build's preConfigure writes the component derivation's
  # `version` into the tree's `.version` on every build, so a `.version` source
  # patch in our series would be clobbered and a version override on the
  # `nix-everything` aggregate would only rename the store path. Set it through
  # `overrideAllMesonComponents`, the last layer in the component builder
  # stack, which also wins over the sourceLayer's `+<patch-count>` suffix.
  # The marker is semver build metadata (`+ix`, like nixpkgs' own `+1`), not
  # `-ix`: meson feeds the version to darwin ld's -current_version, which
  # rejects a `-` suffix as a "malformed 32-bit x.y.z version number" but
  # tolerates `+`.
  patchedComponents =
    ((base.overrideSource patchedSrc).appendPatches patchesCommon).overrideAllMesonComponents
    (_: _: {version = "2.34.7+ix";});

  # The aggregate `nix` package (daemon + client + libs), the same attribute
  # `nixVersions.nix_2_34` exposes.
  nixEverything = patchedComponents.nix-everything;

  package = nixEverything.overrideAttrs (old: {
    version = "2.34.7+ix";
    # The aggregate's `doCheck = true` gates the build on `checkInputs`: the
    # five component unit-test runners plus the entire upstream functional
    # suite. Those dominate a cold build of this closure and re-validate
    # nothing per consumer rebuild: patch applicability is already gated by
    # `checks.<system>.patched-src-nix`, the series carries its own
    # upstream-style functional test inside the patched tree, and the `smoke`
    # passthru below executes the linked binary. With them on, the cache-push
    # darwin lane (3-core hosted mac) blew its 4 h job budget cold-building
    # this package and froze `cache-ready` (run 28772327218, index#1967).
    doCheck = false;
    meta =
      (old.meta or {})
      // {
        description = "NixOS/nix 2.34.7 with the index in-repo patch series (GC-roots daemon-crash fix, lookup-path EPERM eval fix)";
        mainProgram = "nix";
      };
  });

  # The override's real risk is that the whole modular C++ tree still links and
  # the patched daemon still runs, so the smoke test executes the binary and
  # asserts the `+ix` marker. `--version` exits 0 without touching a store or
  # daemon (absent in the sandbox), so it is safe as a build-time check.
  smoke =
    pkgs.runCommand "nix-ix-smoke"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      version=$(nix --version)
      case "$version" in
        *"2.34.7+ix"*) ;;
        *)
          echo "nix --version did not report the patched 2.34.7+ix build" >&2
          printf '%s\n' "$version" >&2
          exit 1
          ;;
      esac
      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or old.tests or {})
          // {
            inherit smoke;
          };
      };
  })
