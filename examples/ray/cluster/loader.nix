/**
Shared loader inputs for Ray's prebuilt binaries.

Ray's wheel ships a compiled `_raylet.so` (dlopened by the Python process) and
standalone `raylet`/`gcs_server` ELF executables (exec'd by `ray start`). Both
need a modern libstdc++ and zlib; the standalone binaries additionally need
the FHS dynamic linker the image's `nix-ld` provides. This is the single owner
of that library set and the `nix-ld` location, consumed by package.nix
(`runtimeLibraryInputs`), cli.nix (wrapper env), and cluster-node.nix (unit
env).
*/
{
  lib,
  pkgs,
}: let
  libs = [
    pkgs.stdenv.cc.cc.lib
    pkgs.zlib
  ];
  libraryPath = lib.makeLibraryPath libs;
  nixLdDir = "/run/current-system/sw/share/nix-ld/lib";
in {
  inherit libs libraryPath nixLdDir;
  nixLd = "${nixLdDir}/ld.so";
  # The standalone binaries search NIX_LD_LIBRARY_PATH; include the same libs
  # plus the image's default nix-ld bundle.
  nixLdLibraryPath = "${libraryPath}:${nixLdDir}";
}
