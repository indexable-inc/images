# Body of the `main-sync` writeShellApplication (see profiles/darwin-home.nix).
#
# Keeps the shared main checkouts of ix and index fast-forwarded to
# origin/main so worktrees branch from a fresh base. Conservative by design:
# it fetches always (safe, never touches the working tree) and only advances
# the local main ref when HEAD is main AND the tree is clean AND the move is a
# fast-forward. WIP, detached heads, or a non-ff divergence are left untouched.
set -euo pipefail

repos=(
  "$HOME/Projects/indexable-inc/ix"
  "$HOME/Projects/indexable-inc/index"
)

for repo in "${repos[@]}"; do
  [ -d "$repo/.git" ] || continue

  # Fetch is always safe: updates remote-tracking refs only.
  git -C "$repo" fetch --quiet origin main || continue

  branch="$(git -C "$repo" rev-parse --abbrev-ref HEAD)"
  [ "$branch" = "main" ] || continue

  # Skip if the working tree or index has changes.
  git -C "$repo" diff --quiet --ignore-submodules || continue
  git -C "$repo" diff --cached --quiet --ignore-submodules || continue

  # --ff-only refuses anything that is not a clean fast-forward, so a
  # divergence (local commits on main) is reported and skipped, never merged.
  git -C "$repo" merge --ff-only --quiet origin/main || continue
done
