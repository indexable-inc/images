#!/usr/bin/env bash
# Serve the prebuilt agent skills and subagents at session start.
#
# Skills and subagents are not committed (see .gitignore); they are materialized
# here at session start into .claude/skills + .claude/agents (Claude Code) /
# .agents/skills (Codex).
#
# Subagents are rendered from Nix, so they can only come from the prebuilt
# IX_CLAUDE_AGENTS_DIR the wrapper exposes. Skills are plain directories that
# this repo tracks, so they come from the checkout instead, with
# IX_CLAUDE_SKILLS_DIR filling only what the checkout cannot supply -- see
# materialize_skills below for why that distinction is the whole point.
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

# Materialize the skills catalog onto disk. The destination must be a real
# directory of real files, not a symlink to the store: Claude Code's
# `/`-autocomplete discovery filters symlinks (anthropics/claude-code#36659)
# even though the skill *loader* follows them fine.
#
# Content comes from two places, and the checkout wins:
#
#   $root/packages/agent/skills/<name>/  the tracked source
#   $IX_CLAUDE_SKILLS_DIR                the `.#skills` output the Claude
#                                        wrapper baked into its launch spec
#
# The wrapper's store path is fixed at `darwin-rebuild switch` time and reaches
# this repo three pins away (index -> ix -> the host's nix config), so on its
# own it materializes whatever packages/agent/skills looked like whenever the
# machine was last rebuilt. Nothing an agent can do in the checkout moves it:
# not pulling, not rebasing, not editing the skill it is reading. ENG-11189
# caught that copy 11 skills of 56 out of date in a live checkout, `linting`
# 125 diff lines behind the file sitting next to it, with an agent reading a
# version that predated the fix for the exact mistake that skill exists to
# prevent. Nothing noticed, because the copy is gitignored.
#
# Copying the tree last is what makes that unreachable: the materialized
# catalog is a copy of the working tree, so it cannot lag the working tree. The
# store path supplies only the entries no checkout can (skills lib/skills.nix
# resolves out of a packaged upstream, named in vendored-skills.txt); anything
# else it carries is a skill this checkout has deleted, and does not come back.
# `nix flake check`'s agent-skills-materialize gate runs this function and
# diffs the result against `.#skills`, so this reimplementation cannot drift
# from mkSkillsDir either.
materialize_skills() {
  local dest=$1
  local store=${IX_CLAUDE_SKILLS_DIR:-}
  local tree=$root/packages/agent/skills
  local manifest=$tree/vendored-skills.txt
  local name src

  rm -rf "$dest"
  mkdir -p "$dest"

  if [ ! -d "$tree" ]; then
    # A checkout without the skills source (this hook is shared with consumer
    # repos): the wrapper's copy is the only catalog available, so take it
    # whole and accept that it is only as fresh as the last system rebuild.
    copy_tree "$store" "$dest"
    return 0
  fi

  if [ -d "$store" ] && [ -r "$manifest" ]; then
    while IFS= read -r name; do
      case "$name" in "" | \#*) continue ;; esac
      [ -d "$store/$name" ] || continue
      cp -RL "$store/$name" "$dest/"
    done < "$manifest"
  fi

  for src in "$tree"/*/; do
    [ -d "$src" ] || continue
    cp -RL "${src%/}" "$dest/"
  done

  chmod -R u+w "$dest"
}

case "$target" in
codex-md) materialize_skills "$root/.agents/skills" ;;
*)        materialize_skills "$root/.claude/skills" ;;
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
