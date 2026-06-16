# Neovim Linux CI image: toolchain and test dependencies for ix-backed jobs.
{ lib, pkgs, ... }:
let
  python = pkgs.python3.withPackages (ps: [
    ps.pynvim
  ]);
in
{
  ix.image.name = lib.mkOptionDefault "neovim-ci";

  environment.systemPackages = [
    pkgs.attr
    pkgs.cmake
    pkgs.diffutils
    pkgs.fish
    pkgs.gcc
    pkgs.gettext
    pkgs.glibcLocales
    pkgs.gnumake
    pkgs.inotify-tools
    pkgs.lua51Packages.lpeg
    pkgs.lua51Packages.luafilesystem
    pkgs.lua51Packages.luv
    pkgs.luajit
    pkgs.ninja
    pkgs.nodejs
    pkgs.perl
    pkgs.perlPackages.Appcpanminus
    pkgs.perlPackages.NeovimExt
    pkgs.pkg-config
    pkgs.ruby
    pkgs.shellcheck
    pkgs.stylua
    pkgs.ts_query_ls
    pkgs.unzip
    pkgs.xdg-utils
    pkgs.zig

    python

    pkgs.llvmPackages_21.clang
    pkgs.llvmPackages_21.clang-tools
  ];
}
