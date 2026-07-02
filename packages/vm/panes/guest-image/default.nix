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
  path,
  repoPackages,
}:
let
  nixos = import "${path}/nixos/lib/eval-config.nix" {
    system = "aarch64-linux";
    modules = [
      ./nixos.nix
      # Inject the repo-built compositor through the package set so the guest
      # module reads it as `pkgs.panes-compositor` (same pattern as
      # packages/vm/vz-linux-guest); its meta lives in one place,
      # packages/vm/panes/compositor.
      {
        nixpkgs.overlays = [
          (_final: _prev: { inherit (repoPackages) panes-compositor; })
        ];
      }
    ];
  };
in
# Expose the raw disk directly as the package output (the repart module
# produces it at `${system.build.image}/${image.filePath}`).
nixos.pkgs.runCommand "panes-guest.raw" { } ''
  cp --sparse=always "${nixos.config.system.build.image}/${nixos.config.image.filePath}" "$out"
''
