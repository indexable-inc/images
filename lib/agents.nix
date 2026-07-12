{
  lib,
  markdown,
}:
# Render declarative subagent definitions to a `.claude/agents/<name>.md`
# directory. An agent is `{ frontmatter; content; }`: `frontmatter` is an attrset
# (its `mcpServers` value comes straight from `ix.mcp.toClaudeJson`, so a
# subagent's servers are declared from the same registry the wrappers bake, not
# hand-copied), and `content` is the markdown system prompt. This is the agent
# sibling of `skills.mkSkillsDir`.
let
  frontmatterOrder = [
    "name"
    "description"
    "model"
    "effort"
    "color"
    "tools"
    "mcpServers"
  ];

  knownFrontmatter = frontmatterOrder;

  isStringList = value: builtins.isList value && lib.all builtins.isString value;
  isMcpServers = value:
    builtins.isList value && lib.all (entry: builtins.isString entry || builtins.isAttrs entry) value;

  assertOptional = agentName: frontmatter: field: predicate: expected:
    assert lib.assertMsg (
      !(builtins.hasAttr field frontmatter) || predicate frontmatter.${field}
    ) "agents.mkAgentsDir: agent ${agentName} frontmatter.${field} must be ${expected}"; true;

  validateAgent = name: agent: let
    frontmatter =
      agent.frontmatter or (throw "agents.mkAgentsDir: agent ${name} is missing frontmatter");
    content = agent.content or (throw "agents.mkAgentsDir: agent ${name} is missing content");
    unknown = lib.subtractLists knownFrontmatter (builtins.attrNames frontmatter);
  in
    assert lib.assertMsg (builtins.isAttrs frontmatter)
    "agents.mkAgentsDir: agent ${name} frontmatter must be an attrset";
    assert lib.assertMsg (builtins.isString content)
    "agents.mkAgentsDir: agent ${name} content must be a string";
    assert lib.assertMsg ((frontmatter.name or name) == name)
    "agents.mkAgentsDir: agent ${name} has frontmatter.name=${frontmatter.name or "?"} (must match its key)";
    assert lib.assertMsg (unknown == [])
    "agents.mkAgentsDir: agent ${name} has unknown frontmatter key(s): ${lib.concatStringsSep ", " unknown}";
    assert lib.assertMsg (builtins.hasAttr "description" frontmatter)
    "agents.mkAgentsDir: agent ${name} frontmatter.description is required";
    assert lib.assertMsg (
      assertOptional name frontmatter "name" builtins.isString
      "a string"
    ) "agents.mkAgentsDir: agent ${name} frontmatter.name failed validation";
    assert lib.assertMsg (
      assertOptional name frontmatter "description" builtins.isString
      "a string"
    ) "agents.mkAgentsDir: agent ${name} frontmatter.description failed validation";
    assert lib.assertMsg (
      assertOptional name frontmatter "model" builtins.isString
      "a string"
    ) "agents.mkAgentsDir: agent ${name} frontmatter.model failed validation";
    assert lib.assertMsg (
      assertOptional name frontmatter "effort" builtins.isString
      "a string"
    ) "agents.mkAgentsDir: agent ${name} frontmatter.effort failed validation";
    assert lib.assertMsg (
      assertOptional name frontmatter "color" builtins.isString
      "a string"
    ) "agents.mkAgentsDir: agent ${name} frontmatter.color failed validation";
    assert lib.assertMsg (
      assertOptional name frontmatter "tools" isStringList
      "a list of strings"
    ) "agents.mkAgentsDir: agent ${name} frontmatter.tools failed validation";
    assert lib.assertMsg (
      assertOptional name frontmatter "mcpServers" isMcpServers
      "a list of server names or inline server attrsets"
    ) "agents.mkAgentsDir: agent ${name} frontmatter.mcpServers failed validation"; {
      frontmatter =
        {
          inherit name;
        }
        // frontmatter;
      inherit content;
    };

  renderAgent = name: agent:
    markdown.renderDocument (
      validateAgent name agent
      // {
        order = frontmatterOrder;
      }
    );

  # The `name:` value from a hand-authored agent file's YAML frontmatter, or
  # null if absent. Used to check a raw file's declared name against its
  # filename, the same invariant `validateAgent` enforces for rendered agents.
  # The file is known to open with `---` (per-system.nix's discovery filters on
  # that first). The scan is restricted to the frontmatter block (between the
  # first two `---` fences) so a `name:` line in the markdown body can't be
  # mistaken for the declared name; `\r` is stripped so CRLF files parse; and
  # the value is taken via a regex tolerant of any whitespace after the colon
  # (`name:foo`, `name:  foo`), not a fixed `"name: "` prefix.
  rawFrontmatterName = path: let
    lines = map (lib.removeSuffix "\r") (lib.splitString "\n" (builtins.readFile path));
    # Lines after the opening `---`, up to (not including) the closing `---`.
    # foldl' rather than lib.takeWhile, which this nixpkgs pin lacks.
    collect = acc: l:
      if acc.done || l == "---"
      then acc // {done = true;}
      else acc // {out = acc.out ++ [l];};
    fmLines =
      (lib.foldl' collect {
        out = [];
        done = false;
      } (lib.drop 1 lines)).out;
    nameLine = lib.findFirst (lib.hasPrefix "name:") null fmLines;
    m =
      if nameLine == null
      then null
      else builtins.match "name:[[:space:]]*(.*)" nameLine;
    value =
      if m == null
      then null
      else builtins.head m;
  in
    if value == ""
    then null
    else value;

  mkAgentsDir = {
    pkgs,
    agents ? {},
    rawFiles ? [],
  }: let
    renderedEntries =
      lib.mapAttrsToList (name: agent: {
        name = "${name}.md";
        path = pkgs.writeText "${name}.md" (renderAgent name agent);
      })
      agents;
    rawEntries =
      map (
        f: let
          fmName = rawFrontmatterName f.path;
        in
          assert lib.assertMsg (fmName == f.name)
          "agents.mkAgentsDir: raw agent file \"${f.name}.md\" declares frontmatter name=${
            if fmName == null
            then "(missing)"
            else "\"${fmName}\""
          } (must match its filename)"; {
            name = "${f.name}.md";
            inherit (f) path;
          }
      )
      rawFiles;
    entries = renderedEntries ++ rawEntries;
    names = map (e: e.name) entries;
    collisions = lib.filter (n: lib.count (x: x == n) names > 1) (lib.unique names);
    farm = pkgs.linkFarm "claude-agents-farm" entries;
  in
    assert lib.assertMsg (
      collisions == []
    ) "agents.mkAgentsDir: duplicate agent name(s): ${lib.concatStringsSep ", " collisions}";
    # Materialize real files, no symlinks: Claude Code's agent/`/`-autocomplete
    # discovery drops symlinked entries (anthropics/claude-code#36659), the same
    # reason skills.mkSkillsDir dereferences here in the sandbox rather than
    # asking each consumer to do it on the host.
      pkgs.runCommand "claude-agents" {} ''
        cp -RL ${farm} "$out"
        links=$(find "$out" -type l)
        if [ -n "$links" ]; then
          echo "claude-agents: symlinks survived materialization:" >&2
          echo "$links" >&2
          exit 1
        fi
      '';
in {
  /**
  Build a single directory of rendered subagents for `.claude/agents`.

  Arguments:
  - `pkgs`: the package set used to build the directory.
  - `agents`: attrset from agent name to `{ frontmatter; content; }`.
    `frontmatter` is checked against Claude Code's agent metadata shape and
    rendered by `markdown.renderDocument`; `content` is the markdown system
    prompt. A `frontmatter.name`, if present, must equal the attribute key. Use
    this for agents whose frontmatter is computed (e.g. `mcpServers` from
    `ix.mcp`).
  - `rawFiles`: list of `{ name; path; }` for agents that already ship as a
    complete, hand-authored `.md` (frontmatter + body). The file at `path` is
    copied verbatim to `<name>.md`. Use this for static agents so adding one is
    just dropping a `.md` file, with no nix entry. The file's frontmatter
    `name:` must equal `name` (Claude registers the agent under the frontmatter
    name, so a mismatch would silently install it under the wrong handle), and
    names must not collide with `agents` keys.

  Returns a directory with one `<name>.md` per agent, built as real files with
  no symlinks (Claude Code drops symlinked agent entries,
  anthropics/claude-code#36659), so deliver it by copying into a repository's
  `.claude/agents` rather than symlinking the store path.
  */
  inherit mkAgentsDir;
}
