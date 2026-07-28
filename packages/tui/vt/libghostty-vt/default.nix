# libghostty-vt: ghostty's VT engine built as a standalone C library, from the
# PATCHED fork source (packages/ghostty/patches). The patch series includes
# C-API additions -- per-cell OSC 8 hyperlink URIs (index#3835) -- that
# `ix-vt-sys` binds, so the artifact every consumer links (this flake output,
# the Rust workspace's unit graph, and indexable-inc/ix via
# `index.packages.<system>.libghostty-vt`) must carry them.
#
# The build recipe (zon2nix-vendored deps, the `-Demit-lib-vt=true` zig build,
# and the darwin SDK shim) lives in `lib/build/libghostty-vt.nix` so the Rust
# workspace can reuse the exact same artifact when linking `ix-vt-sys`. This
# package is the thin flake-output wrapper plus a smoke test. ghostty-src is
# the jj megamerge fork tree, already patched.
{
  ix,
  pkgs,
  ...
}: let
  inherit (pkgs) lib;
  package = ix.buildLibghosttyVt pkgs {
    ghosttySource = ix.ghosttySrc;
  };

  # Confirm the build emitted the artifacts `ix-vt-sys` links against and the
  # headers `bindgen` parses, rather than re-asserting the build recipe.
  layout =
    pkgs.runCommand "libghostty-vt-layout"
    {
      strictDeps = true;
      nativeBuildInputs = lib.optional pkgs.stdenv.hostPlatform.isDarwin pkgs.darwin.cctools;
    }
    ''
      sharedExt=${
        if pkgs.stdenv.hostPlatform.isDarwin
        then "dylib"
        else "so"
      }

      test -f ${package}/lib/libghostty-vt.a
      test -f ${package}/include/ghostty/vt.h
      test -d ${package}/include/ghostty/vt

      # The patched header surface is what ix-vt-sys' checked-in bindings
      # were generated from; assert the fork's C-API addition is present so
      # a silent repoint to an unpatched source fails here, not at ix link
      # time.
      grep -q GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HYPERLINK_URI_LEN \
        ${package}/include/ghostty/vt/render.h

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
    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or {})
          // {
            inherit layout;
          };
      };
  })
