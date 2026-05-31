{
  # The Rust crate keeps the `agents-md` name (it renders AGENTS.md/CLAUDE.md);
  # the flake output is `agent-context` to match the lib surface that now owns
  # both always-on instructions and on-demand skills.
  id = "agents-md";
  packageSet = true;
  flake = {
    attrName = "agent-context";
  };
  inRustWorkspace = true;
  passthruTests = true;
}
