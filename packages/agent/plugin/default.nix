# The index plugin: every repo skill bundled in the Claude Code plugin format
# (`.claude-plugin/plugin.json` + `skills/`), the one delivery unit both agent
# CLIs consume. Claude Code loads the directory directly via `--plugin-dir`
# (the wrapper's `pluginDirs`, or `programs.claude-code.housePlugin`); Codex
# discovers the identical format through a local marketplace declared in
# config (`passthru.marketplace`; Codex resolves `.claude-plugin/plugin.json`
# and `.claude-plugin/marketplace.json` as compatible manifest locations).
# Plugin skills invoke namespaced: `/index:<skill>`.
#
# Deliberately skills-only:
#  - hooks: both wrappers already deliver the shared hook policy (Claude
#    through the settings flagSettings layer, Codex through
#    ~/.codex/hooks.json); plugin-carried hooks would double-fire it, the two
#    runtimes want different hook schemas, and Codex trust-gates plugin hooks
#    per content hash (a re-approval on every store-path bump).
#  - MCP: a plugin-scoped MCP server is namespaced by the plugin, which would
#    break the `mcp__index__*` tool names the permission denies, subagents,
#    and docs reference; the wrappers keep baking the shared registry
#    (`--mcp-config` / soft `-c mcp_servers...`).
#  - agents: a plugin namespaces `subagent_type`, breaking bare agent
#    references (see lib/claude-plugin.nix); agents keep their `.claude/agents`
#    delivery.
{ix}: let
  inherit (ix) pkgs;
  plugin = ix.claudePlugin.mkPlugin {
    inherit pkgs;
    name = "index";
    description = "index house skills for coding agents";
  };
  marketplace = ix.claudePlugin.mkMarketplace {
    inherit pkgs;
    name = "index";
    plugins.index = plugin;
  };
in
  plugin.overrideAttrs (previousAttrs: {
    passthru = (previousAttrs.passthru or {}) // {inherit marketplace;};
  })
