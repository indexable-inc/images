{ ix, pkgs }:

ix.buildRustPackage pkgs {
  pname = "minecraft-nbt";
  version = "0.1.0";
  srcRoot = ./.;
}
