# Default JVM major used by every helper and module in the repo that
# does not pin its JDK explicitly. Bumping this string is the single
# load-bearing change when retargeting to the next LTS.
#
# OpenJDK 25 is the current LTS (released Sep 2025) and matches the
# JREs the Minecraft, Minestom, and Velocity services pin.
#
# Consumers read this file directly while resolving the package from their
# caller-supplied `pkgs`, so exported NixOS modules still work with a plain
# nixpkgs package set that has not installed the repo overlay.
"25"
