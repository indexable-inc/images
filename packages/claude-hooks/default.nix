{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "claude-hooks";
  meta = {
    description = "Claude Code hook commands (session-digest, worktree-guard, prompt-priors) as one compiled binary";
    license = lib.licenses.mit;
    mainProgram = "claude-hooks";
  };
}
