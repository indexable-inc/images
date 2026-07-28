#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
scratch="$(mktemp -d)"
trap 'rm -rf "${scratch}"' EXIT

bin_dir="${scratch}/bin"
calls="${scratch}/calls"
pr_base=1111111111111111111111111111111111111111
push_base=3333333333333333333333333333333333333333
mkdir -p "${bin_dir}" "${scratch}/runner-temp"

bash_bin="$(command -v bash)"
env_bin="$(command -v env)"
for command in cat install mkdir; do
  ln -s "$(command -v "${command}")" "${bin_dir}/${command}"
done
ln -s "${bash_bin}" "${bin_dir}/bash"

cat >"${bin_dir}/git" <<EOF
#!${bash_bin}
set -euo pipefail
case "\$*" in
  "rev-parse --verify HEAD^1") printf '%s\\n' '${pr_base}' ;;
  "rev-parse HEAD") printf '%s\\n' '2222222222222222222222222222222222222222' ;;
  *) printf 'unexpected git arguments: %s\\n' "\$*" >&2; exit 64 ;;
esac
EOF

cat >"${bin_dir}/nix" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  --version)
    printf 'nix (Nix) test\n'
    ;;
  store)
    [[ "$*" == "store info --json" ]]
    printf '{"trusted":false}\n'
    ;;
  run)
    if [[ "${2:-}" == ".#clone" ]]; then
      [[ "$*" == "run .#clone -- . --diff ${EXPECTED_BASE_SHA:?}" ]]
      printf 'clone\n' >>"${CALLS:?}"
    elif [[ "${2:-}" == ".#check" ]]; then
      [[ "$*" == "run .#check -- required" ]]
      printf 'check\n' >>"${CALLS:?}"
      printf 'required gate passed\n'
    else
      printf 'unexpected nix arguments: %s\n' "$*" >&2
      exit 65
    fi
    ;;
  *)
    printf 'unexpected nix command: %s\n' "$*" >&2
    exit 66
    ;;
esac
EOF
chmod +x "${bin_dir}/git" "${bin_dir}/nix"

run_gate() {
  local event_name=$1 expected_base=$2 expected_calls=$3
  shift 3
  : >"${calls}"
  (
    cd "${repo_root}" &&
      "${env_bin}" -i \
        PATH="${bin_dir}" \
        CALLS="${calls}" \
        EXPECTED_BASE_SHA="${expected_base}" \
        GITHUB_EVENT_NAME="${event_name}" \
        GITHUB_RUN_ATTEMPT=1 \
        GITHUB_RUN_ID=1234 \
        GITHUB_WORKFLOW=Check \
        IX_CI_RUN_LOG_ROOT="${scratch}/logs" \
        RUNNER_NAME=ix-ci-test-runner \
        RUNNER_TEMP="${scratch}/runner-temp" \
        "$@" \
        "${bash_bin}" "${repo_root}/.github/scripts/run-required-gate.sh"
  )
  [[ "$(<"${calls}")" == "${expected_calls}" ]]
}

run_gate pull_request "${pr_base}" $'clone\ncheck'
run_gate push "${push_base}" $'clone\ncheck' EVENT_BASE_SHA="${push_base}"

# Traced path (#4031): with NET_TRACE set (check.yml's bootstrap step), the
# wrapper must see both gate phases, preserve the underlying call order, and
# the EXIT trap must leave a summary in the working directory.
cat >"${bin_dir}/net-trace" <<EOF
#!${bash_bin}
set -euo pipefail
mode="\${1:?net-trace stub needs a subcommand}"
shift
if [[ "\${mode}" == run ]]; then
  while [[ "\$1" != -- ]]; do
    # The real tracer creates --dir; the EXIT trap's render only fires when
    # that directory exists.
    if [[ "\$1" == --dir ]]; then mkdir -p "\$2"; fi
    shift
  done
  shift
  printf 'traced\n' >>"\${CALLS:?}"
  exec "\$@"
fi
[[ "\${mode}" == render ]]
printf '{"phases":[]}\n'
EOF
chmod +x "${bin_dir}/net-trace"

run_gate pull_request "${pr_base}" $'traced\nclone\ntraced\ncheck' NET_TRACE="${bin_dir}/net-trace"
# The EXIT trap writes the summary into the working directory (the CI
# workspace); here that is the repo root, so assert and clean it up.
[[ "$(<"${repo_root}/net-trace-summary.json")" == '{"phases":[]}' ]]
rm -f "${repo_root}/net-trace-summary.json"

if "${env_bin}" -i \
  PATH="${bin_dir}" \
  GITHUB_EVENT_NAME=push \
  RUNNER_NAME=ix-ci-test-runner \
  "${bash_bin}" "${repo_root}/.github/scripts/run-required-gate.sh" \
  >"${scratch}/missing-base.out" 2>"${scratch}/missing-base.err"; then
  echo "required gate accepted a push without EVENT_BASE_SHA" >&2
  exit 1
fi
grep -q 'EVENT_BASE_SHA is required' "${scratch}/missing-base.err"
