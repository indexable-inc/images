{
  ix,
  pkgs,
}:

ix.buildRustPackage pkgs {
  pname = "ix-dev-diagnose";
  version = "0.1.0";

  src = ix.rustWorkspace.src;
  cargoLock.lockFile = ix.rustWorkspace.cargoLock;
  buildAndTestSubdir = "packages/ix-dev-diagnose";
  cargoArgs = [
    "-p"
    "ix-dev-diagnose"
  ];

  meta.mainProgram = "ix-dev-diagnose";
}
