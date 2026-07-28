#!/usr/bin/env bash
# Serve the prebuilt agent skills and subagents at session start.
#
# Skills and subagents are not committed (see .gitignore). They are Nix-built by
# the agent wrapper and exposed through IX_CLAUDE_SKILLS_DIR and
# IX_CLAUDE_AGENTS_DIR. This hook only copies those already-built store paths
# into .claude/skills + .claude/agents (Claude Code) / .agents/skills (Codex).
#
# Output is the SessionStart hook JSON envelope. Codex requires a JSON object
# with hookSpecificOutput.additionalContext (its parser rejects plain stdout);
# Claude Code accepts the same shape, so one format serves both tools. There is
# no always-on instruction document anymore (all guidance is on-demand skills),
# so additionalContext is an empty string.
set -euo pipefail

# Claude Code exports CLAUDE_PROJECT_DIR; Codex runs the hook from the session
# cwd (which may be a subdirectory), so fall back to the git root there.
root=${CLAUDE_PROJECT_DIR:-}
[ -n "$root" ] || root=$(git rev-parse --show-toplevel)

# First positional arg selects the target tool: `claude-md` (Claude Code) or
# `codex-md` (Codex). It no longer names a document package; it only picks the
# destination layout below and whether to materialize subagents.
target=${1:-claude-md}

copy_tree() {
  local src=$1 dest=$2

  if [ -z "$src" ] || [ ! -d "$src" ]; then
    return 0
  fi

  rm -rf "$dest"
  mkdir -p "$dest"
  cp -R "$src"/. "$dest"/
  chmod -R u+w "$dest"
}

# Copy the skills package onto disk. The package is symlink-free by build-time
# assertion (see lib/skills.nix), but the destination directory itself must also
# be real rather than a symlink to the store: Claude Code's `/`-autocomplete
# discovery filters symlinks (anthropics/claude-code#36659) even though the skill
# *loader* follows them fine. The wrapper, not this startup hook, owns the Nix
# build so a stuck evaluator cannot block the first prompt.
case "$target" in
codex-md) copy_tree "${IX_CLAUDE_SKILLS_DIR:-}" "$root/.agents/skills" ;;
*)        copy_tree "${IX_CLAUDE_SKILLS_DIR:-}" "$root/.claude/skills" ;;
esac

# Claude Code also discovers subagents from .claude/agents/*.md. Codex's
# subagent model is config-driven (features.multi_agent_v2), not markdown
# files, so materialize the rendered agents only for Claude.
if [ "$target" != codex-md ]; then
  copy_tree "${IX_CLAUDE_AGENTS_DIR:-}" "$root/.claude/agents"
fi

# Codex: emit an empty additionalContext, no reloadSkills field its parser
# might reject.
if [ "$target" = codex-md ]; then
  printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":""}}'
  exit 0
fi

# Claude Code: reloadSkills so the freshly materialized .claude/skills is picked
# up this session.
printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"","reloadSkills":true}}'
