#!/usr/bin/env bash
set -euo pipefail

# The composed fix patch for the checked-out tree (the PR's synthetic merge).
# The required gate already built the fixer lanes (the lint check judges the
# fixed tree), so this build only assembles the diff on top of cached lanes;
# an empty patch is the common no-drift exit.
patch="$(nix build .#lint-fix-patch --no-link --print-out-paths \
  --option extra-experimental-features ca-derivations)"
if [[ ! -s "$patch" ]]; then
  echo "lint autofix: no fixable drift"
  exit 0
fi

# The lint gate passed on the FIXED tree, so without a pushable fix this
# drift would merge unrepaired; fail rather than go silently green. Token
# minting is skipped for fork PRs and when the MIRROR_APP_* credentials are
# absent (see check.yml).
if [[ -z "${AUTOFIX_PUSH_TOKEN:-}" ]]; then
  echo "fixable lint drift, but no autofix push token is available;" \
    "apply the fix locally with: nix run .#lint -- --fix" >&2
  exit 1
fi

# The patch is computed against the synthetic merge tree but the commit must
# sit on the PR head, so apply it in a scratch worktree of the head SHA (the
# merge commit's second parent, always present in this full clone). main is
# lint-clean by induction (this gate), so merge-tree drift is head drift and
# the patch applies; `git apply` inside lint-autofix is all-or-nothing, so
# when main touched the same hunks the run fails loudly and rebasing the PR
# is the fix.
worktree="${RUNNER_TEMP:?RUNNER_TEMP is required}/lint-autofix"
git worktree add --quiet "$worktree" "${HEAD_SHA:?HEAD_SHA is required}"
cd "$worktree"
exec nix run "${GITHUB_WORKSPACE:?GITHUB_WORKSPACE is required}#lint-autofix" -- \
  --patch "$patch" \
  --push-url "https://github.com/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}.git" \
  --branch "${HEAD_REF:?HEAD_REF is required}" \
  --expected-head "$HEAD_SHA"
