/**
  The Ray command line wrapped for the ix guest.

  `buildUvApplication` wraps only the `ray-demo` entry point, and the bare `ray`
  console script in the venv has neither the loader path its `_raylet.so` needs
  nor a default cluster address. This builds both `ray` and `ray-demo` with the
  same `nix-ld` loader environment the services use, plus a default
  `RAY_ADDRESS` so `ray status` and `ray-demo` attach to the head without flags.

  `ix shell ray-head -- ray status` runs non-interactively and may not source
  the login profile, so the wrapper sets every variable rather than leaning on
  the session environment.
*/
{
  ix,
  lib,
  pkgs,
  rayAddress,
}:
let
  package = import ./package.nix { inherit ix lib pkgs; };
  loader = import ./loader.nix { inherit lib pkgs; };
in
pkgs.runCommand "ray-cli"
  {
    nativeBuildInputs = [ pkgs.makeWrapper ];
    meta.mainProgram = "ray-demo";
  }
  ''
    mkdir -p "$out/bin"
    for entry in ray ray-demo; do
      makeWrapper ${package}/venv/bin/"$entry" "$out/bin/$entry" \
        --prefix LD_LIBRARY_PATH : ${lib.escapeShellArg loader.libraryPath} \
        --set NIX_LD ${lib.escapeShellArg loader.nixLd} \
        --set NIX_LD_LIBRARY_PATH ${lib.escapeShellArg loader.nixLdLibraryPath} \
        --set-default RAY_ADDRESS ${lib.escapeShellArg rayAddress} \
        --set-default RAY_DISABLE_USAGE_STATS 1
    done
  ''
