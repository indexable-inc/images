{
  ix,
  pkgs,
}:

ix.buildRustPackage pkgs {
  pname = "minecraft-nbt";
  version = "0.1.0";

  src = ix.rustWorkspace.src;
  cargoLock.lockFile = ix.rustWorkspace.cargoLock;
  buildAndTestSubdir = "packages/minecraft/nbt";
  cargoArgs = [
    "-p"
    "minecraft-nbt"
  ];

  meta.mainProgram = "minecraft-nbt";
}
