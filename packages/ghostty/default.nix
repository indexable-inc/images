# ghostty: the full terminal application source, vendored as a fork
# (index#3768). Today this builds only the VT-engine subtree
# (`-Demit-lib-vt=true`, via the shared `lib/build/libghostty-vt.nix`
# recipe) from the *patchable* fork source, proving the fork/patch pipeline
# end-to-end. It deliberately does not (yet) reach the rest of ghostty's
# darwin core -- see the doc comment on `ix.buildLibghosttyVt` for exactly
# which absolute-path Xcode-toolchain calls block that, unconditionally,
# inside the Nix sandbox. Closing that gap (so the follow-up
# surface-teardown patch's `Surface`/`apprt` code actually gets built and
# tested here) is the concrete follow-up work index#3768 calls for.
{
  ix,
  pkgs,
  ...
}: let
  source = ix.patchedSrc {
    name = "ghostty";
    src = ix.ghosttySrc;
    patchDir = ./patches;
  };

  package = ix.buildLibghosttyVt pkgs {
    ghosttySource = source;
    baseSource = ix.ghosttySrc;
  };

  # Confirm the build actually reached the artifacts the package claims
  # rather than re-asserting the recipe.
  layout = pkgs.runCommand "ghostty-layout" {strictDeps = true;} ''
    test -f ${package}/lib/libghostty-vt.a
    test -f ${package}/include/ghostty/vt.h

    mkdir -p "$out"
  '';
in
  package.overrideAttrs (old: {
    pname = "ghostty";

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
