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
#     '(import ./packages/agent-prompt { lib = (import <nixpkgs> {}).lib; }).systemPrompt'
# Swap `.systemPrompt` for `.contextFor "codex"` and friends for the variants.
# The copy actually baked into the wrapper (after any `omitRules`) is
# `nix eval --raw .#claude-code.systemPrompt`.
{
  lib,
  # Rule names dropped from every render, e.g.
  # `claude-code.override { omitRules = [ "reportToPlaybook" ]; }`.
  omitRules ? [],
  # Topic names dropped from every render: a rule declaring ANY listed topic
  # is omitted, e.g. `omitTopics = [ "writing" ]` sheds the whole house
  # voice. Topics group rules by concern where names address one rule.
  omitTopics ? [],
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

  # The topical axis, orthogonal to render-target tags: what a rule is about,
  # for group denial via `omitTopics`. Extend deliberately; consumers depend
  # on these names the way they depend on rule names.
  knownTopics = [
    "agency" # decisiveness, autonomy, guard handling
    "architecture" # code-structure and dependency doctrine
    "comms" # disclosure and outward-facing messaging
    "tooling" # kernel, structured output, harness mechanics
    "verification" # evidence, repro, experiments
    "workflow" # git, issues, CI, landing changes
    "writing" # the human-voice register: prose, style, report shape
  ];

  normalizeRule = rule: let
    names = builtins.attrNames rule;
    name = builtins.head names;
    value = builtins.getAttr name rule;
    valueNames = builtins.attrNames value;
    # attrNames is lexicographically sorted; legal shapes are `reason` and
    # `text` plus optional `tags` (render-target narrowing) and optional
    # `topics` (group-denial axis).
    legalShapes = [
      ["reason" "text"]
      ["reason" "tags" "text"]
      ["reason" "text" "topics"]
      ["reason" "tags" "text" "topics"]
    ];
  in
    assert lib.assertMsg (
      builtins.length names == 1
    ) "prompt/rules.nix: each rule entry must have exactly one attribute";
    assert lib.assertMsg (
      builtins.elem valueNames legalShapes
    ) "prompt/rules.nix: rule `${name}` must have `reason` and `text` (and optional `tags`/`topics` lists)"; {
      inherit name;
      inherit (value) text reason;
      tags = value.tags or [];
      topics = value.topics or [];
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
  unknownRuleTopics = builtins.filter (topic: !(builtins.elem topic knownTopics)) (
    lib.unique (lib.concatMap (rule: rule.topics) canonicalRules)
  );
  unknownOmitTopics = builtins.filter (topic: !(builtins.elem topic knownTopics)) omitTopics;
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
      rule:
        !(builtins.elem rule.name omitRules)
        && !(builtins.any (topic: builtins.elem topic omitTopics) rule.topics)
        && applies rule
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
    assert lib.assertMsg (
      unknownRuleTopics == []
    ) "prompt/rules.nix: unknown topics: ${lib.concatStringsSep ", " unknownRuleTopics}";
    assert lib.assertMsg (
      unknownOmitTopics == []
    ) "prompt: omitTopics not in the known topic list: ${lib.concatStringsSep ", " unknownOmitTopics}";
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
