{ix, ...}:
# scipql-py through `ix.unibind.build`: the wheel is the package (Linux-only,
# see package.nix), with the module/site/library outputs and the strict type
# gate attached as passthru. Same build arguments as the module bundle in
# packages/mcp/default.nix (scipqlModule); keep the two call sites in sync.
let
  built = ix.unibind.build {
    crate = "scipql-py";
    targets.py = {
      package = "scipql";
      pythonSource = builtins.path {
        name = "scipql-py-python-source";
        path = ./python;
      };
      pythonPackages = ps: [ps.polars];
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
