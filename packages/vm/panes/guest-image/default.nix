# Build the raw EFI-bootable aarch64 NixOS disk for the panes seamless-windows
# guest (index#1686): a headless Wayland compositor (`panes-compositor`)
# exporting each toplevel over vsock, plus one systemd-nspawn container per app
# from ./apps.nix. Boot it with `vmkit boot-linux --disk <out> --gpu`.
#
# Assembled with systemd-repart (the image/repart module imported by
# ./nixos.nix), which runs in the build sandbox with no qemu/kvm VM, so it
# builds on any plain aarch64-linux builder, same as
# packages/vm/chrome-vm-image. `path` is `pkgs.path` from the package autoArgs.
{
  lib,
  path,
  repoPackages,
  ix,
  nix,
  # Writer for `passthru.updateScript`, bound only on the flake-package path
  # (lib/packages.nix); the overlay path leaves it null so `pkgs.*` carries no
  # updater. Same nullable-writer pattern as vector-bin.
  updateScriptWriter ? null,
  # Public ssh key authorized for root in the guest, enabling the ssh
  # switch-in-place loop (README, "Iterating on the guest"). Deliberately null
  # by default: the image is repo-built and cacheable, so any default key
  # would ship a static root credential to everyone. With null, sshd still
  # runs but nothing can log in; bake your own key via
  # `panes-guest-image.override { sshAuthorizedKey = ...; }`.
  sshAuthorizedKey ? null,
}:
let
  nixos = import "${path}/nixos/lib/eval-config.nix" {
    system = "aarch64-linux";
    modules = [
      ./nixos.nix
      # Inject the repo-built compositor through the package set so the guest
      # module reads it as `pkgs.panes-compositor` (same pattern as
      # packages/vm/vz-linux-guest); its meta lives in one place,
      # packages/vm/panes/compositor. Minecraft's LWJGL natives ride the same
      # overlay because their pins (./pins.json, loaded through `ix`) live
      # outside apps.nix (see ./lwjgl-natives.nix).
      {
        nixpkgs.overlays = [
          (final: _prev: {
            inherit (repoPackages) panes-compositor;
            lwjgl-natives-linux-arm64 = final.callPackage ./lwjgl-natives.nix {
              pins = ix.pins.loadPins ./pins.json;
            };
          })
        ];
        # The builder-chosen root login key (see the sshAuthorizedKey package
        # arg above); an empty list leaves sshd running with no way in.
        users.users.root.openssh.authorizedKeys.keys = lib.optional (
          sshAuthorizedKey != null
        ) sshAuthorizedKey;
      }
    ];
  };
  # Mechanically re-pins the ./pins.json jar hashes from their URLs
  # (`nix run .#update`); bumping the LWJGL version is the human edit.
  updateScript =
    if updateScriptWriter == null then
      null
    else
      ix.pins.mkUpdater {
        writeNushellApplication = updateScriptWriter;
        inherit nix;
        pname = "panes-guest-image";
        relPath = "packages/vm/panes/guest-image/pins.json";
      };
in
# Expose the raw disk directly as the package output (the repart module
# produces it at `${system.build.image}/${image.filePath}`).
nixos.pkgs.runCommand "panes-guest.raw"
  {
    __structuredAttrs = true;
    passthru = {
      # The system closure alone, for the ssh switch-in-place loop (README,
      # "Iterating on the guest"): build
      # `.#packages.aarch64-linux.panes-guest-image.toplevel`, `nix copy` it
      # into the running guest, activate with its switch-to-configuration.
      # Skips the disk assembly entirely.
      toplevel = nixos.config.system.build.toplevel;
    }
    // lib.optionalAttrs (updateScript != null) { inherit updateScript; };
  }
  ''
    cp --sparse=always "${nixos.config.system.build.image}/${nixos.config.image.filePath}" "$out"
  ''
