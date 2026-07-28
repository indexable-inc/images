{repoRoot}: {
  pkgs,
  rustToolchain,
}: let
  inherit (pkgs) lib;

  lists = import (repoRoot + "/lib/util/lists.nix") {inherit lib;};
  pins = import (repoRoot + "/lib/util/pins.nix") {inherit lib;};
  ruffAnnArgs = import (repoRoot + "/lib/ruff-ann.nix") {
    inherit lib;
    ruffToml = repoRoot + "/ruff.toml";
  };
  writers = import (repoRoot + "/lib/util/writers.nix") {
    inherit lib;
    inherit (ruffAnnArgs) ruffAnnArgs;
  };
  rust = import (repoRoot + "/lib/rust/build.nix") {
    inherit
      lib
      pkgs
      rustToolchain
      lists
      pins
      ;
    # The owner repository type-checks these scripts. Consumers should not
    # rebuild that policy closure with their unrelated nixpkgs revision.
    writePythonApplication = args:
      writers.writePythonApplication pkgs (args // {check = false;});
    evalTimeSubstitutable = import (repoRoot + "/lib/util/eval-time-substitutable.nix");
  };

  sourceRoot = repoRoot + "/packages/nix-cargo-unit";
  manifest = lib.importTOML (sourceRoot + "/Cargo.toml");
  checkedCargoUnit = rust.buildPackage {
    pname = manifest.package.name;
    inherit (manifest.package) version;
    src = sourceRoot;
    policy = rust.policyPresets.pureBuild;
  };
  nixCargoUnit = checkedCargoUnit.passthru.unchecked;
in
  import (repoRoot + "/lib/rust/cargo-unit.nix") {
    inherit
      lib
      pkgs
      rust
      nixCargoUnit
      ;
  }
