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
  formats,
  ix,
  lib,
  makeBinaryWrapper,
  runCommandLocal,
  writeText,
}: let
  port = 9222;
  dia = "/Applications/Dia.app/Contents/MacOS/Dia";
  # Dia is outside the store and absent during the build, so wrapping it
  # directly is not an option: makeBinaryWrapper asserts its target is an
  # executable file and would fail in the sandbox. config-launch
  # (packages/config-launch) reads its target from this spec at run time, so
  # the bundle still gets a compiled launcher rather than a generated script.
  launchSpec = (formats.json {}).generate "dia-cdp-launch-spec.json" {
    target = dia;
    flags = ["--remote-debugging-port=${toString port}"];
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
    nativeBuildInputs = [makeBinaryWrapper];
    meta = {
      description = "Dia CDP.app: launch the installed Dia with --remote-debugging-port=${toString port}";
      platforms = ["aarch64-darwin"];
    };
  } ''
    contents="$out/Applications/Dia CDP.app/Contents"
    mkdir -p "$contents/MacOS"
    cp ${infoPlist} "$contents/Info.plist"
    # Compiled in place, not symlinked: CFBundleExecutable must be a regular
    # executable for LaunchServices to accept the bundle, and makeBinaryWrapper
    # emits exactly that. `--argv0` because config-launch passes its own argv0
    # through to the target; without it Dia runs under the launcher's store
    # path. Chromium locates its bundle from the executable path rather than
    # argv0, so that is the name in `ps`, not the profile it loads.
    makeBinaryWrapper ${ix.rustWorkspace.units.binaries.config-launch}/bin/config-launch \
      "$contents/MacOS/dia-cdp" \
      --argv0 ${dia} \
      --set IX_LAUNCH_SPEC ${launchSpec}
  ''
