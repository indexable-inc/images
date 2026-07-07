{ix, ...}:
# The swift conformance package through `ix.unibind.build`: the compiled
# runner is the package, with the generated Swift sources, the staticlib,
# and the compile-and-run conformance check attached as passthru. The check
# surfaces in CI as `checks.<system>.unibind-conformance-swift-conformance`
# (darwin only, gated by package.nix `systems`).
let
  built = ix.unibind.build {
    crate = "unibind-conformance-swift";
    targets.swift = {
      package = "conformance";
      swiftSource = builtins.path {
        name = "unibind-conformance-swift-runner-source";
        path = ./swift;
      };
    };
  };
in
  built.swift.runner.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit (built.swift) generated library;
        tests =
          (old.passthru.tests or {})
          // {
            inherit (built.swift.tests) conformance;
          };
      };
  })
