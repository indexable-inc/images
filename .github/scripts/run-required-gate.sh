#!/usr/bin/env bash
set -euo pipefail

# The changed-line clone gate needs the checked-out synthetic merge's first
# parent for pull requests; the event base can predate that merge after main
# advances. Push and merge-group callers provide their immutable event base.
if [[ "${GITHUB_EVENT_NAME:?GITHUB_EVENT_NAME is required}" == "pull_request" ]]; then
  base_sha="$(git rev-parse --verify HEAD^1)"
else
  base_sha="${EVENT_BASE_SHA:?EVENT_BASE_SHA is required outside pull requests}"
fi

# This checkout-only check cannot live in the pure `.#check` derivation because
# that derivation intentionally has no .git directory.
nix run .#clone -- . --diff "${base_sha}" >/dev/null

# Reuse the repository-owned quiet-log wrapper for the combined required-root
# gate. The phase-clock worker owns this script's process group, so cancellation
# and validation timeout terminate both this wrapper and every Nix descendant.
export CHECK_SUBCOMMAND=required
export RUNNER_IDENTITY="${RUNNER_NAME:?RUNNER_NAME is required}"
exec bash .github/actions/check-logged/run.sh
