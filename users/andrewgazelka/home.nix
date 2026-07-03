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
#     packages/minecraft/bossbar-overlay/ci-bars-home-module.nix), which draw one Minecraft
#     boss bar per in-flight GitHub Actions run across our repos (green, filled by
#     elapsed / average-duration; purple while a run is still queued or not yet
#     picked up by a runner) and clear each as the run finishes. Silent (pr-watch
#     owns the CI sounds) and a different palette from the ix-downtime outage bars
#     (red/yellow/blue) so the two never read alike. This module just imports that
#     component and turns it on with our repos;
#   * the shared "play a gentle sound, then speak it detached" helper
#     (`say-detached`), now used only by the ix-downtime watcher;
#   * the lifelog recorder (`services.portable.lifelog`): the
#     github:andrewgazelka/lifelog daemon sampling the frontmost app, idle and
#     lock state, ingesting macOS Screen Time (knowledgeC.db), and accepting
#     phone events over a local HTTP API, all into one queryable SQLite db.
#     The package comes from the consuming config (the lifelog flake is not an
#     index input), so this module only carries the wiring.
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
  claudeCodeModule,
  ix,
}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.users.andrewgazelka;

  # The index flake's package set for the *host* system. Use this (not the
  # `pkgs.<name>` overlay attrs) for any index-built rust package: the overlay's
  # `pkgs.claude-code` carries an x86_64-linux `config-launch` even on an
  # aarch64-darwin host, so its install-check drags the whole x86_64-linux cargo
  # unit graph (alsa-sys et al.) into a Mac home build, which real nix cannot
  # place. The agent Home Manager modules use `indexPackages` for the same
  # host-targeted package selection.
  # See indexable-inc/index#1085.
  indexPkgs = indexPackages pkgs.stdenv.hostPlatform.system;
  claudeCode = config.programs.claude-code.finalPackage;

  isDarwin = pkgs.stdenv.hostPlatform.isDarwin;

  # macOS speaks through the built-in `say`; Linux falls back to a configurable
  # speech command (speech-dispatcher's `spd-say` by default). Baked into the
  # say-detached body via the @SAY_CMD@ placeholder.
  sayCommand =
    if isDarwin
    then "/usr/bin/say"
    else cfg.sound.linuxSayCommand;

  # The repo's checked-bash writer (lib/util/writers.nix): these watchers lean
  # on POSIX process control (the perl setsid/flock detach idioms) that is
  # native bash territory, so they use the shared escape hatch instead of
  # Nushell. The script body gets `set -euo pipefail` and runtimeInputs on
  # PATH, and `bash -n` + shellcheck run in the build.
  inherit (ix) writeBashApplication;

  # Shared announcement helper: plays an optional Minecraft sound then speaks
  # text, detached into its own session (POSIX setsid via perl) so a switch
  # reloading the calling agent never clips an in-flight speech. The @SAY_CMD@
  # placeholder is baked to the per-OS speech command at build time.
  sayDetached = writeBashApplication pkgs {
    name = "say-detached";
    runtimeInputs = [indexPkgs.minecraft-sound];
    text = builtins.replaceStrings ["@SAY_CMD@"] [sayCommand] (
      builtins.readFile ./scripts/say-detached.sh
    );
  };

  # The ix.dev downtime watcher itself. bossbar raises/clears the per-service
  # outage bar; say-detached carries the growl + spoken summary; claude-code
  # writes the one-sentence root cause; coreutils provides `timeout`/`date`.
  ixDowntime = writeBashApplication pkgs {
    name = "ix-downtime";
    runtimeInputs = [
      pkgs.curl
      pkgs.jq
      pkgs.sqlite
      sayDetached
      claudeCode
      pkgs.coreutils
      pkgs.perl # the intrinsic non-overlap flock guard at the top of the script
      indexPkgs.bossbar
    ];
    text = builtins.readFile ./scripts/ix-downtime.sh;
  };

  # Stage-2 of the pr-watch CI response: a per-run DEEP DIVE into a main-branch
  # Actions failure. Launched DETACHED by pr-watch (own session + `timeout`), it
  # uses `claude -p` with Opus 4.8 + the Bash tool to fetch the failed logs,
  # diagnose the root cause, and file (or dedupe) a Linear ENG ticket when the
  # failure is a genuine code break. It is SILENT (the villager pop is the alert),
  # so it needs no say-detached. The Linear API key is read at runtime from the
  # login Keychain (service `pr-watch-linear`); see the script header.
  # CI_TRIAGE_DRY_RUN makes it non-destructive for testing.
  ciTriage = writeBashApplication pkgs {
    name = "ci-triage";
    runtimeInputs = [
      pkgs.gh
      pkgs.jq
      claudeCode
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
  prWatch = writeBashApplication pkgs {
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
      ["@REPOS@" "@ORB_BIN@" "@LOG_DIR@" "@TRIAGE_COOLDOWN@"]
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
  ciBarsModule =
    import (ix.paths.packagesRoot + "/minecraft/bossbar-overlay/ci-bars-home-module.nix")
    {
      inherit indexPackages portableServicesModule ix;
    };
in {
  imports = [
    portableServicesModule
    ciBarsModule
    claudeCodeModule
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
      scale = lib.mkOption {
        type = lib.types.numbers.positive;
        default = 1.25;
        description = ''
          Pixel scale for the boss bar sprites (and, proportionally, their title
          text and pop-down panels), multiplied by the monitor scale factor.
          Fractional values are honored, so 1.25 renders the bars 25% larger than
          1.0. The overlay binary's own default is 2.
        '';
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

    lifelog = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Run the lifelog recorder (github:andrewgazelka/lifelog): samples the
          frontmost app / idle / lock state into SQLite, ingests macOS Screen
          Time data from knowledgeC.db (that part needs Full Disk Access for
          the binary), and accepts phone events over a local HTTP API.
          Requires `lifelog.package`.
        '';
      };

      package = lib.mkOption {
        type = lib.types.nullOr lib.types.package;
        default = null;
        description = ''
          The lifelog package. Supplied by the consuming config (e.g.
          `inputs.lifelog.packages.''${system}.lifelog`); the lifelog flake is
          deliberately not an index input.
        '';
      };

      listen = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = "127.0.0.1:5599";
        description = ''
          Bind address for the phone-event ingest API; null disables it. Put a
          bearer token in LIFELOG_TOKEN (via `lifelog.environment`, or a
          wrapper that reads the Keychain) before binding beyond localhost.
        '';
      };

      environment = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        default = {};
        description = ''
          Extra environment for the recorder (e.g. LIFELOG_TOKEN). Rendered
          into the world-readable Nix store, so do not inline real secrets.
        '';
      };

      extraArgs = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        example = [
          "--interval-secs"
          "10"
        ];
        description = "Extra arguments appended to `lifelog record`.";
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
    assertions = [
      {
        assertion = cfg.lifelog.enable -> cfg.lifelog.package != null;
        message = "users.andrewgazelka.lifelog: set lifelog.package (the lifelog flake's package) when enabling.";
      }
    ];

    # Expose the shared speaker on PATH so the user can announce by hand too.
    home.packages = [sayDetached];

    services.portable = lib.mkMerge [
      (lib.mkIf cfg.downtime.enable {
        ix-downtime = {
          description = "ix.dev downtime watcher";
          command = [(lib.getExe' ixDowntime "ix-downtime")];
          interval = cfg.downtime.interval;
          standardOutPath = "${cfg.logDir}/ix-downtime.log";
          standardErrorPath = "${cfg.logDir}/ix-downtime.log";
          # No launchd escape hatch needed: the portable layer's `runAtLoad`
          # (default true) fires the first poll immediately even for interval
          # services, and the launchd Label defaults to the space-free home
          # convention. The script's own flock guard prevents overlap.
        };
      })
      (lib.mkIf cfg.prWatch.enable {
        pr-watch = {
          description = "merged-PR + CI-failure watcher";
          command = [(lib.getExe' prWatch "pr-watch")];
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
          command = [
            (lib.getExe' indexPkgs.bossbar-overlay "bossbar-overlay")
            "--scale"
            (toString cfg.bossbarOverlay.scale)
          ];
          restart = "always";
          standardOutPath = "${cfg.logDir}/bossbar-overlay.log";
          standardErrorPath = "${cfg.logDir}/bossbar-overlay.log";
          # Label defaults to the space-free home convention; no escape hatch.
        };
      })
      (lib.mkIf cfg.lifelog.enable {
        lifelog = {
          description = "lifelog activity recorder";
          command =
            [
              (lib.getExe cfg.lifelog.package)
              "record"
            ]
            ++ (
              if cfg.lifelog.listen == null
              then ["--no-listen"]
              else [
                "--listen"
                cfg.lifelog.listen
              ]
            )
            ++ cfg.lifelog.extraArgs;
          # Long-lived daemon: relaunch on any exit so recording survives
          # crashes; the recorder's gap-aware span logic absorbs the restart.
          restart = "always";
          environment = cfg.lifelog.environment;
          standardOutPath = "${cfg.logDir}/lifelog.log";
          standardErrorPath = "${cfg.logDir}/lifelog.log";
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

    programs.claude-code = {
      enable = true;
      personalStartupContext = true;
    };
  };
}
