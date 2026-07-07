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

  buildRs = import ./rs.nix {
    inherit lib pkgs rustWorkspace;
  };

  buildTs = import ./ts.nix {
    inherit lib pkgs rustWorkspace;
  };

  supportedTargets = [
    "py"
    "rust"
    "ts"
  ];
in {
  /**
  Build host-language outputs for one unibind-annotated crate.

  - `crate`: the Cargo package name (e.g. `scipql-py`). For the `py` target
    the crate must be marked `pyExtension = true` in its package.nix; the
    marker is what makes the shared workspace inject the darwin
    `dynamic_lookup` link args its cdylib needs (lib/rust/workspace.nix).
    napi (`ts`) crates carry a `napi_build::setup()` build.rs instead.
  - `targets.<language>`: selects and configures each language target: `py`
    (see [./py.nix](./py.nix) for its arguments), `rust` (see
    [./rs.nix](./rs.nix)), and `ts` (see [./ts.nix](./ts.nix)); the `ex`
    target lands with issue #1995.

  Returns one attrset per requested target; `py` is
  `{ wheel; module; pythonSite; library; tests.pyStrict; }` (`wheel` is
  Linux-only and throws when forced on darwin), `ts` is `{ npm; library; }`
  (`npm` is Linux-only, same policy as the wheel), and `rust` is
  `{ generated; library; }` (`generated` is the emitted client crate's
  source tree).
  */
  build = {
    crate,
    targets,
  }: let
    unknown = lib.subtractLists supportedTargets (builtins.attrNames targets);
  in
    assert lib.assertMsg (unknown == []) ''
      unibind.lib.build: unsupported target(s) for `${crate}`: ${lib.concatStringsSep ", " unknown}
      Supported: ${lib.concatStringsSep ", " supportedTargets}. (`ex` is issue #1995.)'';
      lib.optionalAttrs (targets ? py) {
        py = buildPy ({inherit crate;} // targets.py);
      }
      // lib.optionalAttrs (targets ? rust) {
        rust = buildRs ({inherit crate;} // targets.rust);
      }
      // lib.optionalAttrs (targets ? ts) {
        ts = buildTs ({inherit crate;} // targets.ts);
      };
}
