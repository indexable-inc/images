{
  lib,
  mcpServers ? { },
}:
let
  forceMergeDenyTools = [
    "Bash(gh pr merge*--admin*)"
    "Bash(gh pr merge*--force*)"
  ];

  replacementToolDenyTools =
    lib.optionals (mcpServers ? exa) [
      "WebSearch"
      "WebFetch"
    ]
    ++ lib.optional (mcpServers ? index) "Bash";
in
{
  claude = {
    deny = forceMergeDenyTools ++ replacementToolDenyTools;
  };

  codex = {
    denyCommands = [
      "gh pr merge*--admin*"
      "gh pr merge*--force*"
    ];
  };
}
