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

# A committed cargo-unit catalog must make the selected Rust derivation
# enumerable without building planner metadata inside the evaluator. Keep this
# as a separate lib-only lookup: the broad checks catalog intentionally still
# contains legacy IFD consumers and would make this boundary test ambiguous.
# This proves enumeration only; it does not sandbox evaluator fetches or harden
# the build daemon. Those controls belong to the coordinator's trusted profile.
catalog_drv="$(
  nix eval --raw .#lib \
    --apply 'ix: (import (ix.paths.root + "/tests/cargo-unit-catalog.nix") { inherit ix; pkgs = ix.pkgs; }).workspace.binaries.cargo-unit-hello.drvPath' \
    --option allow-import-from-derivation false \
    --option builders '' \
    --option fallback false \
    --option max-jobs 0
)"
printf 'IFD-free cargo-unit catalog selected %s\n' "${catalog_drv}"

# Reuse the repository-owned quiet-log wrapper for the combined required-root
# gate. The phase-clock worker owns this script's process group, so cancellation
# and validation timeout terminate both this wrapper and every Nix descendant.
export CHECK_SUBCOMMAND=required
export RUNNER_IDENTITY="${RUNNER_NAME:?RUNNER_NAME is required}"
exec bash .github/actions/check-logged/run.sh
