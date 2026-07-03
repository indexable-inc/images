{ix, ...}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "minecraft-nbt";
  meta.mainProgram = "minecraft-nbt";
}
