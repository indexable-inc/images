#!/usr/bin/env bash
set -euo pipefail

commit="$(git rev-parse HEAD 2>/dev/null || printf 'unknown')"
workflow="${GITHUB_WORKFLOW:-local}"
run_tag="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}"
log_root="${IX_CI_RUN_LOG_ROOT:-/var/log/ix-ci/runs}"
log_dir="${log_root}/${commit}/${workflow}"
check_args=(run .#check)
if [[ -n "${CHECK_SUBCOMMAND}" ]]; then
  check_args+=(-- "${CHECK_SUBCOMMAND}")
fi
if install -d -m 0755 "${log_dir}" 2>/tmp/index-ci-log-dir.err && [[ -w "${log_dir}" ]]; then
  check_log="${log_dir}/${run_tag}.stdout"
else
  fallback_dir="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
  check_log="${fallback_dir%/}/index-check-${run_tag}.stdout"
fi

printf 'index check log: %s on %s\n' "${check_log}" "${RUNNER_IDENTITY}"
nix --version
nix store info --json
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    printf "## index check\n\n"
    printf "log: \`%s\` on \`%s\`\n" "${check_log}" "${RUNNER_IDENTITY}"
  } >>"${GITHUB_STEP_SUMMARY}"
fi

if ! nix "${check_args[@]}" >"${check_log}" 2>&1; then
  cat "${check_log}"
  exit 1
fi
