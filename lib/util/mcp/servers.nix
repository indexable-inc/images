# Single source of truth for the MCP servers the agent wrappers bake in. Define
# a server ONCE in the neutral shape below and render it to each tool's native
# config with `toClaudeJson` / `toCodexEntries`, so `index` (and any future
# shared server) is declared in one place instead of copied into the Claude Code
# and Codex wrappers in two different schemas.
#
# A neutral server definition is one of:
#   { transport = "stdio"; command = <str>; args ? [ <str> ]; env ? { <k> = <str>; }; envVars ? [ <str> ]; }
#   { transport = "http";  url = <str>; }
# and `servers` throughout is an attrset from server name to such a definition.
{ lib }:
let
  indexApiEnvVars = [
    "GH_TOKEN"
    "GITHUB_TOKEN"
    "IX_TOKEN"
    "LINEAR_API_KEY"
    "NOTION_API_KEY"
    "SLACK_TOKEN"
    "SLACK_USER_TOKEN"
  ];

  defaultServers =
    {
      indexCommand ? null,
    }:
    lib.optionalAttrs (indexCommand != null) {
      index = {
        transport = "stdio";
        command = indexCommand;
        args = [ "serve" ];
        envVars = indexApiEnvVars;
      };
    }
    // {
      exa = {
        transport = "http";
        url = "https://mcp.exa.ai/mcp";
      };
    };

  optionalServers =
    {
      # Path to the packaged `blender-mcp` binary (`lib.getExe` of the
      # `packages/blender-mcp` build). A command string rather than the package
      # itself because this registry is pure lib, out of `pkgs` scope.
      blenderMcp,
    }:
    {
      # Stdio MCP server that bridges to the BlenderMCP addon socket on
      # localhost:9876. Only useful on a host where a Blender GUI session has
      # the matching addon (the package's `passthru.addon`) loaded, which is
      # why it never enters `defaultServers`.
      blender = {
        transport = "stdio";
        command = blenderMcp;
        env = {
          # telemetry.py opt-out; the default phones home per tool call.
          DISABLE_TELEMETRY = "true";
        };
      };
    };
in
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
  inherit defaultServers;

  /**
    Opt-in servers that depend on machine-local state (a running GUI app, a
    local daemon) and so never enter the default set: baking them into every
    wrapper would hand fleet and CI agents a dead tool surface. Consumers merge
    what applies, e.g.
    `defaultServers { ... } // optionalServers { blenderMcp = lib.getExe blender-mcp; }`.
  */
  inherit optionalServers;

  /**
    Compatibility name for consumers pinned to the original MCP registry API.
    Use `defaultServers` in new code.
  */
  houseServers = defaultServers;
}
