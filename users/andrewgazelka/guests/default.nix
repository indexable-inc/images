# Personal vmkit macOS guests: data only. The machinery (plist rendering,
# ssh push, launchd bootstrap) is modules/home/macos-guests.nix, imported
# alongside this module by profiles/darwin-home.nix.
#
# macos-primary runs the Beeper iMessage bridge (Linear ENG-7746; an interim
# consumer increment of index#2682). The agent below mirrors the runtime
# state validated live on the guest 2026-07-14. Bridge registration with
# secrets (`~/Library/Application Support/bbctl/prod/sh-imessage/`) lives
# only on the guest and is never rendered from nix; see README.md for the
# one-time bootstrap (bbctl login, TCC grants).
{indexPackages}: {
  lib,
  pkgs,
  ...
}: let
  # bbctl is aarch64-darwin only; this module is imported by the
  # darwin-only home profile, so the reference never forces on Linux.
  indexPkgs = indexPackages pkgs.stdenv.hostPlatform.system;
in {
  macosGuests.macos-primary = {
    lifecycle.macAddress = "0e:c9:c7:6c:25:a8";
    ssh = {
      host = "192.168.64.6";
      user = "ix";
    };
    launchAgents."com.beeper.sh-imessage".config = {
      ProgramArguments = [
        "/Users/ix/.local/bin/bbctl"
        "run"
        "--param"
        "imessage_platform=mac"
        "-n"
        "sh-imessage"
      ];
      EnvironmentVariables = {
        HOME = "/Users/ix";
        # launchd agents get the bare system PATH; bbctl re-execs the bridge
        # binary it manages out of ~/.local/bin, so that must come first.
        PATH = "/Users/ix/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
      };
      RunAtLoad = true;
      KeepAlive = true;
      ProcessType = "Background";
      # launchd block-buffers this log, so silence in it is normal.
      StandardOutPath = "/tmp/imsg.log";
      StandardErrorPath = "/tmp/imsg.log";
    };
    # Version + hash pin: packages/bbctl/pins.json (bump version/url there,
    # then `nix run .#update` re-pins the hash).
    binaries.bbctl.source = lib.getExe indexPkgs.bbctl;
  };
}
