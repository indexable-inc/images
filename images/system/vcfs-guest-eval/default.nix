# Boot image for ix's VCFS guest benchmark (`vcfs-guest-eval`).
#
# `ix new registry.ix.dev/ix/vcfs-guest-eval:latest` boots a VM from this
# image; the controller uploads the `vcfs-bench` binary and runs the `real`
# suite inside it. Each `real` op shells out to a host tool that must already
# be on the guest PATH (crates/storage/fs/vcfs/guest-bench/src/workloads/mod.rs):
#
#   sqlite.{write_txn,read_query}  -> sqlite3   (pkgs.sqlite)
#   pnpm.{install,build}           -> pnpm/node (pkgs.pnpm, pkgs.nodejs)
#   cargo.build                    -> cargo + a C linker
#                                     (pkgs.cargo, pkgs.rustc, pkgs.gcc,
#                                      pkgs.binutils, pkgs.pkg-config)
#   git.status (macro)             -> git       (pkgs.git)
#
# `nix` (nix.shell / macro ops) and `strace` (the controller wraps the bench
# in strace) come from NixOS and the auto-enabled base profile, so they are
# not repeated here.
{ pkgs, ... }:
{
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
