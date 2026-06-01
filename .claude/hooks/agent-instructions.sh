#!/usr/bin/env bash
# Serve the live-rendered agent context at session start.
#
# CLAUDE.md and AGENTS.md are not committed (see .gitignore). The always-on core
# is rendered from agent-context/sections (the `disclosure: always` fragments)
# into the `claude-md` / `codex-md` flake packages, and the `disclosure:
# progressive` fragments become on-demand skills in the `skills` flake package
# alongside the handwritten skills/. This hook builds those packages, prints the
# always-on core as `additionalContext`, and repoints `.claude/skills` (Claude
# Code) / `.agents/skills` (Codex) at the generated skill link farm.
#
# Output is the SessionStart hook JSON envelope. Codex requires a JSON object
# with hookSpecificOutput.additionalContext (its parser rejects plain stdout);
# Claude Code accepts the same shape, so one format serves both tools.
#
# Unlike a chunked scheme, this emits the whole always-on core as a single
# additionalContext value. That is safe because lib/agent-context.nix asserts at
# build time that the core stays under Claude Code's 10000-char per-value cap, so
# `nix build` fails loudly if a section is mismarked `always` rather than
# silently truncating live context.
set -euo pipefail

# Claude Code exports CLAUDE_PROJECT_DIR; Codex runs the hook from the session
# cwd (which may be a subdirectory), so fall back to the git root there.
root=${CLAUDE_PROJECT_DIR:-}
[ -n "$root" ] || root=$(git rev-parse --show-toplevel)

# First positional arg selects the instruction target: `claude-md` (CLAUDE.md)
# for Claude Code, `codex-md` (AGENTS.md) for Codex.
package=${1:-claude-md}

# jaq builds the JSON envelope and safely escapes the document. Prefer one on
# PATH; fall back to nixpkgs so the hook works before direnv loads the devshell.
if command -v jaq >/dev/null 2>&1; then
  jaq() { command jaq "$@"; }
else
  jaq() { nix run nixpkgs#jaq -- "$@"; }
fi

doc=$(nix build --no-link --print-out-paths "$root#$package")

# Repoint skills at the generated link farm. Claude Code reads `.claude/skills`,
# Codex reads `.agents/skills`. Best-effort: a skills-build failure must not
# abort the session, so guard the build and skip the repoint if it produces no
# path.
skills_store=$(nix build --no-link --print-out-paths "$root#skills" 2>/dev/null || true)
if [ -n "$skills_store" ]; then
  case "$package" in
  codex-md)
    mkdir -p "$root/.agents"
    ln -sfn "$skills_store" "$root/.agents/skills"
    ;;
  *)
    ln -sfn "$skills_store" "$root/.claude/skills"
    ;;
  esac
fi

# Codex: emit the document as one value, no reloadSkills field its parser might
# reject.
if [ "$package" = codex-md ]; then
  jaq -n --rawfile additionalContext "$doc" \
    '{hookSpecificOutput: {hookEventName: "SessionStart", additionalContext: $additionalContext}}'
  exit 0
fi

# Claude Code: emit the core plus reloadSkills so the freshly repointed
# .claude/skills is picked up this session.
jaq -n --rawfile additionalContext "$doc" \
  '{hookSpecificOutput: {hookEventName: "SessionStart", additionalContext: $additionalContext, reloadSkills: true}}'
