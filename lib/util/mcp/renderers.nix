# Render neutral MCP server definitions to each agent wrapper's native shape.
{
  lib,
  toml,
}: let
  isStdio = def: (def.transport or "stdio") == "stdio";

  # One neutral def -> the object Claude Code's `mcpServers` expects (its schema
  # is `{ type = "stdio"; command; args; env; }` / `{ type = "http"; url; }`).
  claudeOne = def: let
    forwardedEnv = lib.genAttrs (def.envVars or []) (name: "\${${name}:-}");
    env = forwardedEnv // (def.env or {});
  in
    if isStdio def
    then
      {
        type = "stdio";
        inherit (def) command;
      }
      // lib.optionalAttrs (def ? args) {inherit (def) args;}
      // lib.optionalAttrs (env != {}) {inherit env;}
    else {
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
  # by `url`, mirroring `[mcp_servers.<name>]` tables in config.toml. Default MCP
  # tools are trusted defaults, so approve server tools unless the user config
  # sets a stricter `default_tools_approval_mode` or per-tool override.
  codexEntriesOne = name: def: let
    prefix = "mcp_servers.${name}";
    envEntries = lib.mapAttrsToList (k: v: {
      key = "${prefix}.env.${k}";
      value = toml.scalar v;
    }) (def.env or {});
    envVarEntries = lib.optional (def ? envVars && def.envVars != []) {
      key = "${prefix}.env_vars";
      value = tomlArray def.envVars;
    };
  in
    [
      {
        key = "${prefix}.default_tools_approval_mode";
        value = toml.scalar "approve";
      }
    ]
    ++ (
      if isStdio def
      then
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
        ++ envVarEntries
        ++ envEntries
      else [
        {
          key = "${prefix}.url";
          value = toml.scalar def.url;
        }
      ]
    );
in {
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
  toAgentMcpServers = servers: lib.mapAttrsToList (name: def: {${name} = claudeOne def;}) servers;

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
