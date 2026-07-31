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
# `cursor-search`); do not rely on a declared Bash surviving the deny.
# The inline server must carry a name the parent session does not already
# bind: a same-named inline `index` is silently discarded and the parent's
# connection shared instead (#2382), which is no isolation at all.
{
  lib,
  # True when the wrapper bakes the `index` MCP server. The kernel owns stateful
  # data and fleet work. Native file tools are disabled wherever it is present,
  # while the native shell remains available as the direct command path.
  indexKernelBaked ? false,
  # True when the wrapper bakes the `exa` MCP server, which supersedes the
  # stock web search/fetch surface.
  exaSearchBaked ? false,
  # False drops the protected-merge command denies from every render, for
  # consumers whose operator deliberately permits merge-protection bypasses
  # (pairs with omitting the `forceMerge` prompt rule).
  protectedMergeGuard ? true,
  # True also denies the native shell, making the kernel the only command
  # path. Deliberately NOT implied by `indexKernelBaked`: see the
  # `shellSuperseded` row below for what that costs.
  kernelSupersedesShell ? false,
}: let
  # One list of protected-merge command globs; the Claude render wraps them in
  # Bash(...) deny patterns, the codex render ships them verbatim for hook use.
  protectedMergeCommandPatterns = lib.optionals protectedMergeGuard [
    "gh pr merge*--admin*"
    "gh pr merge*--force*"
  ];

  # Native capabilities the index kernel supersedes. `claudeTools` are Claude
  # Code tool names for `permissions.deny`; `codexFeatures` are codex
  # `features.*` leaves for the forced `-c` layer. The file IO and search rows
  # carry no codex handle: codex reads, writes, and searches through its shell
  # and its `apply_patch` tool is enabled per-model upstream with no config
  # toggle to reach it.
  kernelSuperseded = {
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
  # The native shell. Kept OUT of `kernelSuperseded` above on purpose: a baked
  # kernel denies the file tools unconditionally, but the shell is also the
  # kernel-outage path (index#4080) -- an agent whose kernel dies with no Bash
  # cannot run a command at all, and the dev-base gate in tests/default.nix
  # asserts that default. An operator opting in trades that fallback, plus
  # every per-command `Bash(...)` deny pattern (the deny vocabulary cannot see
  # inside a kernel cell), for one audited command path: `IxMcp.Cmd` pins the
  # launch cwd against a sibling's `File.cd!` (#3902), spawns with stdin closed
  # so a pathless `rg` cannot hang (#3867), and lands every run in the action
  # log.
  shellSuperseded = {
    claudeTools = ["Bash" "BashOutput" "KillShell"];
    # No codex handle: codex reads, writes and searches THROUGH its shell and
    # its apply_patch tool has no config toggle, so denying the shell would
    # leave that agent unable to work at all rather than kernel-first.
    codexFeatures = {};
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
    "CronCreate"
    "CronDelete"
    "CronList"
  ];

  # Claude-bundled skills that inject Anthropic's own style guidance with no
  # real benefit to this harness. `artifact-design` vanishes from the listing
  # once the Artifact tool itself is denied in the wrapper's tool table, and
  # this scoped deny hard-blocks any blind invocation (verified 2026-07,
  # index#3607). It HOLDS under `--dangerously-skip-permissions` (probed
  # 2026-07), like every deny; bypass skips prompts, not deny rules.
  # Unconditional: unlike the kernel-superseded rows it has no non-kernel
  # fallback role. The sibling `dataviz` skill needs no deny: the wrapper
  # removes it outright via `skillOverrides` in claude-code/default.nix
  # (index#3659), which both delists it and refuses invocation.
  claudeBundledSkillDenies = [
    "Skill(artifact-design)"
  ];in
  # Denying the shell without the kernel that supersedes it leaves the agent no
  # command path at all, so refuse the combination rather than render an agent
  # that cannot work.
  assert kernelSupersedesShell -> indexKernelBaked; {
  claude = {
    deniedToolPatterns =
      map (pattern: "Bash(${pattern})") protectedMergeCommandPatterns
      ++ claudeBundledSkillDenies
      ++ lib.optionals exaSearchBaked exaSuperseded.claudeTools
      ++ lib.optionals indexKernelBaked (kernelClaudeTools ++ claudeHouseDeniedTools)
      ++ lib.optionals kernelSupersedesShell shellSuperseded.claudeTools;
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
