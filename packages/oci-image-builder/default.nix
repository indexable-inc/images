{ ix, pkgs }:

ix.buildRustPackage pkgs {
  pname = "oci-image-builder";
  version = "0.1.0";
  srcRoot = ./.;
}
