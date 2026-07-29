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

  # Acronyms a rendered rule may leave bare. Closed on purpose: the
  # `defineAcronyms` rule renders this same list, and `undefinedAcronyms`
  # below fails any render that introduces an all-caps token which is
  # neither listed here nor expanded as "words (TOKEN)" in the text. The
  # membership test is "a programmer who has never seen this repo reads it
  # without expanding", not "it appears in this file": the rule governs
  # everything the agent writes, so a list scoped to the prompt would make
  # it expand `SQL` and `VM` in replies. `CDP` and `DAG` were in this text
  # and are not on the list, which is why the exception is a list rather
  # than the author's judgment (index#1616).
  bareAcronyms = [
    "AI"
    "API"
    "CI"
    "CLI"
    "CPU"
    "CSS"
    "DNS"
    "DOM"
    "HTML"
    "HTTP"
    "ID"
    "JS"
    "JSON"
    "OS"
    "PDF"
    "PR"
    "RAM"
    "SDK"
    "SQL"
    "SSH"
    "TCP"
    "TLS"
    "UI"
    "URL"
    "VM"
    "XML"
    "YAML"
  ];
  # Wrapped to the width of the prose around it: the list renders inside a
  # rule body, and one 118-character line in a 70-column prompt reads as a
  # formatting bug.
  bareAcronymList = let
    items = lib.imap0 (index: acronym:
      if index == builtins.length bareAcronyms - 1
      then acronym
      else "${acronym},")
    bareAcronyms;
    lines =
      lib.foldl (
        wrapped: item: let
          joined = "${lib.last wrapped} ${item}";
        in
          if wrapped == []
          then [item]
          else if builtins.stringLength joined <= 66
          then lib.init wrapped ++ [joined]
          else wrapped ++ [item]
      ) []
      items;
  in
    lib.concatStringsSep "\n" lines;

  rulesFor = agentName:
    map normalizeRule (import ./rules.nix {
      inherit agentName bareAcronymList;
    });

  # Acronyms a rendered prompt uses without ever expanding them, and without
  # listing them in `bareAcronyms`. Order is not checked, so an expansion
  # after first use satisfies this; it catches undefined acronyms, not
  # misplaced definitions.
  #
  # Each step exists because a first version without it let a real acronym
  # through, measured by editing one rule and re-rendering:
  #  - a backticked span is skipped, since `IX_MCP_ISSUE_WATCH_OWNERS` is an
  #    identifier to copy, not an acronym, and underscores split it into five
  #    invented ones. An all-caps span like `RPC` is not an identifier and is
  #    scanned anyway, otherwise one backtick disables the check;
  #  - digits stay inside a token, or `S3` splits into `S` and `3` and
  #    matches nothing;
  #  - a trailing plural `s` is stripped, or `CDPs` reads as unremarkable
  #    mixed case;
  #  - the expansion must be a lowercase word immediately before the
  #    parenthesis, or a bare `(RPC)` anywhere in the text excuses every
  #    other use of it.
  #
  # What it still misses: any lowercase word before the parenthesis counts,
  # so "commands (RPC)" passes as an expansion. Checking that the preceding
  # words carry the acronym's initials would catch it, at the cost of
  # rejecting real expansions that carry a filler word ("directed acyclic
  # graph of patches (DAG)"). A reviewer reading a rule text still has to
  # catch that one.
  undefinedAcronyms = text: let
    parts = builtins.split "(`[^`]*`)" text;
    outsideCode = lib.concatStrings (builtins.filter builtins.isString parts);
    codeSpans = map builtins.head (builtins.filter builtins.isList parts);
    acronymSpans = builtins.filter (span: builtins.match "`[A-Z]+`" span != null) codeSpans;
    prose = lib.concatStringsSep " " ([outsideCode] ++ acronymSpans);
    words = builtins.filter builtins.isString (builtins.split "[^A-Za-z0-9]+" prose);
    candidates = map (lib.removeSuffix "s") (
      builtins.filter (word: builtins.match "[A-Z][A-Z0-9]+s?" word != null) words
    );
    expanded = acronym: builtins.match ".*[a-z] \\(${acronym}\\)s?.*" prose != null;
  in
    lib.unique (builtins.filter (
        acronym: !(builtins.elem acronym bareAcronyms) && !(expanded acronym)
      )
      candidates);

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
    rendered = lib.concatStringsSep "\n\n" (map (rule: rule.text) kept);
    undefined = undefinedAcronyms rendered;
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
    assert lib.assertMsg (
      undefined == []
    ) "prompt/rules.nix: acronyms used without an expansion: ${lib.concatStringsSep ", " undefined}; expand each as \"words (ACRONYM)\" in the rule text, or add it to `bareAcronyms` in ./default.nix with a reason"; rendered;
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
