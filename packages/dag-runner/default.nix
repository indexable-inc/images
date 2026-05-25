{
  ix,
  pkgs,
}:

ix.buildRustPackage pkgs {
  pname = "dag-runner";
  version = "0.1.0";

  src = ix.rustWorkspace.src;
  cargoLock.lockFile = ix.rustWorkspace.cargoLock;
  buildAndTestSubdir = "packages/dag-runner";
  cargoArgs = [
    "-p"
    "dag-runner"
  ];

  meta.mainProgram = "dag-runner";
}
