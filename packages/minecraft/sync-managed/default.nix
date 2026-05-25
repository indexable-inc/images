{
  ix,
  pkgs,
}:

ix.buildRustPackage pkgs {
  pname = "minecraft-sync-managed";
  version = "0.1.0";

  src = ix.rustWorkspace.src;
  cargoLock.lockFile = ix.rustWorkspace.cargoLock;
  buildAndTestSubdir = "packages/minecraft/sync-managed";
  cargoArgs = [
    "-p"
    "minecraft-sync-managed"
  ];

  meta.mainProgram = "minecraft-sync-managed";
}
