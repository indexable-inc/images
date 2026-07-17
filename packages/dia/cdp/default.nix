# "Dia CDP.app": launches the installed Dia browser with the Chrome DevTools
# Protocol listening on 127.0.0.1:9222. LaunchServices launchers (Raycast,
# Spotlight, Dock) pass no argv, so a wrapper bundle is the only way a normal
# app launch can carry `--remote-debugging-port`; Home Manager links the
# bundle into ~/Applications/Home Manager Apps, which Raycast indexes.
#
# The wrapper targets /Applications/Dia.app (the Homebrew cask install, see
# users/andrewgazelka/darwin.nix) rather than the nix-built `dia` package:
# both would share the same profile directory, and the cask tracks upstream
# releases faster than manifest.json, so pointing the launcher at a possibly
# older nix build risks a Chromium profile-version downgrade.
#
# Chromium reads the flag only at process start: if Dia is already running,
# this launch is forwarded to the existing instance and the flag is dropped,
# so quit Dia first to get the CDP listener.
{
  ix,
  lib,
  pkgs,
  runCommandLocal,
  writeText,
}: let
  port = 9222;
  launcher = ix.writeNushellApplication pkgs {
    name = "dia-cdp";
    text = ''
      # nu
      def main [] {
        exec "/Applications/Dia.app/Contents/MacOS/Dia" --remote-debugging-port=${toString port}
      }
    '';
  };
  infoPlist = writeText "dia-cdp-info-plist" (lib.generators.toPlist {escape = true;} {
    CFBundleDisplayName = "Dia CDP";
    CFBundleExecutable = "dia-cdp";
    CFBundleIdentifier = "dev.ix.dia-cdp";
    CFBundleName = "Dia CDP";
    CFBundlePackageType = "APPL";
  });
in
  runCommandLocal "dia-cdp" {
    meta = {
      description = "Dia CDP.app: launch the installed Dia with --remote-debugging-port=${toString port}";
      platforms = ["aarch64-darwin"];
    };
  } ''
    contents="$out/Applications/Dia CDP.app/Contents"
    mkdir -p "$contents/MacOS"
    cp ${infoPlist} "$contents/Info.plist"
    # Copied, not symlinked: CFBundleExecutable must be a regular executable
    # for LaunchServices to accept the bundle.
    cp ${lib.getExe launcher} "$contents/MacOS/dia-cdp"
  ''
