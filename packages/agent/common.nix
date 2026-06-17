# Defaults shared by the agent CLI wrappers under ./ (claude-code, codex). The
# house system prompt and the house MCP server set are declared here ONCE so
# both wrappers draw them from a single source instead of re-deriving the same
# `repoPackages.mcp` wiring in each default.nix.
#
# Imported from a wrapper's default.nix as `import ../common.nix { inherit lib
# ix repoPackages; }`. `repoPackages` is the flake package-set fix-point (the
# overlay passes `{ }`), so the index kernel is wired in only where the `mcp`
# sibling is in scope, exactly as each wrapper did inline before.
{
  lib,
  ix,
  repoPackages ? { },
}:
{
  # The house system prompt a wrapper bakes for its agent. One paragraph per
  # list element; see ./system-prompt.nix for the authored text and how
  # claude-code bakes it (`systemPrompt`, which REPLACES the stock prompt).
  systemPrompt = import ./system-prompt.nix { inherit lib; };

  # The house MCP servers (the `index` kernel plus `exa` web search), rendered
  # from the shared `ix.mcp` registry with the kernel pointed at the `mcp`
  # sibling when it is in scope. Each wrapper adapts this to its own config
  # shape (`ix.mcp.toClaudeJson` / `ix.mcp.toCodexEntries`).
  houseServers = ix.mcp.houseServers {
    indexCommand = if repoPackages ? mcp then lib.getExe repoPackages.mcp else null;
  };
}
