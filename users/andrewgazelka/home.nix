# Personal-but-shareable home-manager module for github:andrewgazelka.
#
# Hoisted out of the private ~/.config/nix so the reusable parts live in the
# open monorepo where they can be reviewed and shared:
#
#   * the ix.dev "service down or not" watcher (`ix-downtime`), which mirrors
#     the public Better Stack status page onto a per-service Minecraft boss bar
#     plus an Ender Dragon growl + spoken root cause;
#   * the boss bar overlay GUI it drives (`bossbar-overlay`);
#   * the shared "play a gentle sound, then speak it detached" helper
#     (`say-detached`) the watcher announces through.
#
# All three are declared as `services.portable.<name>` so they render to a
# native launchd agent on macOS and a native systemd user unit on Linux from one
# spec (index lib/portable-services.nix, imported below). The module is a
# function over the index flake's per-system package set so it can resolve the
# `bossbar` / `bossbar-overlay` / `minecraft-sound` derivations (flake outputs,
# not all in the nixpkgs overlay) for the host it is evaluated on.
#
# Host-specific glue stays in the consuming config: the Better Stack API token
# (seeded into the macOS Keychain, or exported as IX_BETTERSTACK_TOKEN) and any
# absolute log paths beyond the defaults here.
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
in
{
  imports = [ portableServicesModule ];

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

      label = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = "org.nix-community.home.ix-downtime";
        description = ''
          launchd Label for the watcher agent. Pinned to the home-manager
          reverse-DNS convention: it keeps the plist filename
          (`<Label>.plist`) free of spaces and lets a switch update the
          existing agent in place. Null leaves the portable layer's default
          (`description`), which would otherwise be the human title.
        '';
      };
    };

    bossbarOverlay = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Run the always-on-top Minecraft boss bar overlay GUI the watcher draws onto.";
      };

      label = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = "org.nix-community.home.bossbar-overlay";
        description = ''
          launchd Label for the overlay agent. Pinned to the home-manager
          convention so this updates an existing agent in place instead of
          orphaning the old plist. Null leaves the portable layer's default.
        '';
      };
    };

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
          # Pin the launchd Label (off `description`, which has spaces) to the
          # home-manager convention so the plist filename stays space-free and a
          # switch updates the existing agent in place. RunAtLoad fires the first
          # poll immediately on switch/login instead of waiting one interval
          # (the portable layer omits it for interval services); the script's
          # own flock guard keeps a slow poll from overlapping the next fire.
          launchd.config = {
            RunAtLoad = true;
          }
          // lib.optionalAttrs (cfg.downtime.label != null) {
            Label = cfg.downtime.label;
          };
        };
      })
      (lib.mkIf cfg.bossbarOverlay.enable {
        bossbar-overlay = {
          description = "Minecraft boss bar overlay";
          command = [ (lib.getExe' indexPkgs.bossbar-overlay "bossbar-overlay") ];
          restart = "always";
          standardOutPath = "${cfg.logDir}/bossbar-overlay.log";
          standardErrorPath = "${cfg.logDir}/bossbar-overlay.log";
          # Pin the launchd Label so this updates the existing agent in place
          # instead of orphaning the old `org.nix-community.home.*` plist.
          launchd.config = lib.optionalAttrs (cfg.bossbarOverlay.label != null) {
            Label = cfg.bossbarOverlay.label;
          };
        };
      })
    ];
  };
}
