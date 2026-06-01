# Default JVM major used by every helper and module in the repo that
# does not pin its JDK explicitly. Bumping this string is the single
# load-bearing change when retargeting to the next LTS.
#
# OpenJDK 25 is the current LTS (released Sep 2025) and matches the
# JREs the Minecraft, Minestom, and Velocity services pin.
#
# Two consumers read this file: `lib/languages/{java,scala}` (which
# receive `pkgs` from the caller and may run against a plain nixpkgs)
# and `lib/overlay.nix` (which exposes `pkgs.ixDefaultJre` to NixOS
# modules). Both reach this same string.
"25"
