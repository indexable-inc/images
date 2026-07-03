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

  # Both Blender bridges share one shape: a stdio server the client spawns
  # plus an addon socket inside a Blender GUI session, so each is emitted only
  # when the consumer passes its binary. Command strings rather than packages
  # because this registry is pure lib, out of `pkgs` scope.
  optionalServers =
    {
      # `lib.getExe` of the `packages/blender-mcp` build (community bridge,
      # ahujasid): broad automation surface (objects, materials, Poly Haven,
      # code execution). Its addon owns localhost:9876.
      blenderMcp ? null,
      # `lib.getExe` of the `packages/blender-lab-mcp` build (official Blender
      # Lab): docs/analysis surface (blendfile summaries, API + manual lookup,
      # screenshots, code execution).
      blenderLabMcp ? null,
    }:
    lib.optionalAttrs (blenderMcp != null) {
      blender = {
        transport = "stdio";
        command = blenderMcp;
        env = {
          # telemetry.py opt-out; the default phones home per tool call.
          DISABLE_TELEMETRY = "true";
        };
      };
    }
    // lib.optionalAttrs (blenderLabMcp != null) {
      blender-lab = {
        transport = "stdio";
        command = blenderLabMcp;
        env = {
          # One up from the community addon's 9876 so both bridges can run in
          # the same Blender. This value is the source of truth for the port:
          # consumers configure the Lab addon's port preference FROM this env
          # (read it back off this definition) rather than restating 9877.
          BLENDER_MCP_PORT = "9877";
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
    what applies (packages come from the flake package set / `packageSetFor`,
    not the overlay), e.g.
    `defaultServers { ... } // optionalServers { blenderMcp = lib.getExe repoPackages.blender-mcp; }`.

    Caution: both Blender servers expose arbitrary-code-execution tools, and
    `toCodexEntries` stamps `default_tools_approval_mode = "approve"` on every
    rendered server. A consumer wiring these into an agent accepts
    auto-approved code execution against its local Blender; keep that opt-in
    per machine, never fleet policy.
  */
  inherit optionalServers;

  /**
    Compatibility name for consumers pinned to the original MCP registry API.
    Use `defaultServers` in new code.
  */
  houseServers = defaultServers;
}
