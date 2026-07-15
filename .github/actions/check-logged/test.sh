#!/usr/bin/env bash
set -euo pipefail

action_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "${scratch}"' EXIT

bin_dir="${scratch}/bin"
commit="3365b8ad742ed57b7db3b06d8b62e1aeb3c0bad5"
workflow="Closure gate"
run_id="29414110217"
run_attempt="1"
runner_identity="ix-ci-job-index-29414110217-87347897797"
log_root="${scratch}/logs"
check_log="${log_root}/${commit}/${workflow}/${run_id}-${run_attempt}.stdout"
summary="${scratch}/summary"
stdout="${scratch}/stdout"
stderr="${scratch}/stderr"
bash_bin="$(command -v bash)"
env_bin="$(command -v env)"

mkdir -p "${bin_dir}" "$(dirname -- "${check_log}")"
ln -s "$(command -v cat)" "${bin_dir}/cat"
ln -s "$(command -v install)" "${bin_dir}/install"

printf '#!%s\n' "${bash_bin}" >"${bin_dir}/git"
cat >>"${bin_dir}/git" <<EOF
printf '%s\\n' '${commit}'
EOF

printf '#!%s\n' "${bash_bin}" >"${bin_dir}/nix"
cat >>"${bin_dir}/nix" <<'EOF'
case "$1" in
  --version)
    printf 'nix (Nix) test\n'
    ;;
  store)
    printf '{"trusted":false}\n'
    ;;
  run)
    printf 'check passed\n'
    ;;
  *)
    exit 64
    ;;
esac
EOF
chmod +x "${bin_dir}/git" "${bin_dir}/nix"

"${env_bin}" -i \
  PATH="${bin_dir}" \
  CHECK_SUBCOMMAND=closure \
  GITHUB_RUN_ATTEMPT="${run_attempt}" \
  GITHUB_RUN_ID="${run_id}" \
  GITHUB_STEP_SUMMARY="${summary}" \
  GITHUB_WORKFLOW="${workflow}" \
  IX_CI_RUN_LOG_ROOT="${log_root}" \
  RUNNER_IDENTITY="${runner_identity}" \
  RUNNER_TEMP="${scratch}/runner-temp" \
  "${bash_bin}" "${action_dir}/run.sh" >"${stdout}" 2>"${stderr}"

if [[ -s "${stderr}" ]]; then
  printf 'unexpected stderr:\n%s\n' "$(<"${stderr}")" >&2
  exit 1
fi
if [[ "$(<"${stdout}")" != *"index check log: ${check_log} on ${runner_identity}"* ]]; then
  printf 'stdout omits the log path or runner identity:\n%s\n' "$(<"${stdout}")" >&2
  exit 1
fi
if [[ "$(<"${summary}")" != *"log: \`${check_log}\` on \`${runner_identity}\`"* ]]; then
  printf 'step summary omits the log path or runner identity:\n%s\n' "$(<"${summary}")" >&2
  exit 1
fi
if [[ "$(<"${check_log}")" != "check passed" ]]; then
  printf 'check output was not written to %s\n' "${check_log}" >&2
  exit 1
fi
