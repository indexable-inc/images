{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "claude-stories";
  meta = {
    description = "Instagram-style status-line 'stories' for Claude Code, served peer-to-peer over a Tailscale tailnet";
    license = lib.licenses.mit;
    mainProgram = "claude-stories";
  };
}
