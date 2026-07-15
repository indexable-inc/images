#!/usr/bin/env bash
set -euo pipefail

if (( $# > 1 )); then
  printf 'usage: %s [closure]\n' "$0" >&2
  exit 2
fi
subcommand="${1:-}"
if [[ -n "${subcommand}" && "${subcommand}" != closure ]]; then
  printf 'unknown check subcommand: %s\n' "${subcommand}" >&2
  exit 2
fi

commit="$(git rev-parse HEAD 2>/dev/null || printf 'unknown')"
workflow="${GITHUB_WORKFLOW:-local}"
run_tag="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}"
log_root="${IX_CI_RUN_LOG_ROOT:-/var/log/ix-ci/runs}"
log_dir="${log_root}/${commit}/${workflow}"
if install -d -m 0755 "${log_dir}" 2>/tmp/index-ci-log-dir.err && [[ -w "${log_dir}" ]]; then
  check_log="${log_dir}/${run_tag}.stdout"
else
  fallback_dir="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
  check_log="${fallback_dir%/}/index-check-${run_tag}.stdout"
fi

printf 'index check log: %s on %s\n' "${check_log}" "$(hostname)"
nix --version
nix store info --json
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    printf '## index check\n\n'
    printf 'log: `%s` on `%s`\n' "${check_log}" "$(hostname)"
  } >>"${GITHUB_STEP_SUMMARY}"
fi

command=(nix run .#check)
if [[ -n "${subcommand}" ]]; then
  command+=(-- "${subcommand}")
fi
if ! "${command[@]}" >"${check_log}" 2>&1; then
  cat "${check_log}"
  exit 1
fi
