#!/usr/bin/env bash
set -euo pipefail
: "${RUNNER_IDENTITY:?RUNNER_IDENTITY is required}"

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

# The gate's output goes to a runner-local file rather than to the job log
# because nix-fast-build over ~2900 attributes produces more than GitHub will
# keep. That is fine while nix exits on its own, because the failure path below
# prints it. It is not fine when something else kills the process: the shell
# dies before reaching that `cat`, and the job then contains no output from the
# gate whatsoever. 20 of 117 sampled Check failures looked like that -- 298
# seconds between the clone gate and the artifact upload, and the only red
# marker anywhere was closure-gate's mirror of the verdict.
#
# The ci-budget worker signals before it kills, so trapping recovers the
# diagnostic in that case. SIGKILL cannot be trapped, which is why check.yml
# also uploads this log as an artifact unconditionally; the two together cover
# both ways the gate can die without speaking.
dump_gate_log() {
  local signal="$1"
  printf '\n::error title=index check was killed (SIG%s), not failed::last 200 lines of the gate log follow; the whole log is the check-log artifact and %s on %s\n' \
    "${signal}" "${check_log}" "${RUNNER_IDENTITY}"
  # tail's own complaint is kept rather than silenced: "no such file" and "the
  # gate wrote nothing" are different diagnoses, and this function exists
  # precisely because a diagnosis went missing.
  if ! tail -n 200 "${check_log}"; then
    printf '(no readable gate log at %s -- killed before it wrote anything)\n' "${check_log}"
  fi
  # 128 + signal number, the convention for death by signal, so the caller can
  # tell this apart from the gate's own non-zero exit.
  case "${signal}" in
  TERM) exit 143 ;;
  INT) exit 130 ;;
  *) exit 1 ;;
  esac
}
trap 'dump_gate_log TERM' TERM
trap 'dump_gate_log INT' INT

if ! nix "${check_args[@]}" >"${check_log}" 2>&1; then
  cat "${check_log}"
  exit 1
fi
