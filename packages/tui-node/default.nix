{
  ix,
  lib,
  pkgs ? ix.pkgs,
}:
let
  # The N-API addon is already built as a workspace cdylib library, so this is
  # just packaging: rename the shared object to `tui_node.node`, sanitize it,
  # and lay it next to the hand-written JS wrapper and TypeScript types.
  library = ix.rustWorkspace.units.libraries.tui_node;

  npmSource = builtins.path {
    name = "tui-node-npm-source";
    path = ./npm;
  };
in
pkgs.runCommand "ix-tui-node"
  {
    strictDeps = true;
    nativeBuildInputs = [
      pkgs.coreutils
      pkgs.patchelf
      pkgs.removeReferencesTo
    ];
    passthru = { inherit library; };
    meta.description = "ix tui Node.js package (N-API bindings for the tui PTY manager)";
  }
  ''
    set -euo pipefail

    cdylib=""
    for candidate in \
      ${library}/lib/libtui_node.so \
      ${library}/lib/libtui_node-*.so
    do
      if [ -f "$candidate" ]; then
        cdylib="$candidate"
        break
      fi
    done
    if [ -z "$cdylib" ]; then
      echo "tui-node: no cdylib under ${library}/lib" >&2
      ls -la ${library}/lib >&2 || true
      exit 1
    fi

    mkdir -p "$out/native"
    cp ${npmSource}/package.json ${npmSource}/index.js ${npmSource}/index.d.ts "$out/"
    cp "$cdylib" "$out/native/tui_node.node"
    chmod u+w "$out/native/tui_node.node"

    # Strip the build-time rpath and nixpkgs toolchain references so the package
    # is not pinned to this store path.
    if patchelf --print-rpath "$out/native/tui_node.node" >/dev/null 2>&1; then
      patchelf --remove-rpath "$out/native/tui_node.node"
    fi
    remove-references-to \
      -t ${pkgs.glibc} \
      -t ${pkgs.stdenv.cc.cc.lib} \
      "$out/native/tui_node.node"
  ''
