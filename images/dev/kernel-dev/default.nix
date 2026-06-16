# Linux kernel dev image: build toolchain and a shallow Linus tree at
# /src/linux. The base profile already brings ripgrep, fd, neovim, gdb, perf.
{ lib, pkgs, ... }:
{
  ix.image.name = lib.mkDefault "linux-kernel-dev";

  environment.systemPackages = [
    pkgs.gnumake
    pkgs.gcc
    pkgs.gnugrep
    pkgs.findutils
  ];

  services.git-clone = {
    enable = true;
    activation = "timer";
    url = "https://github.com/torvalds/linux.git";
    dest = "/src/linux";
  };
}
