# Host-language build glue for unibind-annotated crates. `build { crate;
# targets; }` assembles, per target language, the generated host files and
# distribution artifacts from the crate's already-built cdylib in the shared
# workspace graph. Imported by lib/default.nix and bound per package set
# (`ix.unibind` inside the repo, `index.lib.unibind` from the flake).
{
  lib,
  pkgs,
  packageRegistry,
  rustWorkspace,
  buildPyStrictCheck,
}: let
  buildPy = import ./py.nix {
    inherit lib pkgs packageRegistry rustWorkspace buildPyStrictCheck;
  };

  buildEx = import ./ex.nix {
    inherit lib pkgs packageRegistry rustWorkspace;
  };

  supportedTargets = ["ex" "py"];
in {
  /**
  Build host-language outputs for one unibind-annotated crate.

  - `crate`: the Cargo package name (e.g. `scipql-py`). For the `py`
    target the crate must be marked `pyExtension = true` in its package.nix;
    the marker is what makes the shared workspace inject the darwin
    `dynamic_lookup` link args its cdylib needs (lib/rust/workspace.nix).
    An `ex` crate carries the same flags in its own build.rs instead (see
    packages/unibind/conformance-ex/build.rs).
  - `targets.<language>`: selects and configures each language target: `py`
    (see [./py.nix](./py.nix) for its arguments) and `ex` (see
    [./ex.nix](./ex.nix)); the `ts` target lands with issue #1993.

  Returns one attrset per requested target; `py` is
  `{ wheel; module; pythonSite; library; tests.pyStrict; }` (`wheel` is
  Linux-only and throws when forced on darwin), `ex` is
  `{ mixPackage; generated; library; soname; }` (`mixPackage` is the
  mix-importable tree: generated `lib/`, `priv/native/<soname>`, and the
  caller's mix project overlaid).
  */
  build = {
    crate,
    targets,
  }: let
    unknown = lib.subtractLists supportedTargets (builtins.attrNames targets);
  in
    assert lib.assertMsg (unknown == []) ''
      unibind.lib.build: unsupported target(s) for `${crate}`: ${lib.concatStringsSep ", " unknown}
      Supported: ${lib.concatStringsSep ", " supportedTargets}. (`ts` is issue #1993.)'';
      lib.optionalAttrs (targets ? py) {
        py = buildPy ({inherit crate;} // targets.py);
      }
      // lib.optionalAttrs (targets ? ex) {
        ex = buildEx ({inherit crate;} // targets.ex);
      };
}
