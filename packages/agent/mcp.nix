# Default MCP server set shared by agent CLI wrappers.
{
  lib,
  ix,
  repoPackages ? {},
}: {
  # Rendered from the shared `ix.mcp` registry with the kernel pointed at the
  # Elixir `mcp-ex` sibling when it is in scope. Each wrapper adapts this to
  # its own config shape.
  defaultServers = ix.mcp.defaultServers {
    indexCommand =
      if repoPackages ? mcp-ex
      then lib.getExe repoPackages.mcp-ex
      else null;
    indexArgs = [];
  };
}
