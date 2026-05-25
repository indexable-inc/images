{ ix, pkgs }:

ix.buildRustPackage pkgs {
  pname = "minecraft-sync-managed";
  version = "0.1.0";
  srcRoot = ./.;
}
