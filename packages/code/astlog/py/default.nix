{
  ix,
  pkgs ? ix.pkgs,
}:
# astlog-py has no wheel of its own: the PyO3 cdylib is built by the shared
# cargo-unit workspace graph and bundled into the ix-mcp interpreter by
# packages/mcp/default.nix. The only thing to package here is the strict
# type/annotation gate over the Python source, so this derivation *is* that
# check (zuban --strict + ruff ANN), mirroring buildUvApplication's
# pyChecker="zuban" path. It is also exposed as `passthru.tests.pyStrict` so it
# joins the CI check set as `checks.<system>.astlog-py-pyStrict`.
let
  pythonSource = builtins.path {
    name = "astlog-py-python-source";
    path = ./python;
  };

  # The sources import polars; the `_astlog.pyi` stub travels with the tree.
  pyStrict = ix.buildPyStrictCheck pkgs {
    pname = "astlog-py";
    pythonSrc = pythonSource;
    pythonPackages = ps: [ps.polars];
  };
in
  pyStrict.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or {})
          // {
            inherit pyStrict;
          };
      };
  })
