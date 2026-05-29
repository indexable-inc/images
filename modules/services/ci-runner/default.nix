# Self-hosted GitHub Actions runners for this repository, meant to run on a
# persistent NixOS host instead of an ephemeral cloud VM. The win is cache
# locality: jobs reuse the host's global /nix/store and the indexable-inc
# Cachix substituter, so `nix build .#...` pulls warm artifacts instead of
# rebuilding from a cold store every run. The per-job work directory is still
# wiped (see `ephemeral`); only the shared store survives, which is exactly the
# state that is safe to keep between jobs.
#
# This is the small counterpart to the ix repo's webhook dispatcher: a static
# pool of registered runners, no just-in-time minting and no per-job VM.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkIf
    mkOption
    types
    ;
  cfg = config.services.ci-runner;
  runnerNames = map (index: "index-${toString index}") (lib.range 1 cfg.count);
in
{
  options.services.ci-runner = {
    enable = mkEnableOption "self-hosted GitHub Actions runners for this repository";

    url = mkOption {
      type = types.str;
      example = "https://github.com/indexable-inc/index";
      description = ''
        Repository or organization URL the runners register against, the same
        value you would pass to {option}`services.github-runners.<name>.url`.
      '';
    };

    tokenFile = mkOption {
      # A runtime path string, not `types.path`: a path literal like `./token`
      # would copy the PAT into the world-readable Nix store, which this option
      # explicitly promises not to do.
      type = types.str;
      example = "/run/secrets/ci-runner/token";
      description = ''
        Path to a file holding a fine-grained GitHub PAT with read and write
        access to the repository's self-hosted runners. A PAT (not a one-hour
        registration token) is required because {option}`ephemeral` runners
        mint a fresh registration on every restart. The file must contain
        exactly one line and never enters the Nix store.
      '';
    };

    count = mkOption {
      type = types.ints.positive;
      default = 2;
      description = ''
        How many runners register in parallel. Each processes one job at a
        time, so this is the host's CI job concurrency.
      '';
    };

    labels = mkOption {
      type = types.listOf types.str;
      default = [ "nix" ];
      description = ''
        Extra labels appended to every runner. A workflow opts in by setting
        {option}`runs-on` to `self-hosted` plus these labels.
      '';
    };

    ephemeral = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Register single-use runners that de-register after one job and
        re-register on restart. This isolates jobs from each other while the
        host's `/nix/store` cache survives the per-job wipe, so caching is
        unaffected. Requires {option}`tokenFile` to hold a PAT. Disable only
        for a deliberate long-lived runner.
      '';
    };

    packages = mkOption {
      type = types.listOf types.package;
      default = [ ];
      example = lib.literalExpression "[ pkgs.gh pkgs.jq ]";
      description = ''
        Extra packages on each job's PATH, on top of the git, Nix, and Cachix
        tooling the module always provides.
      '';
    };
  };

  config = mkIf cfg.enable {
    # A warm shared cache is the whole point of a persistent runner: let the
    # daemon substitute index artifacts so jobs skip cold rebuilds. Every key
    # uses the `extra-*` form so it adds to Nix's defaults and any host config
    # rather than replacing them (keeping cache.nixos.org, kvm, etc.).
    nix.settings = {
      extra-experimental-features = [
        "nix-command"
        "flakes"
      ];
      # Consume the repo flake's nixConfig substituters without the interactive
      # prompt; `nix flake check` otherwise stalls waiting for confirmation.
      accept-flake-config = true;
      extra-substituters = [ "https://indexable-inc.cachix.org" ];
      extra-trusted-public-keys = [
        "indexable-inc.cachix.org-1:HQ5mjdOyhgNjLVhjv0qgVMJ5YiO1zEEVMAtF9mTcpiI="
      ];
      # Index images pin `gcc.arch = znver5`, so every derivation in the closure
      # requires this builder feature; advertise it or the daemon refuses the
      # builds before they evaluate.
      extra-system-features = [ "gccarch-znver5" ];
    };

    services.github-runners = lib.genAttrs runnerNames (_name: {
      enable = true;
      inherit (cfg) url tokenFile ephemeral;
      extraLabels = cfg.labels;
      # Re-register under the same name after a host config change instead of
      # failing on a name clash with the already-registered runner.
      replace = true;
      extraPackages = [
        pkgs.cachix
        pkgs.gh
        pkgs.git
        config.nix.package
      ]
      ++ cfg.packages;
    });
  };
}
