{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "nix-clj-unit";
  meta = {
    description = "Render a Clojure source tree as a per-namespace dependency graph for Nix unit builds";
    license = lib.licenses.mit;
    mainProgram = "nix-clj-unit";
  };
}
