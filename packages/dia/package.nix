{
  id = "dia";
  # macOS-only: Dia ships a single Apple-Silicon .app inside a .dmg and
  # requires macOS 14+ on M1+, so every target is gated to aarch64-darwin.
  # The linux flake/overlay/update paths then never see (or try to build) it.
  packageSet = {
    systems = ["aarch64-darwin"];
  };
  flake = {
    systems = ["aarch64-darwin"];
  };
  overlay = {
    systems = ["aarch64-darwin"];
  };
  updateScript = true;
}
