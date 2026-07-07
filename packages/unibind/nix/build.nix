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

  buildTs = import ./ts.nix {
    inherit lib pkgs rustWorkspace;
  };

  buildEx = import ./ex.nix {
    inherit lib pkgs packageRegistry rustWorkspace;
  };

  supportedTargets = [
    "py"
    "ts"
    "ex"
  ];
in {
  /**
  Build host-language outputs for one unibind-annotated crate.

  - `crate`: the Cargo package name (e.g. `scipql-py`). For the `py` target
    the crate must be marked `pyExtension = true` in its package.nix; the
    marker is what makes the shared workspace inject the darwin
    `dynamic_lookup` link args its cdylib needs (lib/rust/workspace.nix).
    napi (`ts`) crates carry a `napi_build::setup()` build.rs instead, and
    an `ex` crate carries the same darwin flags in its own build.rs (see
    packages/unibind/conformance-ex/build.rs).
  - `targets.<language>`: selects and configures each language target: `py`
    (see [./py.nix](./py.nix) for its arguments), `ts` (see
    [./ts.nix](./ts.nix)), and `ex` (see [./ex.nix](./ex.nix)).

  Returns one attrset per requested target; `py` is
  `{ wheel; module; pythonSite; library; tests.pyStrict; }` (`wheel` is
  Linux-only and throws when forced on darwin), `ts` is `{ npm; library; }`
  (`npm` is Linux-only, same policy as the wheel), and `ex` is
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
      Supported: ${lib.concatStringsSep ", " supportedTargets}.'';
      lib.optionalAttrs (targets ? py) {
        py = buildPy ({inherit crate;} // targets.py);
      }
      // lib.optionalAttrs (targets ? ts) {
        ts = buildTs ({inherit crate;} // targets.ts);
      }
      // lib.optionalAttrs (targets ? ex) {
        ex = buildEx ({inherit crate;} // targets.ex);
      };
}
