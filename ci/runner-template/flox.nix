# Toolchain-baked runner policy for flox-class pools, layered ON
# platform.nix (the flake's `flox` attr imports module.nix + platform.nix +
# this file; never this file alone). Ported from the preview mirror
# indexable-inc/flox, ci/ix-runner-template/flox.nix, reduced to the delta
# over platform.nix: the substrate facts that file restated (cache.ix.dev
# substitution + nix.settings GC headroom, gai.conf v4 preference,
# build-essential parity packages, stateVersion) already live in
# platform.nix with the same values, so they are deliberately not repeated
# here - platform.nix's jobEnvironment values are mkDefault, so this layer
# overrides the ones flox needs different.
#
# Unlike the baml family, flox bakes almost no toolchain: every flox CI step
# runs inside `nix develop`, so the devshell closure carries rust, just,
# pre-commit and the whole bats dependency set (pkgs/flox-cli-tests already
# lists bats, expect, procps, openssh, podman, ...). What the IMAGE has to
# supply is the three things a job cannot give itself on a machine that
# pins `trusted-users = ["root"]`: a substituter, daemon-side build
# parallelism, and hosted-image PATH parity.
#
# EVERY ENTRY BELOW IS EVIDENCE-DRIVEN. Each one is a lane that failed on
# the flox preview pool on 2026-08-21 while it ran the platform default
# `ci-runner` attr instead of this file, with the observation that produced
# it. See indexable-inc/flox HANDOFF.md.
{
  lib,
  pkgs,
  ...
}: {
  nix.settings = {
    # WHY: flox's root flake declares
    # `nixConfig.extra-substituters = ["https://cache.flox.dev"]`, but
    # module.nix pins trusted-users to root, so a job running as the
    # unprivileged `runner` user cannot add a substituter and
    # `--accept-flake-config` is silently ignored for it. Without this every
    # lane source-builds flox's patched Nix and its whole Rust workspace.
    # MEASURED: a stock platform-template guest's nix.conf carries only
    # cache.ix.dev and cache.nixos.org, and the first green lanes took
    # 20-35 minutes each realising rust-stable from source.
    #
    # Substituters concatenate across modules, so platform.nix's two entries
    # stay and this appends the third.
    substituters = ["https://cache.flox.dev"];
    trusted-public-keys = [
      "flox-cache-public-1:7F4OyH7ZCnFhcze3fJdfyXYLQw/aV7GEed86nQ7IsOs="
    ];

    # WHY: THE ENTRY THAT CANNOT BE REPLACED BY A WORKFLOW, and the reason
    # this file has to exist at all rather than living in flox's `env:`.
    #
    # `max-jobs` is a DAEMON setting. A job on a machine pinning
    # trusted-users to root cannot lower it, so the daemon keeps scheduling
    # one derivation per visible core - and an ix guest advertises ~100
    # vCPUs while growing RAM on demand toward a 256 GiB ceiling. `cores`
    # the client can forward per-build; `max-jobs` it cannot.
    #
    # MEASURED on the preview pool, all three on the platform default
    # template: a Nix Plugins lane that PASSED sat at 82 GiB resident; a
    # cli-unit guest faulted and restarted mid-build (VM started_at jumped
    # to 02:29:42 for a job begun 02:22:16); a second cli-unit guest stayed
    # healthy at 62 GiB while the kernel killed the RUNNER process out from
    # under it, which lands on GitHub as a job with null step conclusions
    # and no uploaded log. Setting NIX_BUILD_CORES=8 from the workflow
    # halved the fan-out and did not stop it, because ~100 concurrent
    # derivations was always the larger half.
    #
    # 4 x 8 is the ubuntu-22.04-8core-class shape flox's suites are tuned
    # against, with headroom for the elastic guest to grow into.
    max-jobs = 4;
    cores = 8;
  };

  services.ix-runner = {
    # Delta over platform.nix's build-essential set (gcc, gnumake, cmake,
    # pkg-config, python3, perl, openssl, git-lfs, glibc.bin are already
    # there; extraPackages is a list option that concatenates across
    # modules, so restating them would put duplicate store paths on the
    # job PATH).
    extraPackages = with pkgs; [
      # WHY: GitHub's ubuntu images ship util-linux; neither module.nix's
      # base userland nor platform.nix does, and flox notices.
      # MEASURED: the cli-unit lane's
      # `providers::build::nef_tests::manifest_builds_can_depend_on_nef`
      # runs a generated build.bash that pipes through `rev` and died with
      # "rev: command not found" -> make Error 127 - exactly one failing
      # test out of 1702, entirely an image-parity gap. The preview carries
      # a symlink shim in .github/actions/ix-setup that backfills the same
      # binaries out of the guest closure; that shim exists only because
      # this file could not be used, and should be deleted when this lands.
      util-linux
      # `just` is the entry point of every flox build/test step
      # (`nix develop --command just build-cli`). It arrives via the
      # devshell in practice; baked so a step that shells out to `just`
      # before entering the devshell resolves, and so `ix shell` debugging
      # of a stuck lane can drive the same recipes.
      just
      # `nix` reaches for `ssh` whenever a flake input or store URI is
      # ssh-shaped, and the bats suites shell out to git constantly. git is
      # in module.nix's baseUserland and git-lfs in platform.nix; openssh is
      # in neither.
      openssh
      # `ps`/`pstree` parity: the activation suites inspect process trees.
      # flox-cli-tests carries its own copies, but an operator debugging a
      # hung lane over `ix shell` has no PATH but this one.
      procps
    ];

    # Delta over platform.nix's pins. platform.nix targets baml-class
    # 16-thread lanes; flox's suites are tuned for GitHub's
    # `ubuntu-22.04-8core`, so every parallelism knob drops to 8.
    #
    # These are IMAGE-level defaults and flox's workflow sets the same three
    # Rust values in its own `env:` block, which wins (workflow env
    # overrides unit env). Both on purpose: the workflow copy makes the
    # number deterministic per commit and visible in the diff, this copy
    # keeps a lane that forgets the env block from self-sizing off ~100
    # vCPUs. The workflow cannot express the `max-jobs` half at all.
    jobEnvironment = {
      CARGO_BUILD_JOBS = "8";
      NEXTEST_TEST_THREADS = "8";
      RUST_TEST_THREADS = "8";
      # The bats suites read this to size their `parallel` fan-out.
      FLOX_TEST_JOBS = "8";
      # Set in setup_suite.bash and in flox's ci.yml `env:` already; pinned
      # here too so an `ix shell` debugging session behaves like CI.
      FLOX_DISABLE_METRICS = "true";
      # flox's Rust workspace is deeply generic in places and rustc has
      # been seen to exhaust the default 8 MiB stack. Cheap, no cost unused.
      RUST_MIN_STACK = "67108864";
    };
  };
}
