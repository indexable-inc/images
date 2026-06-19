{ lib }:
# Render declarative subagent definitions to a `.claude/agents/<name>.md`
# directory. An agent is `{ frontmatter; body; }`: `frontmatter` is an attrset
# (its `mcpServers` value comes straight from `ix.mcp.toClaudeJson`, so a
# subagent's servers are declared from the same registry the wrappers bake, not
# hand-copied), and `body` is the markdown system prompt. This is the agent
# sibling of `skills.mkSkillsDir`.
let
  # YAML frontmatter is a superset of JSON, so a nested value (mcpServers, a
  # tools list) is emitted as inline JSON on one line, which a YAML parser reads
  # back identically. Plain strings stay bare to match the handwritten skill
  # frontmatter (`description: Use X: do Y` keeps its colons). The fixed leading
  # order keeps the rendered file stable and readable; any extra keys follow,
  # sorted, so the output is deterministic regardless of attrset order.
  renderValue = v: if builtins.isString v then v else builtins.toJSON v;

  leadKeys = [
    "name"
    "description"
    "tools"
    "model"
    "mcpServers"
  ];

  renderFrontmatter =
    fm:
    let
      present = builtins.attrNames fm;
      ordered =
        (builtins.filter (k: builtins.elem k present) leadKeys)
        ++ lib.sort lib.lessThan (lib.subtractLists leadKeys present);
      line = k: "${k}: ${renderValue fm.${k}}";
    in
    lib.concatStringsSep "\n" (map line ordered);

  renderAgent =
    name: agent:
    assert lib.assertMsg (agent.frontmatter.name or name == name)
      "agents.mkAgentsDir: agent ${name} has frontmatter.name=${agent.frontmatter.name or "?"} (must match its key)";
    ''
      ---
      ${renderFrontmatter agent.frontmatter}
      ---

      ${agent.body}'';

  # The `name:` from a hand-authored agent file's YAML frontmatter (the first
  # `name:` line inside the leading `---` block), or null if absent. Used to
  # check a raw file's declared name against its filename, the same invariant
  # `renderAgent` enforces for rendered agents. The file is known to open with
  # `---` (per-system.nix's discovery filters on that before this runs).
  rawFrontmatterName =
    path:
    let
      lines = lib.splitString "\n" (builtins.readFile path);
      # Frontmatter is the leading block, so the first `name:` line is the
      # declared agent name (a body line starting `name:` can't precede it).
      nameLine = lib.findFirst (l: lib.hasPrefix "name:" l) null lines;
    in
    if nameLine == null then null else lib.removePrefix "name: " nameLine;

  mkAgentsDir =
    {
      pkgs,
      agents ? { },
      rawFiles ? [ ],
    }:
    let
      renderedEntries = lib.mapAttrsToList (name: agent: {
        name = "${name}.md";
        path = pkgs.writeText "${name}.md" (renderAgent name agent);
      }) agents;
      rawEntries = map (
        f:
        let
          fmName = rawFrontmatterName f.path;
        in
        assert lib.assertMsg (fmName == f.name)
          "agents.mkAgentsDir: raw agent file \"${f.name}.md\" declares frontmatter name=${
            if fmName == null then "(missing)" else "\"${fmName}\""
          } (must match its filename)";
        {
          name = "${f.name}.md";
          inherit (f) path;
        }
      ) rawFiles;
      entries = renderedEntries ++ rawEntries;
      names = map (e: e.name) entries;
      collisions = lib.filter (n: lib.count (x: x == n) names > 1) (lib.unique names);
      farm = pkgs.linkFarm "claude-agents-farm" entries;
    in
    assert lib.assertMsg (
      collisions == [ ]
    ) "agents.mkAgentsDir: duplicate agent name(s): ${lib.concatStringsSep ", " collisions}";
    # Materialize real files, no symlinks: Claude Code's agent/`/`-autocomplete
    # discovery drops symlinked entries (anthropics/claude-code#36659), the same
    # reason skills.mkSkillsDir dereferences here in the sandbox rather than
    # asking each consumer to do it on the host.
    pkgs.runCommand "claude-agents" { } ''
      cp -RL ${farm} "$out"
      links=$(find "$out" -type l)
      if [ -n "$links" ]; then
        echo "claude-agents: symlinks survived materialization:" >&2
        echo "$links" >&2
        exit 1
      fi
    '';
in
{
  /**
    Build a single directory of rendered subagents for `.claude/agents`.

    Arguments:
    - `pkgs`: the package set used to build the directory.
    - `agents`: attrset from agent name to `{ frontmatter; body; }`. `frontmatter`
      is rendered to the agent file's YAML frontmatter (nested values such as
      `mcpServers` as inline JSON); `body` is the markdown system prompt. A
      `frontmatter.name`, if present, must equal the attribute key. Use this for
      agents whose frontmatter is computed (e.g. `mcpServers` from `ix.mcp`).
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
