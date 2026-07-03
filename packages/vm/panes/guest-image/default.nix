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
          (final: prev: {
            inherit (repoPackages) panes-compositor;
            # ./pins.json also carries the mesa src pin, so hand
            # lwjgl-natives.nix only the lwjgl-* entries (it asserts one
            # shared LWJGL version across its pins).
            lwjgl-natives-linux-arm64 = final.callPackage ./lwjgl-natives.nix {
              pins = lib.filterAttrs (name: _: lib.hasPrefix "lwjgl" name) (ix.pins.loadPins ./pins.json);
            };
            # The checked bash writer, threaded through the overlay because
            # `ix` is not in module scope; apps.nix uses it for the Minecraft
            # launch wrapper instead of the unchecked writeShellScript.
            writeBashApplication = ix.writeBashApplication final;
            # Venus (virtio-gpu Vulkan) with the mesa fork's driver-side
            # external-semaphore patch (index#1742): MoltenVK hosts never
            # support SYNC_FD semaphore import, so stock mesa masks
            # VK_KHR_synchronization2 (clamping the device to Vulkan 1.2) and
            # VK_KHR_swapchain. Only `src` is swapped; the fork is upstream
            # tag mesa-26.1.2 plus the patch commit, so nixpkgs' recipe and
            # patches still apply. The fork branch is the single source of
            # truth for the patch (rather than an in-tree .patch file): its
            # history carries the upstreamable commit and the pinned tree is
            # what upstream review sees. `hardware.graphics.enable` in
            # ./nixos.nix consumes `pkgs.mesa`, so this override is what
            # /run/opengl-driver (and the container ICDs) get.
            mesa =
              let
                pin = ix.pins.loadPin ./pins.json "mesa-src";
              in
              prev.mesa.overrideAttrs (old: {
                # The version assert only catches upstream version bumps; a
                # nixpkgs change to mesa's own patch set can also stop
                # applying against the fork tree and force a rebase, and only
                # the build failure catches that case.
                src =
                  assert lib.assertMsg (old.version == pin.version)
                    "panes-guest-image: mesa fork pin is ${pin.version} but nixpkgs mesa is ${old.version}; rebase indexable-inc/mesa branch ix/venus-driver-side-semaphore onto the new upstream tag and re-pin";
                  final.fetchzip { inherit (pin) url hash; };
              });
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
