# House MCP server set shared by agent CLI wrappers.
{
  lib,
  ix,
  repoPackages ? { },
}:
{
  # Rendered from the shared `ix.mcp` registry with the kernel pointed at the
  # `mcp` sibling when it is in scope. Each wrapper adapts this to its own
  # config shape (`ix.mcp.toClaudeJson` / `ix.mcp.toCodexEntries`).
  houseServers = ix.mcp.houseServers {
    indexCommand = if repoPackages ? mcp then lib.getExe repoPackages.mcp else null;
  };
}
