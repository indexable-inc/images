{ ix, pkgs }:

ix.buildRustPackage pkgs {
  pname = "dag-runner";
  version = "0.1.0";
  srcRoot = ./.;
}
