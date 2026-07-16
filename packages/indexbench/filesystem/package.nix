{
  id = "indexbench-filesystem";
  packageSet = true;
  # The user-facing attr predates the Rust port and is documented as
  # `nix run .#bench-filesystem` (README.md); keep it stable.
  flake = {attrName = "bench-filesystem";};
  overlay = false;
  inRustWorkspace = true;
  passthruTests = true;
}
