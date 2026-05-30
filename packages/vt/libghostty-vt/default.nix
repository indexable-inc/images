# libghostty-vt: ghostty's VT engine built as a standalone C library.
#
# The build recipe (zon2nix-vendored deps, the `-Demit-lib-vt=true` zig build,
# and the darwin SDK shim) lives in `lib/libghostty-vt.nix` so the Rust
# workspace can reuse the exact same artifact when linking `ix-vt-sys`. This
# package is the thin flake-output wrapper plus a smoke test.
{
  ix,
  pkgs,
  ghostty,
  ...
}:
let
  inherit (pkgs) lib;
  package = ix.buildLibghosttyVt pkgs { ghosttySource = ghostty; };

  # Confirm the build emitted the artifacts `ix-vt-sys` links against and the
  # headers `bindgen` parses, rather than re-asserting the build recipe.
  layout =
    pkgs.runCommand "libghostty-vt-layout"
      {
        strictDeps = true;
        nativeBuildInputs = lib.optional pkgs.stdenv.hostPlatform.isDarwin pkgs.darwin.cctools;
      }
      ''
        sharedExt=${if pkgs.stdenv.hostPlatform.isDarwin then "dylib" else "so"}

        test -f ${package}/lib/libghostty-vt.a
        test -f ${package}/include/ghostty/vt.h
        test -d ${package}/include/ghostty/vt

        # A versioned self-contained shared library (libghostty-vt.<ver>.<ext>)
        # is what ix-vt-sys links; assert one exists rather than the bare
        # symlink so a build that emits only the static archive still fails.
        if ! find ${package}/lib -name "libghostty-vt.*.$sharedExt" -type f | grep -q .; then
          echo "no self-contained shared library under ${package}/lib" >&2
          ls -la ${package}/lib >&2
          exit 1
        fi

        mkdir -p "$out"
      '';
in
package.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = (old.passthru.tests or { }) // {
      inherit layout;
    };
  };
})
