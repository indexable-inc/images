{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "claude-statusline";
  meta = {
    description = "House Claude Code statusline: identity mark, context-window bar, model, effort level, and CLI version with an update marker";
    license = lib.licenses.mit;
    mainProgram = "claude-statusline";
  };
}
