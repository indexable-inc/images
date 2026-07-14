# `whence <path>`: deployed config file -> defining nix source line (#2416).
# The lookup logic lives in the crate (src/main.rs); this wrapper only selects
# the workspace binary. No pinned nix in the closure: the fallback
# `nix-store -q --deriver` must speak the host daemon's
# protocol/experimental-feature set, so it uses the ambient nix, same as
# push-cache and the updaters.
{ix, ...}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "whence";
  meta = {
    description = "Deployed config file -> defining nix source line, from the generation's provenance manifest";
    mainProgram = "whence";
  };
}
