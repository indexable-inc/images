{
  lib,
  mcpServers ? { },
}:
let
  protectedMergeToolPatterns = [
    "Bash(gh pr merge*--admin*)"
    "Bash(gh pr merge*--force*)"
  ];

  supersededBuiltinTools = [
    "WebSearch"
    "WebFetch"
  ]
  ++ lib.optional (mcpServers ? index) "Bash";

  codexForcedSettings = {
    features = {
      browser_use = false;
      browser_use_external = false;
      computer_use = false;
      image_generation = false;
      in_app_browser = false;
      shell_tool = false;
      standalone_web_search = false;
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
