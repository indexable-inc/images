# Declarative management of the iMessage bridge inside the vmkit macOS guest
# (ENG-7746; interim consumer increment of index#2682): render the launchd
# agent plist and install `imessage-guest-apply`, which pushes the pinned
# bbctl binary (packages/bbctl) plus the plist to the guest over ssh and
# (re)loads the agent.
#
# The guest itself stays a stateful pet until #2682's mkMacGuest machinery
# lands: VM creation, Apple ID sign-in, the interactive `bbctl login`, and the
# GUI-only TCC grants are manual bootstrap (see README.md). Everything built
# from bytes we control (the bridge binary and its launchd agent) is declared
# here and applied idempotently. Bridge state with secrets
# (`~/Library/Application Support/bbctl/prod/sh-imessage/`) lives only on the
# guest and is never rendered from nix.
{
  indexPackages,
  ix,
}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.users.andrewgazelka.imessageGuest;

  # Host-system index package set; bbctl is aarch64-darwin only, so every
  # reference stays under `cfg.enable` (default off) and non-darwin evals
  # never force it.
  indexPkgs = indexPackages pkgs.stdenv.hostPlatform.system;

  label = "com.beeper.sh-imessage";
  bridge = "sh-imessage";
  guestHome = "/Users/${cfg.user}";
  agentPath = "${guestHome}/Library/LaunchAgents/${label}.plist";

  # One renderer (lib.generators.toPlist) over a typed attrset, never
  # hand-written XML. Mirrors the runtime state validated on the guest
  # 2026-07-14 (ENG-7746): under launchd, bbctl is the TCC-responsible
  # process, so the grants in README.md attach to this agent, not Terminal.
  agentPlist = pkgs.writeText "${label}.plist" (lib.generators.toPlist {escape = true;} {
    Label = label;
    ProgramArguments = [
      "${guestHome}/.local/bin/bbctl"
      "run"
      "--param"
      "imessage_platform=mac"
      "-n"
      bridge
    ];
    EnvironmentVariables = {
      HOME = guestHome;
      # launchd agents get the bare system PATH; bbctl re-execs the bridge
      # binary it manages out of ~/.local/bin, so that must come first.
      PATH = "${guestHome}/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
    };
    RunAtLoad = true;
    KeepAlive = true;
    ProcessType = "Background";
    # launchd block-buffers this log, so silence in it is normal.
    StandardOutPath = "/tmp/imsg.log";
    StandardErrorPath = "/tmp/imsg.log";
  });

  apply = ix.writeNushellApplication pkgs {
    name = "imessage-guest-apply";
    runtimeInputs = [pkgs.openssh];
    meta.description = "Push the pinned bbctl + launchd agent to the iMessage guest and (re)load it";
    text = ''
      # nu
      # Idempotent apply: install bbctl and the agent plist on the guest, then
      # bootout (only if loaded) + bootstrap so launchd picks up the new plist.
      # Fails loudly on any hop; the terminal check reads the agent's live pid.
      def main [] {
        const target = "${cfg.user}@${cfg.host}"
        const bbctl = "${lib.getExe indexPkgs.bbctl}"
        const plist = "${agentPlist}"
        const agent_path = "${agentPath}"
        const label = "${label}"
        const home = "${guestHome}"

        ^ssh $target $"mkdir -p ($home)/.local/bin ($home)/Library/LaunchAgents"

        # Stage-then-rename so a KeepAlive respawn never execs a half-copied
        # binary; the running process keeps its old inode until the reload.
        ^scp -q $bbctl $"($target):($home)/.local/bin/.bbctl.staged"
        ^ssh $target $"chmod 755 ($home)/.local/bin/.bbctl.staged && mv -f ($home)/.local/bin/.bbctl.staged ($home)/.local/bin/bbctl"
        ^scp -q $plist $"($target):($agent_path)"

        let uid = ^ssh $target "id -u" | str trim
        let service = $"gui/($uid)/($label)"

        # `launchctl bootout` of an absent service is an error, not
        # idempotence, so probe with `print` first instead of swallowing it.
        # bootout is also asynchronous: it returns while launchd is still
        # tearing the job down, and a bootstrap racing that teardown fails
        # with EIO. Poll for the terminal state (service gone) before
        # bootstrapping; the deadline turns a wedged teardown into a loud
        # failure instead of an infinite wait.
        let loaded = (do { ^ssh $target $"launchctl print ($service)" } | complete | get exit_code) == 0
        if $loaded {
          ^ssh $target $"launchctl bootout ($service)"
          mut gone = false
          for _ in 1..20 {
            if (do { ^ssh $target $"launchctl print ($service)" } | complete | get exit_code) != 0 {
              $gone = true
              break
            }
            sleep 250ms
          }
          if not $gone {
            error make {msg: $"($service) still loaded 5s after bootout"}
          }
        }
        ^ssh $target $"launchctl bootstrap gui/($uid) ($agent_path)"

        # Terminal artifact: RunAtLoad must leave the agent running with a pid.
        let pid = ^ssh $target $"launchctl print ($service)" | lines | where $it =~ 'pid = ' | first | str trim
        print $"($label) on ($target): ($pid)"
      }
    '';
  };
in {
  options.users.andrewgazelka.imessageGuest = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Install `imessage-guest-apply`, which pushes the pinned bbctl and the
        rendered launchd agent to the vmkit macOS guest running the Beeper
        iMessage bridge (ENG-7746) and (re)loads it. Off by default: it needs
        ssh reachability to the guest and an aarch64-darwin host.
      '';
    };

    host = lib.mkOption {
      type = lib.types.str;
      default = "192.168.64.6";
      description = "Guest address the apply script sshes to.";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "ix";
      description = "Guest user owning the bridge, its launchd agent, and ~/.local/bin/bbctl.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [apply];
  };
}
