#!/usr/bin/env bash
set -euo pipefail

workflow=${1:?usage: workflow-base.sh CHECK_WORKFLOW}
repo="$TMPDIR/repo"

git -C "$TMPDIR" init --quiet --initial-branch=main repo
git -C "$repo" config user.name test
git -C "$repo" config user.email test@example.com

printf 'event base\n' >"$repo/base"
git -C "$repo" add base
git -C "$repo" commit --quiet --message base
event_base=$(git -C "$repo" rev-parse HEAD)

git -C "$repo" switch --quiet --create pull-request
printf 'pull request\n' >"$repo/contribution"
git -C "$repo" add contribution
git -C "$repo" commit --quiet --message contribution

git -C "$repo" switch --quiet main
printf 'new main\n' >"$repo/advanced"
git -C "$repo" add advanced
git -C "$repo" commit --quiet --message advance
checkout_base=$(git -C "$repo" rev-parse HEAD)
git -C "$repo" merge --quiet --no-ff --message synthetic-merge pull-request

# Run the exact workflow body so the test covers its YAML wiring as well as the
# git history. The nix stub observes which base reaches the clone CLI boundary.
workflow_step=$(
  yq '.jobs.flake-build.steps[] | select(.name == "Reject duplication on changed lines").run' "$workflow"
)
if [[ -z "$workflow_step" || "$workflow_step" == "null" ]]; then
  printf 'clone diff workflow step is missing\n' >&2
  exit 1
fi

nix() {
  if [[ "$#" -ne 6 || "$1" != "run" || "$2" != ".#clone" || "$3" != "--" || "$4" != "." || "$5" != "--diff" ]]; then
    printf 'unexpected clone invocation:' >&2
    printf ' %q' "$@" >&2
    printf '\n' >&2
    return 1
  fi
  if [[ "$6" != "$EXPECTED_BASE_SHA" ]]; then
    printf 'expected clone base %s, got %s\n' "$EXPECTED_BASE_SHA" "$6" >&2
    return 1
  fi
}
export -f nix

(
  cd "$repo"
  export EVENT_BASE_SHA="$event_base"
  export EXPECTED_BASE_SHA="$checkout_base"
  export GITHUB_EVENT_NAME=pull_request
  bash -c "$workflow_step"
)

printf 'clone workflow used checked-out merge parent %s\n' "$checkout_base"
