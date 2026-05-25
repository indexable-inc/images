{ ix, pkgs }:

ix.buildRustPackage pkgs {
  pname = "nix-cargo-unit";
  version = "0.1.0";
  srcRoot = ./.;
}
