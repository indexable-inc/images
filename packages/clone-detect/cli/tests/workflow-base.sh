#!/usr/bin/env bash
set -euo pipefail

workflow=${1:?usage: workflow-base.sh CHECK_WORKFLOW GATE_SCRIPT}
gate=${2:?usage: workflow-base.sh CHECK_WORKFLOW GATE_SCRIPT}
# The gate runs from inside the synthetic repo, so anchor its path first.
gate=$(realpath "$gate")
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

# The clone gate moved from an inline workflow step into the gate script
# (a84ffdf1, #3392; regression #3445 when this test kept grepping for the old
# step). The step name is presentation; the `with.script` wiring is the
# contract, so a renamed step still passes and an unwired script still fails.
wired=$(yq '[.jobs.flake-check.steps[] | .with.script // ""] | contains([".github/scripts/run-required-gate.sh"])' "$workflow")
if [[ "$wired" != "true" ]]; then
  printf 'check.yml does not hand the required gate to run-required-gate.sh\n' >&2
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
  # The gate script tail-execs the repository's quiet-log wrapper; the clone
  # base contract under test ends at that handoff, so plant a stub that only
  # records it was reached. run-required-gate-test.sh covers the wrapper's own
  # invocation contract with the real run.sh.
  mkdir -p .github/actions/check-logged
  printf 'touch "$TMPDIR/handoff"\n' >.github/actions/check-logged/run.sh
  export EVENT_BASE_SHA="$event_base"
  export EXPECTED_BASE_SHA="$checkout_base"
  export GITHUB_EVENT_NAME=pull_request
  export RUNNER_NAME=workflow-base-test
  bash "$gate"
)
if [[ ! -f "$TMPDIR/handoff" ]]; then
  printf 'gate never reached the check handoff\n' >&2
  exit 1
fi

printf 'clone gate used checked-out merge parent %s\n' "$checkout_base"
