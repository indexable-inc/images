# home-manager module exposing `services.ciBars`: draw one Minecraft boss bar per
# in-flight GitHub Actions run across a set of repos, as the current user.
#
# This is a reusable, self-contained component: anyone who imports it and sets
#   services.ciBars = { enable = true; repos = [ "owner/name" ]; };
# gets a live CI dashboard on the boss bar overlay, with no personal glue. A
# running run shows a GREEN bar filled by elapsed / the workflow's recent average
# duration; a queued or not-yet-picked-up run shows a thin PURPLE bar. Bars clear
# as runs finish. The palette is deliberately outside the ix-downtime outage
# palette (red/yellow/blue) so CI progress and outages never read alike. It is
# silent by design: the bar fill is the signal.
#
# Closed over the per-system flake package set (to resolve the `bossbar` CLI for
# the host) and the portable-services home module (so one spec renders a native
# launchd agent on macOS and a native systemd user unit + timer on Linux). Same
# shape as packages/indexer/home-module.nix.
#
# Needs `gh` authenticated for the host user, and the boss bar overlay running to
# be visible (on a headless host it just writes harmless overlay rows). The watch
# script itself is the plain, env-driven ./ci-bars.sh.
{
  indexPackages,
  portableServicesModule,
}:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkOption
    mkEnableOption
    mkIf
    types
    ;

  cfg = config.services.ciBars;

  defaultBossbar = (indexPackages pkgs.stdenv.hostPlatform.system).bossbar;

  # Checked replacement for the lint-banned writeShellApplication: writeTextFile
  # gives a real bash script, runtimeInputs are prepended to PATH like
  # writeShellApplication does, and `bash -n` + shellcheck run in the build so a
  # syntax or shellcheck-class bug fails the derivation instead of surfacing at
  # runtime. The body assumes `set -euo pipefail` + runtimeInputs on PATH. Same
  # escape hatch as lib/apple-sdk-toolchain.nix's `mkScript`; kept inline so this
  # module is self-contained and copy-pasteable for other people.
  mkBashApp =
    {
      name,
      text,
      runtimeInputs ? [ ],
    }:
    pkgs.writeTextFile {
      inherit name;
      executable = true;
      destination = "/bin/${name}";
      text = ''
        #!${pkgs.runtimeShell}
        set -euo pipefail
        export PATH=${lib.makeBinPath runtimeInputs}''${PATH:+:$PATH}
        ${text}
      '';
      checkPhase = ''
        ${lib.getExe' pkgs.bash "bash"} -n "$out/bin/${name}"
        ${lib.getExe pkgs.shellcheck} --shell=bash --severity=warning "$out/bin/${name}"
      '';
    };

  ciBars = mkBashApp {
    name = "ci-bars";
    runtimeInputs = [
      pkgs.gh
      pkgs.jq
      pkgs.sqlite
      pkgs.coreutils
      pkgs.perl # the intrinsic flock(2) non-overlap guard (no flock(1) on macOS)
      cfg.package
    ];
    text = builtins.readFile ./ci-bars.sh;
  };
in
{
  imports = [ portableServicesModule ];

  options.services.ciBars = {
    enable = mkEnableOption "the GitHub Actions CI progress boss bars (one bar per in-flight run)";

    package = mkOption {
      type = types.package;
      default = defaultBossbar;
      defaultText = lib.literalExpression "index.packages.\${system}.bossbar";
      description = "The `bossbar` CLI the watcher writes the overlay rows with.";
    };

    repos = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [
        "indexable-inc/ix"
        "indexable-inc/index"
      ];
      description = "GitHub `owner/name` repos to draw in-flight CI progress bars for.";
    };

    interval = mkOption {
      type = types.ints.positive;
      default = 20;
      description = "Poll each watched repo for in-flight runs every N seconds.";
    };

    avgTtl = mkOption {
      type = types.ints.positive;
      default = 3600;
      description = "Seconds to cache a workflow's average successful-run duration before re-deriving it.";
    };

    defaultAvg = mkOption {
      type = types.ints.positive;
      default = 300;
      description = "Fallback average duration (seconds) for a workflow with no green history yet, so its bar still advances at a sane rate.";
    };

    maxBars = mkOption {
      type = types.ints.positive;
      default = 12;
      description = "Cap on the number of bars drawn per repo per poll, so a busy moment cannot flood the screen.";
    };

    logDir = mkOption {
      type = types.str;
      # The platform-conditional resting value is seeded in `config` with
      # mkOptionDefault, keeping this declaration a self-contained option whose
      # docs come from defaultText (a conditional `default` here would make the
      # rendered docs resolve to one branch).
      defaultText = lib.literalMD "`~/Library/Logs` on macOS, `~/.local/state` on Linux";
      description = "Directory the watcher appends its stdout/stderr log to.";
    };
  };

  config = mkIf cfg.enable {
    # Seed the platform-conditional log directory at option-default precedence,
    # so a downstream override still wins and the option declaration stays a
    # self-contained literal.
    services.ciBars.logDir = lib.mkOptionDefault (
      if pkgs.stdenv.hostPlatform.isDarwin then
        "${config.home.homeDirectory}/Library/Logs"
      else
        "${config.home.homeDirectory}/.local/state"
    );

    services.portable.ci-bars = {
      description = "GitHub Actions CI progress boss bars";
      command = [ (lib.getExe' ciBars "ci-bars") ];
      inherit (cfg) interval;
      # All tunables flow in as environment so the script stays a plain file and
      # the options are the single source of truth.
      environment = {
        CI_BARS_REPOS = lib.concatStringsSep " " cfg.repos;
        CI_BARS_AVG_TTL = toString cfg.avgTtl;
        CI_BARS_DEFAULT_AVG = toString cfg.defaultAvg;
        CI_BARS_MAX = toString cfg.maxBars;
      };
      standardOutPath = "${cfg.logDir}/ci-bars.log";
      standardErrorPath = "${cfg.logDir}/ci-bars.log";
      # runAtLoad (default true) draws the bars on load; the script's own flock
      # guard prevents overlap; the Label defaults to the space-free convention.
    };
  };
}
