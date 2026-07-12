# House prompt shared by agent CLI wrappers and context files.
#
# The rules live in ./rules.nix as pure data; this file owns validation, tag
# filtering, and rendering. A render target is a tag set built from two axes:
#
#  - kind: "system" when the rendered text IS the agent's whole system prompt
#    (claude-code's `--system-prompt-file`, codex's `model_instructions_file`),
#    so identity and harness basics must be established here; "context" when
#    the text rides on top of the runtime's stock prompt as an instructions
#    file (~/.claude/CLAUDE.md, ~/.codex/AGENTS.md), where restating those
#    basics would only spend tokens contradicting the stock prompt.
#  - runtime: "claude-code", "codex", or "cursor", for rules that name
#    runtime-specific tooling.
#
# A rule applies when every tag it declares is in the target set: untagged
# rules render everywhere, `system`-tagged rules never leak into context
# files, and runtime-tagged rules stay with their runtime.
#
# Render and reread the claude-code system prompt (the default) with:
#   nix eval --raw --impure --expr \
#     '(import ./packages/agent/prompt { lib = (import <nixpkgs> {}).lib; }).systemPrompt'
# Swap `.systemPrompt` for `.contextFor "codex"` and friends for the variants.
{
  lib,
  # Rule names dropped from every render, e.g.
  # `claude-code.override { omitRules = [ "reportToPlaybook" ]; }`.
  omitRules ? [],
}: let
  providers = {
    claude = {
      agentName = "Claude Code";
      tag = "claude-code";
    };
    codex = {
      agentName = "Codex";
      tag = "codex";
    };
    cursor = {
      agentName = "Cursor";
      tag = "cursor";
    };
  };
  kinds = [
    "system"
    "context"
  ];
  knownTags = kinds ++ map (provider: provider.tag) (builtins.attrValues providers);

  normalizeRule = rule: let
    names = builtins.attrNames rule;
    name = builtins.head names;
    value = builtins.getAttr name rule;
    valueNames = builtins.attrNames value;
    # attrNames is lexicographically sorted, so the two legal shapes are:
    #   ["reason" "text"]  (renders everywhere) and
    #   ["reason" "tags" "text"]  (narrowed to targets carrying every tag).
  in
    assert lib.assertMsg (
      builtins.length names == 1
    ) "prompt/rules.nix: each rule entry must have exactly one attribute";
    assert lib.assertMsg (
      valueNames
      == ["reason" "text"]
      || valueNames == ["reason" "tags" "text"]
    ) "prompt/rules.nix: rule `${name}` must have `reason` and `text` (and an optional `tags` list)"; {
      inherit name;
      inherit (value) text reason;
      tags = value.tags or [];
    };

  rulesFor = agentName: map normalizeRule (import ./rules.nix {inherit agentName;});

  # Names and tags do not vary with agentName, so validate one instantiation.
  canonicalRules = rulesFor providers.claude.agentName;
  ruleNames = map (rule: rule.name) canonicalRules;
  # Duplicate names would make omitRules drop several rules under one key.
  duplicateNames = builtins.filter (
    name: builtins.length (builtins.filter (other: other == name) ruleNames) > 1
  ) (lib.unique ruleNames);
  unknownOmits = builtins.filter (name: !(builtins.elem name ruleNames)) omitRules;
  unknownTags = builtins.filter (tag: !(builtins.elem tag knownTags)) (
    lib.unique (lib.concatMap (rule: rule.tags) canonicalRules)
  );

  render = {
    provider,
    kind,
  }: let
    providerConfig =
      providers.${provider}
        or (throw "prompt: unknown provider `${provider}`; known: ${lib.concatStringsSep ", " (builtins.attrNames providers)}");
    targetTags = [kind providerConfig.tag];
    applies = rule: builtins.all (tag: builtins.elem tag targetTags) rule.tags;
    kept = builtins.filter (
      rule: !(builtins.elem rule.name omitRules) && applies rule
    ) (rulesFor providerConfig.agentName);
  in
    assert lib.assertMsg (
      builtins.elem kind kinds
    ) "prompt: unknown kind `${kind}`; known: ${lib.concatStringsSep ", " kinds}";
    assert lib.assertMsg (
      duplicateNames == []
    ) "prompt/rules.nix: duplicate rule names: ${lib.concatStringsSep ", " duplicateNames}";
    assert lib.assertMsg (
      unknownOmits == []
    ) "prompt: omitRules names not found in rules: ${lib.concatStringsSep ", " unknownOmits}";
    assert lib.assertMsg (
      unknownTags == []
    ) "prompt/rules.nix: unknown tags: ${lib.concatStringsSep ", " unknownTags}";
      lib.concatStringsSep "\n\n" (map (rule: rule.text) kept);
in {
  inherit render;

  # The full system prompt a wrapper bakes when it replaces the stock prompt.
  systemPromptFor = provider:
    render {
      inherit provider;
      kind = "system";
    };

  # The instructions-file render (global CLAUDE.md / AGENTS.md): the same house
  # rules minus the `system`-tagged basics the stock prompt already owns.
  contextFor = provider:
    render {
      inherit provider;
      kind = "context";
    };

  # The house system prompt claude-code bakes by default; see ./rules.nix for
  # the authored text.
  systemPrompt = render {
    provider = "claude";
    kind = "system";
  };
}
