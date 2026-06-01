# Personal-but-shareable home-manager module for github:andrewgazelka.
#
# Hoisted out of the private ~/.config/nix so the reusable parts live in the
# open monorepo where they can be reviewed and shared:
#
#   * the ix.dev "service down or not" watcher (`ix-downtime`), which mirrors
#     the public Better Stack status page onto a per-service Minecraft boss bar
#     plus an Ender Dragon growl + spoken root cause;
#   * the boss bar overlay GUI it drives (`bossbar-overlay`);
#   * the merged-PR + CI-failure watcher (`pr-watch`), the visual half of the
#     "karma feed": each PR newly merged to main floats a labelled Minecraft XP
#     orb up the screen (with the orb pickup sound), and each newly failed main
#     Actions run pops a grey angry-villager puff (with the villager "no" sound),
#     both onto the `merge-orb-feed` overlay. A failure also launches a silent
#     detached Opus deep dive (`ci-triage`) that files a deduped Linear ticket.
#     Nothing is spoken;
#   * the full-screen karma feed overlay it announces onto (`merge-orb-feed`),
#     which renders both pop kinds and plays their per-kind sounds;
#   * the CI progress bars (`services.ciBars`, the reusable
#     packages/bossbar-overlay/ci-bars-home-module.nix), which draw one Minecraft
#     boss bar per in-flight GitHub Actions run across our repos (green, filled by
#     elapsed / average-duration; purple while a run is still queued or not yet
#     picked up by a runner) and clear each as the run finishes. Silent (pr-watch
#     owns the CI sounds) and a different palette from the ix-downtime outage bars
#     (red/yellow/blue) so the two never read alike. This module just imports that
#     component and turns it on with our repos;
#   * the token-free `/optimize` history scan (`optimize-scan`);
#   * the shared "play a gentle sound, then speak it detached" helper
#     (`say-detached`), now used only by the ix-downtime watcher.
#
# Each is declared as `services.portable.<name>` so they render to a native
# launchd agent on macOS and a native systemd user unit on Linux from one spec
# (index lib/portable-services.nix, imported below). The module is a function
# over the index flake's per-system package set so it can resolve the `bossbar` /
# `bossbar-overlay` / `minecraft-sound` derivations (flake outputs, not all in
# the nixpkgs overlay) for the host it is evaluated on.
#
# Host-specific glue stays in the consuming config: the Better Stack API token
# and the pr-watch Linear key (both seeded into the macOS Keychain, or exported
# as env), `gh` auth for pr-watch, and any absolute log paths beyond the defaults
# here.
#
# Closed over the index flake's per-system package set (`indexPackages system`)
# and the portable-services home-manager module, so the consumer imports just
# this one module and gets everything wired.
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
  cfg = config.users.andrewgazelka;

  indexPkgs = indexPackages pkgs.stdenv.hostPlatform.system;

  isDarwin = pkgs.stdenv.hostPlatform.isDarwin;

  # macOS speaks through the built-in `say`; Linux falls back to a configurable
  # speech command (speech-dispatcher's `spd-say` by default). Baked into the
  # say-detached body via the @SAY_CMD@ placeholder.
  sayCommand = if isDarwin then "/usr/bin/say" else cfg.sound.linuxSayCommand;

  # Checked replacement for the lint-banned writeShellApplication: writeTextFile
  # gives us a real bash script, runtimeInputs are prepended to PATH like
  # writeShellApplication does, and `bash -n` + shellcheck run in the build so a
  # syntax error or a shellcheck-class bug fails the derivation instead of
  # surfacing at runtime. The script body assumes `set -euo pipefail` and the
  # runtimeInputs on PATH, exactly as writeShellApplication would supply. Same
  # escape hatch as lib/apple-sdk-toolchain.nix's `mkScript`.
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

  # Shared announcement helper: plays an optional Minecraft sound then speaks
  # text, detached into its own session (POSIX setsid via perl) so a switch
  # reloading the calling agent never clips an in-flight speech. The @SAY_CMD@
  # placeholder is baked to the per-OS speech command at build time.
  sayDetached = mkBashApp {
    name = "say-detached";
    runtimeInputs = [ indexPkgs.minecraft-sound ];
    text = builtins.replaceStrings [ "@SAY_CMD@" ] [ sayCommand ] (
      builtins.readFile ./scripts/say-detached.sh
    );
  };

  # The ix.dev downtime watcher itself. bossbar raises/clears the per-service
  # outage bar; say-detached carries the growl + spoken summary; claude-code
  # writes the one-sentence root cause; coreutils provides `timeout`/`date`.
  ixDowntime = mkBashApp {
    name = "ix-downtime";
    runtimeInputs = [
      pkgs.curl
      pkgs.jaq
      pkgs.sqlite
      sayDetached
      pkgs.claude-code
      pkgs.coreutils
      pkgs.perl # the intrinsic non-overlap flock guard at the top of the script
      indexPkgs.bossbar
    ];
    text = builtins.readFile ./scripts/ix-downtime.sh;
  };

  # Token-free periodic refresh of the /optimize history analysis. Runs the
  # `optimize` skill's own bundled polars library headless via `uv run --with
  # polars` (no index MCP session under launchd/systemd, and uv is the clean way
  # to get polars off-session; uv manages its own python). The script path is
  # baked from the skill's asset so the service and the skill share one source of
  # truth. It scans the last 60 days of ~/.claude history into
  # ~/.claude/optimize/{latest.txt, *.parquet} — NO LLM, NO sound — so it is safe
  # to run often; `/optimize` reads these fresh caches for the synthesized report.
  # coreutils provides ls/cp/date/tail for the dated-snapshot rotation.
  optimizeScan = mkBashApp {
    name = "optimize-scan";
    runtimeInputs = [
      pkgs.uv
      pkgs.coreutils
      pkgs.perl # flock(2) for the non-overlap guard (no flock(1) on macOS)
    ];
    text = ''
      OUT="$HOME/.claude/optimize"
      mkdir -p "$OUT"
      # Non-overlap guard: if a prior scan is still running, skip this fire. perl
      # takes an exclusive non-blocking flock on fd 9, which the shell keeps open
      # for the whole run, so the lock auto-releases on exit or crash. Prevents two
      # uv processes racing the parquet/HTML writes when a slow scan overruns the
      # interval (the old personal launchd agent had this via lockArgs).
      exec 9>"$OUT/.scan.lock"
      perl -e 'use Fcntl ":flock"; flock(STDIN, LOCK_EX | LOCK_NB) or exit 1' <&9 || exit 0
      uv run --with polars ${../../skills/optimize/assets/build_history_df.py} \
        --days 60 --out "$OUT" > "$OUT/latest.txt" 2>&1
      cp "$OUT/latest.txt" "$OUT/report-$(date +%F).txt"
      # keep only the 14 most recent dated snapshots
      { ls -1t "$OUT"/report-*.txt 2>/dev/null || true; } | tail -n +15 \
        | while read -r f; do rm -f "$f"; done
    '';
  };

  # Stage-2 of the pr-watch CI response: a per-run DEEP DIVE into a main-branch
  # Actions failure. Launched DETACHED by pr-watch (own session + `timeout`), it
  # uses `claude -p` with Opus 4.8 + the Bash tool to fetch the failed logs,
  # diagnose the root cause, and file (or dedupe) a Linear ENG ticket when the
  # failure is a genuine code break. It is SILENT (the villager pop is the alert),
  # so it needs no say-detached. The Linear API key is read at runtime from the
  # login Keychain (service `pr-watch-linear`); see the script header.
  # CI_TRIAGE_DRY_RUN makes it non-destructive for testing.
  ciTriage = mkBashApp {
    name = "ci-triage";
    runtimeInputs = [
      pkgs.gh
      pkgs.jq
      pkgs.claude-code
      pkgs.coreutils
    ];
    text = builtins.readFile ./scripts/ci-triage.sh;
  };

  # The merged-PR + CI-failure watcher itself. It is the visual half of the karma
  # feed and speaks nothing: a merge queues an XP orb and a failure queues an
  # angry-villager pop in the feed overlay (which plays the per-kind sound).
  # ci-triage is the detached stage-2 deep dive; coreutils provides
  # `timeout`/`date`; perl supplies the intrinsic flock guard and the setsid
  # detach. The @PLACEHOLDERS@ are baked from the options.
  prWatch = mkBashApp {
    name = "pr-watch";
    runtimeInputs = [
      pkgs.gh
      pkgs.jq
      ciTriage
      pkgs.coreutils
      pkgs.perl
    ];
    text =
      builtins.replaceStrings
        [ "@REPOS@" "@ORB_BIN@" "@LOG_DIR@" "@TRIAGE_COOLDOWN@" ]
        [
          # escapeShellArg per value: @REPOS@ lands unquoted in `repos=(@REPOS@)`,
          # so a value with a space or shell metacharacter must carry its own
          # quoting (the option is author-set, but bake safely rather than rely on
          # it).
          (lib.concatMapStringsSep " " lib.escapeShellArg cfg.prWatch.repos)
          # The feed binary: a merge is queued as an XP orb and a CI failure as a
          # villager pop (`<orb> push "<repo>: <title>" [--kind villager]`).
          (lib.getExe' indexPkgs.bossbar-overlay "xp-orb-overlay")
          cfg.logDir
          (toString cfg.prWatch.triageCooldown)
        ]
        (builtins.readFile ./scripts/pr-watch.sh);
  };

  # The CI progress bars are a standalone reusable component, not personal glue:
  # this just composes it (imported below, turned on in config). Anyone can do
  # the same with `services.ciBars = { enable = true; repos = [ ... ]; }`.
  ciBarsModule = import ../../packages/bossbar-overlay/ci-bars-home-module.nix {
    inherit indexPackages portableServicesModule;
  };
in
{
  imports = [
    portableServicesModule
    ciBarsModule
  ];

  options.users.andrewgazelka = {
    enable = lib.mkEnableOption "andrewgazelka's personal services (ix-downtime watcher + boss bar overlay)";

    logDir = lib.mkOption {
      type = lib.types.str;
      default = "${config.home.homeDirectory}/Library/Logs";
      defaultText = lib.literalExpression ''"''${config.home.homeDirectory}/Library/Logs"'';
      description = ''
        Directory the agents append their stdout/stderr logs to. Defaults to the
        macOS `~/Library/Logs` convention; point it elsewhere on Linux.
      '';
    };

    downtime = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Run the ix.dev downtime watcher (polls Better Stack, drives the boss bar + voice).";
      };

      interval = lib.mkOption {
        type = lib.types.ints.positive;
        default = 30;
        description = "Poll the status page every N seconds.";
      };
    };

    bossbarOverlay = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Run the always-on-top Minecraft boss bar overlay GUI the watcher draws onto.";
      };
    };

    mergeOrbOverlay = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Run the full-screen click-through merge feed overlay (`xp-orb-overlay
          feed`) that pr-watch announces merges onto: each merged PR floats a
          labelled Minecraft XP orb up the screen and fades it. Needs a display,
          so disable on headless hosts (pr-watch still queues events harmlessly).
        '';
      };
    };

    optimize = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Run the token-free /optimize history scan on a schedule (refreshes ~/.claude/optimize for the /optimize skill).";
      };

      interval = lib.mkOption {
        type = lib.types.ints.positive;
        default = 3600;
        description = "Re-scan ~/.claude history every N seconds.";
      };
    };

    prWatch = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Run the merged-PR + CI-failure watcher (polls each repo's main for
          newly merged PRs and newly failed Actions runs). Needs `gh` to be
          authenticated for the host user; the stage-2 deep dive additionally
          needs the `pr-watch-linear` Keychain entry to file tickets (optional).
        '';
      };

      interval = lib.mkOption {
        type = lib.types.ints.positive;
        default = 30;
        description = "Poll each watched repo every N seconds.";
      };

      repos = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [
          "indexable-inc/ix"
          "indexable-inc/index"
        ];
        description = "GitHub `owner/name` repos to watch for merges and main CI failures.";
      };

      triageCooldown = lib.mkOption {
        type = lib.types.ints.positive;
        default = 1800;
        description = ''
          Minimum seconds between stage-2 Opus deep dives per repo+workflow, so a
          sustained red main can't spawn a storm of Opus runs or near-duplicate
          tickets. Overridable at runtime via CI_TRIAGE_COOLDOWN.
        '';
      };
    };

    # CI progress bars are configured through the reusable `services.ciBars`
    # module (imported above); this module just turns it on with our repos in
    # `config`. No personal options needed here.

    sound.linuxSayCommand = lib.mkOption {
      type = lib.types.str;
      default = "spd-say";
      description = ''
        Speech command used on Linux (macOS always uses `/usr/bin/say`). It is
        invoked with the text as a single argument; the default is
        speech-dispatcher's `spd-say`.
      '';
    };

    sayDetachedPackage = lib.mkOption {
      type = lib.types.package;
      readOnly = true;
      default = sayDetached;
      defaultText = lib.literalMD "the module's built `say-detached` helper";
      description = ''
        The built `say-detached` helper, exposed so the consuming config can bake
        it onto the PATH of its own host-specific agents (e.g. a local pr-watch)
        as a `runtimeInputs` entry without redefining the sound helper.
      '';
    };

    xpOrbPackage = lib.mkOption {
      type = lib.types.package;
      readOnly = true;
      default = indexPkgs.bossbar-overlay;
      defaultText = lib.literalMD "the index `bossbar-overlay` package (provides `xp-orb-overlay`)";
      description = ''
        The overlay package providing the `xp-orb-overlay` binary, exposed so a
        consuming config can bake it onto the PATH of its own host-specific agents
        (e.g. the nixos-gen-watch deploy watcher) to push karma-feed pops
        (`xp-orb-overlay push ... [--kind villager]`) without re-deriving the
        wgpu/winit workspace.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    # Expose the shared speaker on PATH so the user can announce by hand too.
    home.packages = [ sayDetached ];

    services.portable = lib.mkMerge [
      (lib.mkIf cfg.downtime.enable {
        ix-downtime = {
          description = "ix.dev downtime watcher";
          command = [ (lib.getExe' ixDowntime "ix-downtime") ];
          interval = cfg.downtime.interval;
          standardOutPath = "${cfg.logDir}/ix-downtime.log";
          standardErrorPath = "${cfg.logDir}/ix-downtime.log";
          # No launchd escape hatch needed: the portable layer's `runAtLoad`
          # (default true) fires the first poll immediately even for interval
          # services, and the launchd Label defaults to the space-free home
          # convention. The script's own flock guard prevents overlap.
        };
      })
      (lib.mkIf cfg.optimize.enable {
        optimize-scan = {
          description = "optimize history scan";
          command = [ (lib.getExe' optimizeScan "optimize-scan") ];
          interval = cfg.optimize.interval;
          standardOutPath = "${cfg.logDir}/optimize-scan.log";
          standardErrorPath = "${cfg.logDir}/optimize-scan.log";
          # runAtLoad (default true) gives an immediate first scan on load;
          # Label defaults to the space-free home convention. No escape hatch.
        };
      })
      (lib.mkIf cfg.prWatch.enable {
        pr-watch = {
          description = "merged-PR + CI-failure watcher";
          command = [ (lib.getExe' prWatch "pr-watch") ];
          interval = cfg.prWatch.interval;
          standardOutPath = "${cfg.logDir}/pr-watch.log";
          standardErrorPath = "${cfg.logDir}/pr-watch.log";
          # runAtLoad (default true) fires the first poll immediately; the
          # Label defaults to the space-free home convention. Overlap is handled
          # intrinsically by the script's own flock guard, so no escape hatch.
        };
      })
      (lib.mkIf cfg.bossbarOverlay.enable {
        bossbar-overlay = {
          description = "Minecraft boss bar overlay";
          command = [ (lib.getExe' indexPkgs.bossbar-overlay "bossbar-overlay") ];
          restart = "always";
          standardOutPath = "${cfg.logDir}/bossbar-overlay.log";
          standardErrorPath = "${cfg.logDir}/bossbar-overlay.log";
          # Label defaults to the space-free home convention; no escape hatch.
        };
      })
      (lib.mkIf cfg.mergeOrbOverlay.enable {
        merge-orb-feed = {
          description = "karma feed overlay (XP orb + villager pops)";
          command = [
            (lib.getExe' indexPkgs.bossbar-overlay "xp-orb-overlay")
            "feed"
          ];
          restart = "always";
          # The feed owns presentation, so it plays the per-kind Minecraft sound
          # (orb pickup / villager "no") itself. launchd/systemd units don't
          # inherit the interactive PATH, so pin the absolute `minecraft-sound`.
          environment.ORB_SOUND_CMD = lib.getExe' indexPkgs.minecraft-sound "minecraft-sound";
          standardOutPath = "${cfg.logDir}/merge-orb-feed.log";
          standardErrorPath = "${cfg.logDir}/merge-orb-feed.log";
          # Label defaults to the space-free home convention; no escape hatch.
        };
      })
    ];

    # Compose the reusable CI progress bars (the `services.ciBars` module imported
    # above): one boss bar per in-flight Actions run on our repos. Everything else
    # (script, palette, average-duration logic) lives in that shared component, so
    # this is the whole personal config for it.
    services.ciBars = {
      enable = true;
      repos = cfg.prWatch.repos;
      inherit (cfg) logDir;
    };
  };
}
