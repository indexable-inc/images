{
  ix,
  pkgs,
}: let
  inherit (pkgs) lib;
  fs = lib.fileset;
  root = ./fixtures/cargo-unit-hello;
  src = fs.toSource {
    inherit root;
    fileset = fs.unions [
      (root + "/benches")
      (root + "/build.rs")
      (root + "/Cargo.lock")
      (root + "/Cargo.toml")
      (root + "/src")
    ];
  };
  # The renderer emits Nix code with intentional recursive unit references.
  # Keeping machine output extensionless lets lint review source expressions
  # without treating the generated catalog as hand-written Nix.
  unitCatalog = root + "/unit-catalog";
  workspaceArgs = {
    inherit src;
    workspaceRoot = root;
    cargoTargetNames = [
      "build"
      "test"
      "bench"
    ];
    packageTestInputs.cargo-unit-hello = [pkgs.hello];
    packageTestEnv.cargo-unit-hello.CARGO_UNIT_FIXTURE_ENV = "ok";
    # The build script re-exposes this as rustc-env; the runtime test checks
    # the compiled value rather than merely observing the builder environment.
    packageBuildEnv.cargo-unit-hello.CARGO_UNIT_BUILD_ENV = "build-ok";
    packageTestEnv.cargo-unit-hello.CARGO_UNIT_BUILD_ENV_EXPECTED = "build-ok";
    cargoTargets = [
      ["--workspace"]
      [
        "--workspace"
        "--tests"
      ]
      [
        "--workspace"
        "--benches"
      ]
    ];
  };
  workspace = ix.cargoUnit.buildWorkspace (
    workspaceArgs // {inherit unitCatalog;}
  );
in {
  inherit
    root
    src
    unitCatalog
    workspace
    workspaceArgs
    ;
}
