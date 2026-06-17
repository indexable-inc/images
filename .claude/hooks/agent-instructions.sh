#!/usr/bin/env bash
# Serve the live-rendered agent skills and subagents at session start.
#
# Skills and subagents are not committed (see .gitignore). The `skills` flake
# package holds one directory per skill under skills/; the `agents` package
# holds the rendered subagents. This hook builds those packages and copies them
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

# jq builds the JSON envelope. Prefer one on PATH; fall back to nixpkgs so the
# hook works before direnv loads the devshell.
if command -v jq >/dev/null 2>&1; then
  jq() { command jq "$@"; }
else
  jq() { nix run nixpkgs#jq -- "$@"; }
fi

# Copy the skills package onto disk. The package is symlink-free by build-time
# assertion (see lib/skills.nix), but the destination directory itself must also
# be real rather than a symlink to the store: Claude Code's `/`-autocomplete
# discovery filters symlinks (anthropics/claude-code#36659) even though the skill
# *loader* follows them fine. chmod because cp preserves the store's read-only
# mode and the next session's rm -rf must succeed. Best-effort: a skills-build
# failure must not abort the session, so guard the build and skip the copy if it
# produces no path.
skills_store=$(nix build --no-link --print-out-paths "$root#skills" 2>/dev/null || true)
if [ -n "$skills_store" ]; then
  case "$target" in
  codex-md) dest="$root/.agents/skills" ;;
  *)        dest="$root/.claude/skills" ;;
  esac
  rm -rf "$dest"
  mkdir -p "$dest"
  cp -R "$skills_store"/. "$dest"/
  chmod -R u+w "$dest"
fi

# Claude Code also discovers subagents from .claude/agents/*.md. Codex's
# subagent model is config-driven (features.multi_agent_v2), not markdown
# files, so materialize the rendered agents only for Claude. Same best-effort
# guard and symlink-free copy as the skills block above.
if [ "$target" != codex-md ]; then
  agents_store=$(nix build --no-link --print-out-paths "$root#agents" 2>/dev/null || true)
  if [ -n "$agents_store" ]; then
    dest="$root/.claude/agents"
    rm -rf "$dest"
    mkdir -p "$dest"
    cp -R "$agents_store"/. "$dest"/
    chmod -R u+w "$dest"
  fi
fi

# Codex: emit an empty additionalContext, no reloadSkills field its parser
# might reject.
if [ "$target" = codex-md ]; then
  jq -n '{hookSpecificOutput: {hookEventName: "SessionStart", additionalContext: ""}}'
  exit 0
fi

# Claude Code: reloadSkills so the freshly materialized .claude/skills is picked
# up this session.
jq -n '{hookSpecificOutput: {hookEventName: "SessionStart", additionalContext: "", reloadSkills: true}}'
