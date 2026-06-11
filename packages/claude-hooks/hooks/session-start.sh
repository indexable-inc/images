#!/usr/bin/env bash
set -e

dir="${CLAUDE_PROJECT_DIR:-${CODEX_PROJECT_DIR:-$PWD}}"

HOSTNAME=$(hostname -s)
echo "You are running on host: $HOSTNAME. Run commands locally - do NOT ssh to this host."

if git -C "$dir" rev-parse --git-dir >/dev/null 2>&1; then
  origin=$(git -C "$dir" remote get-url origin 2>/dev/null || true)
  if [ -n "$origin" ]; then
    case "$origin" in
    *github.com*) forge="GitHub (use gh CLI)" ;;
    *git.harivan.sh*) forge="Forgejo at git.harivan.sh (use tea CLI)" ;;
    *) forge="unknown forge" ;;
    esac
    echo "=== Origin ==="
    echo "$origin -> $forge"
  fi
  echo "=== Recent commits ==="
  git -C "$dir" log --oneline -10
fi
