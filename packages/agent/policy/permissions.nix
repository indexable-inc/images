{
  lib,
  mcpServers ? { },
}:
let
  protectedMergeToolPatterns = [
    "Bash(gh pr merge*--admin*)"
    "Bash(gh pr merge*--force*)"
  ];

  supersededBuiltinTools =
    lib.optionals (mcpServers ? exa) [
      "WebSearch"
      "WebFetch"
    ]
    ++ lib.optional (mcpServers ? index) "Bash";

  supersededCodexTools = lib.optional (mcpServers ? index) "Bash";
in
{
  claude = {
    deniedToolPatterns = protectedMergeToolPatterns ++ supersededBuiltinTools;
  };

  codex = {
    deniedToolPatterns = supersededCodexTools;
    protectedMergeCommandPatterns = [
      "gh pr merge*--admin*"
      "gh pr merge*--force*"
    ];
  };
}
