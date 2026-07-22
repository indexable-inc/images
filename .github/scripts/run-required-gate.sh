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

# Record the gate's client-side network (#4031): every eval-time fetch, gh, or
# git connection, per phase, reported as a sticky PR comment. NET_TRACE is
# built by check.yml's "Bootstrap net-trace" step, outside this script's
# validation clock so tracing never eats PR budget. Fail open: unset or empty
# means the gate simply runs untraced.
net_trace="${NET_TRACE:-}"
net_trace_dir="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/net-trace"

run_traced() {
  local label="$1"
  shift
  if [[ -n "${net_trace}" ]]; then
    "${net_trace}" run --label "${label}" --dir "${net_trace_dir}" -- "$@"
  else
    "$@"
  fi
}

# Render even when the gate fails: a red run's network profile is the one you
# most want to read. The summary lands in the workspace for the check.yml
# upload step; the Markdown goes to the step summary for humans.
net_trace_report() {
  [[ -n "${net_trace}" && -d "${net_trace_dir}" ]] || return 0
  "${net_trace}" render --dir "${net_trace_dir}" --json >net-trace-summary.json || true
  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    "${net_trace}" render --dir "${net_trace_dir}" >>"${GITHUB_STEP_SUMMARY}" || true
  fi
}
trap net_trace_report EXIT

# This checkout-only check cannot live in the pure `.#check` derivation because
# that derivation intentionally has no .git directory.
run_traced clone-gate nix run .#clone -- . --diff "${base_sha}" >/dev/null

# Reuse the repository-owned quiet-log wrapper for the combined required-root
# gate. The phase-clock worker owns this script's process group, so cancellation
# and validation timeout terminate both this wrapper and every Nix descendant.
export CHECK_SUBCOMMAND=required
export RUNNER_IDENTITY="${RUNNER_NAME:?RUNNER_NAME is required}"
# Not `exec`: the EXIT trap above must still render the network report.
run_traced required-check bash .github/actions/check-logged/run.sh
