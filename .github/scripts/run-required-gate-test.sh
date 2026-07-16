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
for command in cat install; do
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
  eval)
    [[ "$#" -eq 17 ]]
    [[ "${2:-}" == "--raw" ]]
    [[ "${3:-}" == ".#lib" ]]
    [[ "${4:-}" == "--apply" ]]
    [[ "${5:-}" == 'ix: (import (ix.paths.root + "/tests/cargo-unit-catalog.nix") { inherit ix; pkgs = ix.pkgs; }).workspace.binaries.cargo-unit-hello.drvPath' ]]
    [[ "${6:-}" == "--option" && "${7:-}" == "allow-import-from-derivation" && "${8:-}" == "false" ]]
    [[ "${9:-}" == "--option" && "${10:-}" == "builders" && "${11+x}" == "x" && "${11}" == "" ]]
    [[ "${12:-}" == "--option" && "${13:-}" == "fallback" && "${14:-}" == "false" ]]
    [[ "${15:-}" == "--option" && "${16:-}" == "max-jobs" && "${17:-}" == "0" ]]
    printf 'catalog\n' >>"${CALLS:?}"
    if [[ "${CATALOG_EVAL_FAIL:-0}" == 1 ]]; then
      printf 'catalog evaluation failed\n' >&2
      exit 67
    fi
    printf '/nix/store/test-cargo-unit-hello.drv'
    ;;
  *)
    printf 'unexpected nix command: %s\n' "$*" >&2
    exit 66
    ;;
esac
EOF
chmod +x "${bin_dir}/git" "${bin_dir}/nix"

invoke_gate() {
  local event_name=$1 expected_base=$2
  shift 2
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
}

run_gate() {
  : >"${calls}"
  invoke_gate "$@"
  [[ "$(<"${calls}")" == $'clone\ncatalog\ncheck' ]]
}

run_gate pull_request "${pr_base}"
run_gate push "${push_base}" EVENT_BASE_SHA="${push_base}"

: >"${calls}"
if invoke_gate pull_request "${pr_base}" CATALOG_EVAL_FAIL=1 \
  >"${scratch}/catalog-failure.out" 2>"${scratch}/catalog-failure.err"; then
  echo "required gate ignored a failed strict catalog evaluation" >&2
  exit 1
fi
[[ "$(<"${calls}")" == $'clone\ncatalog' ]]
grep -q 'catalog evaluation failed' "${scratch}/catalog-failure.err"

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
