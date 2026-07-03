{
  ix,
  lib,
  pkgs ? ix.pkgs,
}: let
  pyproject = lib.importTOML ./pyproject.toml;
  inherit (pyproject.project) version;

  # The PyO3 cdylib is already built by the shared workspace unit graph (the
  # same one mcp selects its binary from), so the wheel is just packaging: no
  # maturin, no second compile.
  library = ix.rustWorkspace.units.libraries.search_py;

  # Linux-only: the package set restricts search-py to Linux (see
  # package.nix), so only the manylinux tags are reachable here.
  platformTag =
    {
      x86_64-linux = "manylinux_2_34_x86_64";
      aarch64-linux = "manylinux_2_34_aarch64";
    }
    .${
      pkgs.stdenv.hostPlatform.system
    }
      or (throw "search-py: wheel is Linux-only, got ${pkgs.stdenv.hostPlatform.system}");

  pythonSource = builtins.path {
    name = "search-py-python-source";
    path = ./python;
  };

  # Strict type + annotation gate over the Python source (zuban --strict + ruff
  # ANN), mirroring buildUvApplication's pyChecker="zuban" path. The sources
  # resolve `import polars`; the `_search.pyi` stub travels with the tree.
  pyStrictTest = ix.buildPyStrictCheck pkgs {
    pname = "search-py";
    pythonSrc = pythonSource;
    pythonPackages = ps: [ps.polars];
  };
in
  pkgs.runCommand "ix-search-wheel"
  {
    strictDeps = true;
    nativeBuildInputs = [
      pkgs.coreutils
      pkgs.python3
      pkgs.patchelf
      pkgs.removeReferencesTo
    ];
    passthru = {
      inherit library;
      tests.pyStrict = pyStrictTest;
    };
    meta.description = "ix-search Python wheel (PyO3 bindings for content-addressed code search)";
  }
  ''
    set -euo pipefail

    cdylib=""
    for candidate in \
      ${library}/lib/libsearch_py.so \
      ${library}/lib/libsearch_py-*.so \
      ${library}/lib/libsearch_py.dylib \
      ${library}/lib/libsearch_py-*.dylib
    do
      if [ -f "$candidate" ]; then
        cdylib="$candidate"
        break
      fi
    done
    if [ -z "$cdylib" ]; then
      echo "search-py: no cdylib under ${library}/lib" >&2
      ls -la ${library}/lib >&2 || true
      exit 1
    fi

    sanitized="$TMPDIR/$(basename "$cdylib")"
    cp "$cdylib" "$sanitized"
    chmod u+w "$sanitized"

    # Strip the build-time rpath and nixpkgs toolchain references so the wheel
    # is not pinned to this store path.
    if patchelf --print-rpath "$sanitized" >/dev/null 2>&1; then
      patchelf --remove-rpath "$sanitized"
    fi
    remove-references-to \
      -t ${pkgs.glibc} \
      -t ${pkgs.stdenv.cc.cc.lib} \
      "$sanitized"

    mkdir -p "$out"
    python3 ${./wheel/mkwheel.py} \
      --cdylib "$sanitized" \
      --python-src ${pythonSource} \
      --version ${version} \
      --platform-tag ${platformTag} \
      --out "$out"
  ''
