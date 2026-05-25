{
  ix,
  pkgs,
}:

ix.buildRustPackage pkgs {
  pname = "oci-image-builder";
  version = "0.1.0";

  src = ix.rustWorkspace.src;
  cargoLock.lockFile = ix.rustWorkspace.cargoLock;
  buildAndTestSubdir = "packages/oci-image-builder";
  cargoArgs = [
    "-p"
    "oci-image-builder"
  ];

  meta.mainProgram = "oci-image-builder";
}
