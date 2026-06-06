# Registry metadata only. Symphony's default.nix is not callPackage-shaped:
# it needs a rust-overlay pkgs and the resolved index MCP derivation, so
# `symphonyFor` in lib/default.nix owns the wiring and lib/overlay.nix
# exposes `symphony-room-server` from it.
{
  id = "symphony";
}
