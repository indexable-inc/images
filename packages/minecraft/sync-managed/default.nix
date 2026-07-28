{ix, ...}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "minecraft-sync-managed";
  meta.mainProgram = "minecraft-sync-managed";
}
