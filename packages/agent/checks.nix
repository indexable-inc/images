# Materialization gate for the agent surface (#3898), imported by
# lib/per-system.nix into the per-system check catalog.
{
  mkCheck,
  skillsDir,
  agentsDir,
}: {
  # Skills and subagents are rendered live by the SessionStart hook.
  # This gate forces both materialized directories to build.
  agent-skills = mkCheck "agent-skills" {
    script = ''
      test -d ${skillsDir}
      test -d ${agentsDir}
    '';
  };
}
