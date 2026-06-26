{
  lib,
  toml,
}:
# Render the same declarative subagent definitions used by Claude Code into
# Codex custom-agent TOML files. Codex identifies custom agents by the `name`
# field and loads one standalone file per agent from `.codex/agents/`.
let
  assertOptional =
    agentName: attrs: field: predicate: expected:
    assert lib.assertMsg (
      !(builtins.hasAttr field attrs) || predicate attrs.${field}
    ) "codexAgents.mkAgentsDir: agent ${agentName} ${field} must be ${expected}";
    true;

  effortMap = {
    xhigh = "high";
    high = "high";
    medium = "medium";
    low = "low";
  };

  isStringList = value: builtins.isList value && lib.all builtins.isString value;

  hasWriteTool =
    tools:
    lib.any (tool: builtins.elem tool [
      "Edit"
      "MultiEdit"
      "NotebookEdit"
      "Write"
    ]) tools;

  renderScalar =
    key: value:
    ''${key} = ${toml.scalar value}'';

  renderList =
    key: values:
    ''${key} = [ ${lib.concatStringsSep ", " (map toml.scalar values)} ]'';

  renderMcpServer =
    name: value:
    let
      attrs = value.${name};
      common = [
        "[mcp_servers.${name}]"
        ''default_tools_approval_mode = "approve"''
      ];
      stdio =
        common
        ++ [
          (renderScalar "command" attrs.command)
        ]
        ++ lib.optional (attrs ? args) (renderList "args" attrs.args)
        ++ lib.optional (attrs ? env && attrs.env != { }) (
          lib.concatStringsSep "\n" (
            [
              "[mcp_servers.${name}.env]"
            ]
            ++ lib.mapAttrsToList renderScalar attrs.env
          )
        );
      http =
        common
        ++ [
          (renderScalar "url" attrs.url)
        ];
    in
    lib.concatStringsSep "\n" (
      if (attrs.type or "stdio") == "http" then http else stdio
    );

  renderAgentMcp =
    servers:
    lib.concatStringsSep "\n\n" (
      map (
        server:
        let
          names = builtins.attrNames server;
          name = builtins.head names;
        in
        assert lib.assertMsg (builtins.length names == 1)
          "codexAgents.mkAgentsDir: mcpServers entries must contain exactly one server";
        renderMcpServer name server
      ) servers
    );

  renderAgent =
    name: agent:
    let
      frontmatter = agent.frontmatter or (throw "codexAgents.mkAgentsDir: agent ${name} is missing frontmatter");
      content = agent.content or (throw "codexAgents.mkAgentsDir: agent ${name} is missing content");
      tools = frontmatter.tools or [ ];
      codex = frontmatter.codex or { };
      mcpServers = frontmatter.mcpServers or [ ];
      rendered =
        [
          (renderScalar "name" (frontmatter.name or name))
          (renderScalar "description" frontmatter.description)
          (renderScalar "developer_instructions" content)
        ]
        ++ lib.optional (frontmatter ? effort && !(codex ? model_reasoning_effort) && builtins.hasAttr frontmatter.effort effortMap)
          (renderScalar "model_reasoning_effort" effortMap.${frontmatter.effort})
        ++ lib.optional (tools != [ ] && !hasWriteTool tools)
          (renderScalar "sandbox_mode" "read-only")
        ++ lib.optional (codex ? model) (renderScalar "model" codex.model)
        ++ lib.optional (codex ? model_reasoning_effort)
          (renderScalar "model_reasoning_effort" codex.model_reasoning_effort)
        ++ lib.optional (codex ? sandbox_mode) (renderScalar "sandbox_mode" codex.sandbox_mode)
        ++ lib.optional (codex ? nickname_candidates)
          (renderList "nickname_candidates" codex.nickname_candidates)
        ++ lib.optional (mcpServers != [ ]) (renderAgentMcp mcpServers);
    in
    assert lib.assertMsg ((frontmatter.name or name) == name)
      "codexAgents.mkAgentsDir: agent ${name} has frontmatter.name=${frontmatter.name or "?"} (must match its key)";
    assert lib.assertMsg (builtins.isString content)
      "codexAgents.mkAgentsDir: agent ${name} content must be a string";
    assert lib.assertMsg (builtins.isString frontmatter.description)
      "codexAgents.mkAgentsDir: agent ${name} frontmatter.description must be a string";
    assert lib.assertMsg (assertOptional name frontmatter "tools" isStringList "a list of strings")
      "codexAgents.mkAgentsDir: agent ${name} frontmatter.tools failed validation";
    assert lib.assertMsg (assertOptional name codex "nickname_candidates" isStringList "a list of strings")
      "codexAgents.mkAgentsDir: agent ${name} frontmatter.codex.nickname_candidates failed validation";
    lib.concatStringsSep "\n" rendered + "\n";

  stripYamlString =
    value:
    let
      trimmed = lib.trim value;
      length = builtins.stringLength trimmed;
    in
    if length >= 2 && lib.hasPrefix "\"" trimmed && lib.hasSuffix "\"" trimmed then
      lib.substring 1 (length - 2) trimmed
    else if length >= 2 && lib.hasPrefix "'" trimmed && lib.hasSuffix "'" trimmed then
      lib.substring 1 (length - 2) trimmed
    else
      trimmed;

  splitRawMarkdown =
    path:
    let
      lines = map (lib.removeSuffix "\r") (lib.splitString "\n" (builtins.readFile path));
      step =
        state: line:
        if state.inFrontmatter then
          if line == "---" then
            state // {
              inFrontmatter = false;
              closed = true;
            }
          else
            state // { frontmatter = state.frontmatter ++ [ line ]; }
        else
          state // { body = state.body ++ [ line ]; };
      parsed = builtins.foldl' step {
        inFrontmatter = true;
        closed = false;
        frontmatter = [ ];
        body = [ ];
      } (lib.drop 1 lines);
    in
    assert lib.assertMsg (lines != [ ] && builtins.head lines == "---")
      "codexAgents.mkAgentsDir: raw agent ${toString path} must start with YAML frontmatter";
    assert lib.assertMsg parsed.closed
      "codexAgents.mkAgentsDir: raw agent ${toString path} has unterminated YAML frontmatter";
    parsed;

  rawFrontmatterValue =
    frontmatter: key:
    let
      prefix = "${key}:";
      matches = lib.filter (line: lib.hasPrefix prefix line) frontmatter;
      match = builtins.match "[^:]+:[[:space:]]*(.*)" (builtins.head matches);
    in
    if matches == [ ] then null else stripYamlString (builtins.head match);

  rawMarkdownAgent =
    file:
    let
      parsed = splitRawMarkdown file.path;
      name = rawFrontmatterValue parsed.frontmatter "name";
      description = rawFrontmatterValue parsed.frontmatter "description";
      toolsText = rawFrontmatterValue parsed.frontmatter "tools";
      tools =
        if toolsText == null || toolsText == "" then
          [ ]
        else
          map lib.trim (lib.splitString "," toolsText);
    in
    assert lib.assertMsg (name == file.name)
      "codexAgents.mkAgentsDir: raw agent ${file.name} has frontmatter.name=${if name == null then "?" else name} (must match its file name)";
    assert lib.assertMsg (description != null && description != "")
      "codexAgents.mkAgentsDir: raw agent ${file.name} is missing description frontmatter";
    {
      frontmatter = {
        inherit description tools;
        name = file.name;
      };
      content = lib.concatStringsSep "\n" parsed.body;
    };

  mkAgentsDir =
    {
      pkgs,
      agents ? { },
      rawFiles ? [ ],
    }:
    let
      renderedEntries =
        lib.mapAttrsToList (name: agent: {
          name = "${name}.toml";
          path = pkgs.writeText "${name}.toml" (renderAgent name agent);
        }) agents
        ++ map (file: {
          name = "${file.name}.toml";
          path = pkgs.writeText "${file.name}.toml" (renderAgent file.name (rawMarkdownAgent file));
        }) rawFiles;
      entryNames = map (entry: entry.name) renderedEntries;
      duplicateEntries =
        lib.unique (
          lib.filter (name: (builtins.length (lib.filter (candidate: candidate == name) entryNames)) > 1) entryNames
        );
    in
    assert lib.assertMsg (duplicateEntries == [ ])
      "codexAgents.mkAgentsDir: duplicate rendered Codex agent file(s): ${lib.concatStringsSep ", " duplicateEntries}";
    pkgs.linkFarm "codex-agents" renderedEntries;
in
{
  inherit mkAgentsDir renderAgent;
}
