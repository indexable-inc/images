{
  ix,
  pkgs,
}:

ix.buildRustPackage pkgs {
  pname = "nix-cargo-unit";
  version = "0.1.0";

  src = ix.rustWorkspace.src;
  cargoLock.lockFile = ix.rustWorkspace.cargoLock;
  buildAndTestSubdir = "packages/nix-cargo-unit";
  cargoArgs = [
    "-p"
    "nix-cargo-unit"
  ];

  meta.mainProgram = "nix-cargo-unit";
}
