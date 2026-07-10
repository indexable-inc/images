{ix, ...}:
# mc-protocol-py through `ix.unibind.build`: the wheel is the package
# (Linux-only, see package.nix), with the module/site/library outputs and the
# strict type gate attached as passthru. Same shape as scipql-py.
let
  built = ix.unibind.build {
    crate = "mc-protocol-py";
    targets.py = {
      package = "mc_protocol";
      pythonSource = builtins.path {
        name = "mc-protocol-py-python-source";
        path = ./python;
      };
    };
  };
in
  built.py.wheel.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit (built.py) library module pythonSite;
        tests =
          (old.passthru.tests or {})
          // {
            inherit (built.py.tests) pyStrict;
          };
      };
  })
