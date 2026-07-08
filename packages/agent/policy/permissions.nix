# Shared agent permission policy: one agent-neutral fact per row, rendered per
# agent CLI. The tool vocabularies differ (Claude Code denies tool names via
# settings `permissions.deny`; codex disables `features.*` leaves via the
# forced `-c` layer; cursor-agent denies `Shell(...)` patterns via
# `cli-config.json`), so each capability row carries both handles and the
# renderers at the bottom fold in the rows a wrapper's baked MCP servers make
# redundant.
#
# Claude runtime semantics, verified empirically on the pinned CLI (2.1.197,
# headless `claude -p --settings` probes): `permissions.deny` is a hard block
# even under the wrapper's default `--dangerously-skip-permissions` posture
# (bypass skips prompts, not deny rules). A SUBAGENT whose definition declares
# an explicit `tools:` allowlist re-grants only SOME settings-denied tools:
# in a bg-dispatched session a code-reviewer spawn (declares Read/Bash/Glob/
# Grep) got Glob and Grep but neither Bash nor Read (#2077, #2153; probed
# 2026-07-07 on 2.1.197). Subagents that need shell must bring their own
# index kernel via `mcpServers` (see subagents.nix `executor` and
# `index-action-runner`); do not rely on a declared Bash surviving the deny.
{
  lib,
  # True when the wrapper bakes the `index` MCP server (the ix kernel,
  # packages/mcp). The kernel owns shell, file IO, and code search
  # (python_exec/nu, read, grep/find), so the stock native tools this table
  # maps are disabled wherever it is present. A wrapper without the kernel
  # (the overlay package set) keeps the kernel-superseded tools: denying them
  # there would leave the agent with no hands. (The exa-gated web pair below
  # is a separate gate and is still denied in the overlay build, since exa is
  # baked unconditionally by the default server set.)
  indexKernelBaked ? false,
  # True when the wrapper bakes the `exa` MCP server, which supersedes the
  # stock web search/fetch surface.
  exaSearchBaked ? false,
}: let
  # One list of protected-merge command globs; the Claude render wraps them in
  # Bash(...) deny patterns, the codex render ships them verbatim for hook use.
  protectedMergeCommandPatterns = [
    "gh pr merge*--admin*"
    "gh pr merge*--force*"
  ];

  # Native capabilities the index kernel supersedes. `claudeTools` are Claude
  # Code tool names for `permissions.deny`; `codexFeatures` are codex
  # `features.*` leaves for the forced `-c` layer. The file IO and search rows
  # carry no codex handle: codex reads, writes, and searches through its shell
  # (covered by the `shell` row), and its `apply_patch` tool is enabled
  # per-model upstream with no config toggle to reach it.
  kernelSuperseded = {
    shell = {
      claudeTools = ["Bash"];
      codexFeatures = {
        shell_tool = false;
        unified_exec = false;
      };
    };
    fileRead = {
      claudeTools = ["Read"];
      codexFeatures = {};
    };
    fileWrite = {
      claudeTools = ["Write" "NotebookEdit"];
      codexFeatures = {};
    };
    fileEdit = {
      claudeTools = ["Edit"];
      codexFeatures = {};
    };
    fileSearch = {
      claudeTools = ["Glob" "Grep"];
      codexFeatures = {};
    };
  };
  kernelClaudeTools = lib.concatMap (row: row.claudeTools) (lib.attrValues kernelSuperseded);
  kernelCodexFeatures = lib.mergeAttrsList (
    map (row: row.codexFeatures) (lib.attrValues kernelSuperseded)
  );

  # Web search/fetch superseded by the exa server.
  exaSuperseded = {
    claudeTools = [
      "WebSearch"
      "WebFetch"
    ];
    codexFeatures.standalone_web_search = false;
  };

  # Unconditional house policy: no browser/computer/media surfaces in baked
  # wrappers, independent of which MCP servers ride along.
  codexHouseFeatures = {
    browser_use = false;
    browser_use_external = false;
    computer_use = false;
    image_generation = false;
    in_app_browser = false;
  };

  claudeHouseDeniedTools = [
    "Monitor"
    "CronCreate"
    "CronDelete"
    "CronList"
  ];
in {
  claude = {
    deniedToolPatterns =
      map (pattern: "Bash(${pattern})") protectedMergeCommandPatterns
      ++ lib.optionals exaSearchBaked exaSuperseded.claudeTools
      ++ lib.optionals indexKernelBaked (kernelClaudeTools ++ claudeHouseDeniedTools);
  };

  codex = {
    forcedSettings.features =
      codexHouseFeatures
      // lib.optionalAttrs exaSearchBaked exaSuperseded.codexFeatures
      // lib.optionalAttrs indexKernelBaked kernelCodexFeatures;
    inherit protectedMergeCommandPatterns;
  };

  # cursor-agent's `cli-config.json` permission vocabulary only verifiably
  # covers shell commands (`Shell(<glob>)` deny entries), so only the
  # protected-merge row renders here; the kernel/exa gates have no cursor
  # handle yet. Delivery is the consumer's config management (see the
  # cursor-cli wrapper's passthru), since the CLI reads permissions from
  # config, not flags.
  cursor = {
    deniedShellPatterns = map (pattern: "Shell(${pattern})") protectedMergeCommandPatterns;
  };
}
