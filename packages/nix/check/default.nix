# `check` is the full CI gate as one repo-owned command: check.yml runs
# `nix run .#check`, so the same two steps run in CI and locally from a single
# definition. It targets x86_64-linux explicitly because that is the system CI
# builds for; a linux runner can only pure-eval the cross-platform darwin
# images, and that cross-eval was most of what made the old single-threaded
# `nix flake check` slow. `nix` is taken from the ambient PATH on purpose
# (this is always invoked as `nix run .#check`, so the host's daemon-matched
# nix is already present); pinning a client nix here could mismatch the host
# Nix 2.34.x daemon. The gate mechanics (build gate, failed-log replay,
# `::error::` annotations, schema eval gate) live in the crate (src/main.rs).
{
  ix,
  pkgs,
  repoPackages,
  ...
}: let
  inherit (pkgs) lib;

  checkUnit = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "check";
    meta.mainProgram = "check";
  };
in
  ix.wrapPackage pkgs {
    package = checkUnit;
    env = {
      # Patched nix-fast-build (packages/nix/nix-fast-build): stock
      # --skip-cached only skips a job whose nix-eval-jobs cacheStatus is
      # `cached` (in a remote substituter); a `local` output (already in this
      # warm runner's store but never pushed) falls through and is re-realized
      # every run. On this CI the rust units and image closures are
      # floating-CA and resolve to `local`, so the patch makes --skip-cached
      # skip `local` too. nixpkgs' 1.5.0 tag is the same commit (7f185e0) the
      # flake ref used to pin, so this is a like-for-like source swap plus the
      # patch. Invoked directly by store path, not `nix run`.
      IX_NIX_FAST_BUILD = lib.getExe repoPackages.nix-fast-build;
      # nix-eval-jobs is linked to the stable Nix 2.34 components the fleet
      # daemon runs (not nixpkgs' moving default). Built for x86_64-linux
      # (the CI gate system); `check` itself is x86_64-linux-only.
      IX_NIX_EVAL_JOBS = lib.getExe repoPackages.nix-eval-jobs;
    };
    passthru.tests = checkUnit.passthru.tests;
    meta = {
      description = "Run the full CI gate: build .#ciChecks.x86_64-linux and eval-validate .#packages.x86_64-linux (`closure` subcommand: build .#cachePushRoots.x86_64-linux)";
      mainProgram = "check";
    };
  }
