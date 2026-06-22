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

  codexForcedSettings = lib.optionalAttrs (mcpServers ? index) {
    features = {
      shell_tool = false;
      unified_exec = false;
    };
  };
in
{
  claude = {
    deniedToolPatterns = protectedMergeToolPatterns ++ supersededBuiltinTools;
  };

  codex = {
    forcedSettings = codexForcedSettings;
    protectedMergeCommandPatterns = [
      "gh pr merge*--admin*"
      "gh pr merge*--force*"
    ];
  };
}
