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
  buildJvm = import ./jvm.nix {
    inherit lib pkgs rustWorkspace;
  };

  buildPy = import ./py.nix {
    inherit lib pkgs packageRegistry rustWorkspace buildPyStrictCheck;
  };

  supportedTargets = ["jvm" "py"];
in {
  /**
  Build host-language outputs for one unibind-annotated crate.

  - `crate`: the Cargo package name (e.g. `scipql-py`). For the `py`
    target the crate must be marked `pyExtension = true` in its package.nix;
    the marker is what makes the shared workspace inject the darwin
    `dynamic_lookup` link args its cdylib needs (lib/rust/workspace.nix).
  - `targets.<language>`: selects and configures each language target:
    `py` (see [./py.nix](./py.nix) for its arguments) and `jvm` (see
    [./jvm.nix](./jvm.nix)); the `ts` target lands with issue #1993 and
    `ex` with issue #1995.

  Returns one attrset per requested target; `py` is
  `{ wheel; module; pythonSite; library; tests.pyStrict; }` (`wheel` is
  Linux-only and throws when forced on darwin), and `jvm` is
  `{ sources; library; }` (`sources` is the unibind-gen output tree of
  `.java`/`.kt` files under `unibind/<module>/`).
  */
  build = {
    crate,
    targets,
  }: let
    unknown = lib.subtractLists supportedTargets (builtins.attrNames targets);
  in
    assert lib.assertMsg (unknown == []) ''
      unibind.lib.build: unsupported target(s) for `${crate}`: ${lib.concatStringsSep ", " unknown}
      Supported: ${lib.concatStringsSep ", " supportedTargets}. (`ts` is issue #1993; `ex` is issue #1995.)'';
      lib.optionalAttrs (targets ? jvm) {
        jvm = buildJvm {inherit crate;};
      }
      // lib.optionalAttrs (targets ? py) {
        py = buildPy ({inherit crate;} // targets.py);
      };
}
