# Single source of truth for the MCP servers the house wrappers bake in. Define
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
let
  toml = import ./toml.nix { inherit lib; };

  isStdio = def: (def.transport or "stdio") == "stdio";

  # One neutral def -> the object Claude Code's `mcpServers` expects (its schema
  # is `{ type = "stdio"; command; args; env; }` / `{ type = "http"; url; }`).
  claudeOne =
    def:
    if isStdio def then
      {
        type = "stdio";
        inherit (def) command;
      }
      // lib.optionalAttrs (def ? args) { inherit (def) args; }
      // lib.optionalAttrs (def ? env) { inherit (def) env; }
    else
      {
        type = "http";
        inherit (def) url;
      };

  # A TOML array literal of scalars, e.g. `[ "serve" ]`. `ix.toml.scalar` is
  # scalars-only by design (it throws on a list), so the one list leaf a server
  # carries (`args`) gets this local encoder rather than growing that helper.
  tomlArray = xs: "[ ${lib.concatStringsSep ", " (map toml.scalar xs)} ]";

  # One neutral def -> the `{ key; value; }` pairs Codex needs, where `key` is a
  # dotted path under `mcp_servers.<name>` and `value` is the rendered TOML
  # literal. These drop straight into the codex launch spec's `soft` list, which
  # config-launch turns into `-c <key>=<value>` flags (so a user's own
  # `mcp_servers.<name>` in config.toml still wins per the per-leaf presence
  # check). Codex keys stdio servers by `command`/`args`/`env` and HTTP servers
  # by `url`, mirroring `[mcp_servers.<name>]` tables in config.toml. House MCP
  # tools are trusted defaults, so approve server tools unless the user config
  # sets a stricter `default_tools_approval_mode` or per-tool override.
  codexEntriesOne =
    name: def:
    let
      prefix = "mcp_servers.${name}";
      envEntries = lib.mapAttrsToList (k: v: {
        key = "${prefix}.env.${k}";
        value = toml.scalar v;
      }) (def.env or { });
    in
    [
      {
        key = "${prefix}.default_tools_approval_mode";
        value = toml.scalar "approve";
      }
    ]
    ++ (
      if isStdio def then
        [
          {
            key = "${prefix}.command";
            value = toml.scalar def.command;
          }
        ]
        ++ lib.optional (def ? args) {
          key = "${prefix}.args";
          value = tomlArray def.args;
        }
        ++ envEntries
      else
        [
          {
            key = "${prefix}.url";
            value = toml.scalar def.url;
          }
        ]
    );
in
{
  /**
    The house MCP server set, defined once for every wrapper that bakes it.
    Returns the neutral definitions; each consumer renders them with
    `toClaudeJson` / `toCodexEntries`.

    Arguments:
    - `indexCommand`: path to the `ix-mcp` entrypoint, or `null` when the `mcp`
      sibling is out of scope (e.g. the overlay package set), in which case only
      the keyless `exa` server is returned.
  */
  houseServers =
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

  /**
    Render an attrset of neutral server definitions to the value Claude Code's
    `mcpServers` config key expects (the `{ <name> = { type = "stdio"; ... }; }`
    JSON the `claude-code` wrapper bakes into its `--mcp-config` file).

    Arguments:
    - `servers`: attrset from server name to a neutral definition.
  */
  toClaudeJson = servers: lib.mapAttrs (_: claudeOne) servers;

  /**
    Render an attrset of neutral server definitions to the value a Claude Code
    SUBAGENT's `mcpServers` frontmatter key expects, which differs from the
    `.mcp.json` object `toClaudeJson` produces: an ARRAY (`(string | object)[]`)
    whose entries are each a single-key `{ <name> = <inline config>; }` object
    (an inline server connected only while the subagent runs). Inline servers
    only; for a name reference to an already-configured server, append the bare
    string to the result.

    Arguments:
    - `servers`: attrset from server name to a neutral definition.
  */
  toAgentMcpServers = servers: lib.mapAttrsToList (name: def: { ${name} = claudeOne def; }) servers;

  /**
    Render an attrset of neutral server definitions to a flat list of
    `{ key; value; }` entries keyed by dotted `mcp_servers.<name>.*` paths with
    TOML-literal values, ready to splice into the codex launch spec's `soft`
    list (config-launch emits one `-c <key>=<value>` flag per entry).

    Arguments:
    - `servers`: attrset from server name to a neutral definition.
  */
  toCodexEntries = servers: lib.concatLists (lib.mapAttrsToList codexEntriesOne servers);
}
