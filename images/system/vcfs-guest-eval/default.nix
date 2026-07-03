# Runtime image for ix's VCFS guest benchmark controller.
{pkgs, ...}: {
  ix.image.name = "ix/vcfs-guest-eval";

  environment.systemPackages = [
    pkgs.binutils
    pkgs.cargo
    pkgs.gcc
    pkgs.git
    pkgs.nodejs
    pkgs.pkg-config
    pkgs.pnpm
    pkgs.rustc
    pkgs.sqlite
  ];
}
