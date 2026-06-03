{
  ix,
  lib,
  pkgs ? ix.pkgs,
}:
let
  pyproject = lib.importTOML ./pyproject.toml;
  inherit (pyproject.project) version;

  # The PyO3 cdylib is already built by the shared workspace unit graph (the same
  # one mcp selects its binary from), so the wheel is just packaging: no maturin,
  # no second compile.
  library = ix.rustWorkspace.units.libraries.polars_mixedbread;

  inherit (pkgs.stdenv.hostPlatform) isDarwin system;

  # The wheel is consumed inside this nix env (the MCP Python session imports
  # it), so it is not a portable manylinux/macosx wheel; the tag only has to name
  # the host platform. On Linux we still strip the rpath/toolchain refs (cheap,
  # keeps the closure honest); on darwin a dlopen'd module needs no install-name
  # fixups, so the dylib ships as-is.
  platformTag =
    {
      x86_64-linux = "manylinux_2_34_x86_64";
      aarch64-linux = "manylinux_2_34_aarch64";
      x86_64-darwin = "macosx_10_12_x86_64";
      aarch64-darwin = "macosx_11_0_arm64";
    }
    .${system} or (throw "polars-mixedbread: unsupported system ${system}");

  pythonSource = builtins.path {
    name = "polars-mixedbread-python-source";
    path = ./python;
  };

  wheel =
    pkgs.runCommand "polars-mixedbread-wheel"
      {
        strictDeps = true;
        nativeBuildInputs = [
          pkgs.coreutils
          pkgs.python3
        ]
        ++ lib.optionals (!isDarwin) [
          pkgs.patchelf
          pkgs.removeReferencesTo
        ];
        passthru = { inherit library; };
        meta.description = "polars-mixedbread Python wheel (PyO3 IO source over Mixedbread search)";
      }
      ''
        set -euo pipefail

        cdylib=""
        for candidate in \
          ${library}/lib/libpolars_mixedbread.so \
          ${library}/lib/libpolars_mixedbread-*.so \
          ${library}/lib/libpolars_mixedbread.dylib \
          ${library}/lib/libpolars_mixedbread-*.dylib
        do
          if [ -f "$candidate" ]; then
            cdylib="$candidate"
            break
          fi
        done
        if [ -z "$cdylib" ]; then
          echo "polars-mixedbread: no cdylib under ${library}/lib" >&2
          ls -la ${library}/lib >&2 || true
          exit 1
        fi

        sanitized="$TMPDIR/$(basename "$cdylib")"
        cp "$cdylib" "$sanitized"
        chmod u+w "$sanitized"

        ${lib.optionalString (!isDarwin) ''
          # Strip the build-time rpath and nixpkgs toolchain references so the wheel
          # is not pinned to this store path.
          if patchelf --print-rpath "$sanitized" >/dev/null 2>&1; then
            patchelf --remove-rpath "$sanitized"
          fi
          remove-references-to \
            -t ${pkgs.glibc} \
            -t ${pkgs.stdenv.cc.cc.lib} \
            "$sanitized"
        ''}

        mkdir -p "$out"
        python3 ${./wheel/mkwheel.py} \
          --cdylib "$sanitized" \
          --python-src ${pythonSource} \
          --version ${version} \
          --platform-tag ${platformTag} \
          --out "$out"
      '';

  # Predicate pushdown is the riskiest logic (a polarity bug here silently drops
  # rows), and it is pure Python: it depends only on Polars, not the cdylib. So
  # gate it with an offline test that loads the `_pushdown` module by path and
  # asserts the superset invariant, no built wheel or network required.
  pushdownTest =
    pkgs.runCommand "polars-mixedbread-pushdown-test"
      {
        strictDeps = true;
        nativeBuildInputs = [ (pkgs.python3.withPackages (ps: [ ps.polars ])) ];
      }
      ''
        python3 ${./tests/test_pushdown.py} ${./python/polars_mixedbread/_pushdown.py}
        mkdir -p "$out"
      '';
in
wheel.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = (old.passthru.tests or { }) // {
      pushdown = pushdownTest;
    };
  };
})
