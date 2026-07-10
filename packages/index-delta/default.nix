{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "index-delta";
  meta = {
    description = "Mutable files over Nix: seed declared files as plain writable files, track drift as logical format-aware diffs (json/toml/yaml/plist/keyvalue), and queue base-vs-drift conflicts for model-driven resolution instead of auto-merging";
    license = lib.licenses.mit;
    mainProgram = "index-delta";
  };
}
