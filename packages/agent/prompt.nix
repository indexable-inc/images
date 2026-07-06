# House system prompt helpers shared by agent CLI wrappers.
{
  lib,
  # Rule names dropped from the baked prompt; forwarded to ./system-prompt.nix.
  omitRules ? [],
}: let
  providerNames = {
    claude = "Claude Code";
    codex = "Codex";
    cursor = "Cursor";
  };
  # Runtime token each provider renders as, consumed by ./system-prompt.nix to
  # filter runtime-scoped sections (a section's `runtimes` list holds these).
  providerRuntimes = {
    claude = "claude-code";
    codex = "codex";
    cursor = "cursor";
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
    cursor = ''
      You are Cursor. When naming the coding-agent runtime or disclosing AI
      authorship in outward-facing messages, say Cursor.
    '';
  };
  systemPromptFor = provider:
    lib.concatStringsSep "\n\n" [
      (import ./system-prompt.nix {
        inherit lib omitRules;
        agentName = providerNames.${provider};
        runtime = providerRuntimes.${provider};
      })
      extraSystemPrompts.${provider}
    ];
in {
  inherit extraSystemPrompts systemPromptFor;

  # The house system prompt a wrapper bakes for Claude Code by default. One
  # paragraph per list element; see ./system-prompt.nix for the authored text
  # and how claude-code bakes it (`systemPrompt`, replacing the stock prompt).
  systemPrompt = systemPromptFor "claude";
}
