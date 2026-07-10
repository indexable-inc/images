# Body of the `git-wt-submodules` writeShellApplication (see home/common.nix).
# No shebang / `set` line: writeShellApplication supplies bash + `set -euo pipefail`
# and bakes git onto PATH via runtimeInputs.
#
# Installed as `git-wt-submodules`, so git exposes it as `git wt-submodules`.
#
# Initializes this checkout's submodules by BORROWING the main clone's
# already-downloaded objects (git's --reference / objects/info/alternates)
# instead of re-cloning each submodule over the network.
#
# Why: `git worktree add` does not check out submodules, and a plain
# `git submodule update --init` in a fresh worktree clones every submodule again
# from its remote into a per-worktree gitdir
# (<common>/.git/worktrees/<wt>/modules/<name>). But a linked worktree shares the
# main clone's common git dir, which already holds each submodule's objects under
# <common>/modules/<name>. Referencing those makes the bulk history come from
# local disk: no network, near-zero extra space.
#
# Note: --reference makes the worktree depend on those objects existing; do not
# gc/delete the main clone out from under it. Pass --dissociate to copy the
# borrowed objects in instead (independent, costs disk, still no network).
#
# Usage, after creating a worktree:
#   git -C <worktree> wt-submodules            # share objects (default)
#   git -C <worktree> wt-submodules --dissociate
# Any submodule whose objects are not present locally falls back to a plain init.

dissociate=()
case "${1:-}" in
  --dissociate) dissociate=(--dissociate) ;;
  "") ;;
  *) echo "git-wt-submodules: unknown argument: $1" >&2; exit 2 ;;
esac

# --is-inside-work-tree exits 0 but prints "false" in a bare repo or inside a
# .git dir, so check the printed value, not just the exit code.
if [ "$(git rev-parse --is-inside-work-tree 2>&1)" != true ]; then
  echo "git-wt-submodules: not inside a git work tree" >&2
  exit 1
fi

cd "$(git rev-parse --show-toplevel)"

if [ ! -f .gitmodules ]; then
  echo "git-wt-submodules: no .gitmodules here; nothing to do"
  exit 0
fi

common=$(git rev-parse --path-format=absolute --git-common-dir)
gitdir=$(git rev-parse --path-format=absolute --git-dir)

borrowed=0
plain=0
while IFS= read -r key; do
  name=${key#submodule.}
  name=${name%.path}
  path=$(git config -f .gitmodules "submodule.$name.path")
  ref="$common/modules/$name"
  # Borrow only when the main clone actually holds the objects, and we are not the
  # main worktree (which would reference its own gitdir: a no-op at best).
  if [ -d "$ref/objects" ] && [ "$ref" != "$gitdir/modules/$name" ]; then
    echo "-> $path  (--reference $ref)"
    git submodule update --init --recursive --reference "$ref" "${dissociate[@]}" -- "$path"
    borrowed=$((borrowed + 1))
  else
    echo "-> $path  (plain init)"
    git submodule update --init --recursive -- "$path"
    plain=$((plain + 1))
  fi
done < <(git config -f .gitmodules --name-only --get-regexp '\.path$')

echo "git-wt-submodules: $borrowed borrowed from local objects, $plain plain"
