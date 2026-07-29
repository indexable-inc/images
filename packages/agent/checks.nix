# Materialization gate for the agent surface (#3898), imported by
# lib/per-system.nix into the per-system check catalog.
{
  pkgs,
  paths,
  mkCheck,
  skillsDir,
  agentsDir,
}: let
  materializer = paths.root + "/.claude/hooks/agent-instructions.sh";
in {
  # Skills and subagents are rendered live by the SessionStart hook.
  # This gate forces both materialized directories to build.
  agent-skills = mkCheck "agent-skills" {
    script = ''
      test -d ${skillsDir}
      test -d ${agentsDir}
    '';
  };

  # The SessionStart hook assembles `.claude/skills` in bash rather than reading
  # `.#skills`, because the copy has to track the checkout while the store path
  # the wrapper bakes only tracks the last system rebuild (ENG-11189). That
  # makes the hook a second implementation of mkSkillsDir, and a second
  # implementation is only safe if something holds the two byte-identical. This
  # is that something: run the hook against a checkout and a store copy taken
  # from the same revision, where the two definitions must agree exactly, and
  # diff the result.
  #
  # It catches the ways the bash can go wrong that no other gate sees: a
  # vendored skill dropped because vendored-skills.txt was not updated alongside
  # `vendoredSources`, a skill copied without its references/ subtree, a
  # non-skill file in paths.skills landing in the catalog. The drift the ticket
  # is about is prevented by construction, not by this check.
  agent-skills-materialize = mkCheck "agent-skills-materialize" {
    nativeBuildInputs = [pkgs.diffutils];
    script = ''
      root="$(mktemp -d)"
      mkdir -p "$root/packages/agent"
      cp -R ${paths.skills} "$root/packages/agent/skills"
      chmod -R u+w "$root"

      CLAUDE_PROJECT_DIR="$root" \
      IX_CLAUDE_SKILLS_DIR=${skillsDir} \
        bash ${materializer} claude-md > /dev/null

      if ! diff -r ${skillsDir} "$root/.claude/skills" > materialize.diff; then
        echo "agent-skills-materialize: the SessionStart hook's .claude/skills is not .#skills." >&2
        echo "Left is nix (.#skills), right is the hook. Fix .claude/hooks/agent-instructions.sh" >&2
        echo "or packages/agent/skills/vendored-skills.txt until the two agree:" >&2
        cat materialize.diff >&2
        exit 1
      fi
    '';
  };
}
