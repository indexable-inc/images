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

  mkAgentsDir =
    {
      pkgs,
      agents,
    }:
    let
      farm = pkgs.linkFarm "claude-agents-farm" (
        lib.mapAttrsToList (name: agent: {
          name = "${name}.md";
          path = pkgs.writeText "${name}.md" (renderAgent name agent);
        }) agents
      );
    in
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
      `frontmatter.name`, if present, must equal the attribute key.

    Returns a directory with one `<name>.md` per agent, built as real files with
    no symlinks (Claude Code drops symlinked agent entries,
    anthropics/claude-code#36659), so deliver it by copying into a repository's
    `.claude/agents` rather than symlinking the store path.
  */
  inherit mkAgentsDir;
}
