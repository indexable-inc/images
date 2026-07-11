# House system prompt helpers shared by agent CLI wrappers.
{
  lib,
  # Rule names dropped from the baked prompt; forwarded to ./system-prompt.nix.
  omitRules ? [ ],
}:
let
  providerNames = {
    claude = "Claude Code";
    codex = "Codex";
  };
  extraSystemPrompts = {
    claude = ''
      You are Claude Code. When naming the coding-agent runtime or disclosing AI
      authorship in outward-facing messages, say Claude Code.
    '';
    codex = ''
      You are Codex. When naming the coding-agent runtime or disclosing AI
      authorship in outward-facing messages, say Codex.
    '';
  };
  # Rules about Claude Code features other runtimes do not have; dropped from
  # every non-claude prompt so e.g. Codex is never told about agent teams
  # (CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS) it cannot spawn.
  claudeOnlyRules = [ "agentTeams" ];
  systemPromptFor =
    provider:
    lib.concatStringsSep "\n\n" [
      (import ./system-prompt.nix {
        inherit lib;
        omitRules = omitRules ++ lib.optionals (provider != "claude") claudeOnlyRules;
        agentName = providerNames.${provider};
      })
      extraSystemPrompts.${provider}
    ];
in
{
  inherit extraSystemPrompts systemPromptFor;

  # The house system prompt a wrapper bakes for Claude Code by default. One
  # paragraph per list element; see ./system-prompt.nix for the authored text
  # and how claude-code bakes it (`systemPrompt`, replacing the stock prompt).
  systemPrompt = systemPromptFor "claude";
}
