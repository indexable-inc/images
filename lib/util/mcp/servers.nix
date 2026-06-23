# Single source of truth for the MCP servers the agent wrappers bake in. Define
# a server ONCE in the neutral shape below and render it to each tool's native
# config with `toClaudeJson` / `toCodexEntries`, so `index` (and any future
# shared server) is declared in one place instead of copied into the Claude Code
# and Codex wrappers in two different schemas.
#
# A neutral server definition is one of:
#   { transport = "stdio"; command = <str>; args ? [ <str> ]; env ? { <k> = <str>; }; }
#   { transport = "http";  url = <str>; }
# and `servers` throughout is an attrset from server name to such a definition.
{ lib }:
{
  /**
    The default MCP server set, defined once for every wrapper that bakes it.
    Returns the neutral definitions; each consumer renders them with
    `toClaudeJson` / `toCodexEntries`.

    Arguments:
    - `indexCommand`: path to the `ix-mcp` entrypoint, or `null` when the `mcp`
      sibling is out of scope (e.g. the overlay package set), in which case only
      the keyless `exa` server is returned.
  */
  defaultServers =
    {
      indexCommand ? null,
    }:
    lib.optionalAttrs (indexCommand != null) {
      index = {
        transport = "stdio";
        command = indexCommand;
        args = [ "serve" ];
      };
    }
    // {
      exa = {
        transport = "http";
        url = "https://mcp.exa.ai/mcp";
      };
    };
}
