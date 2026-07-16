#!/usr/bin/env bash
set -euo pipefail

workflow=${1:?usage: update-rust-nightly-test.sh WORKFLOW}
model=$(mktemp)
mutated=$(mktemp)
validator_fixtures=$(mktemp -d)
trap 'rm -f "$model" "$mutated"; rm -rf "$validator_fixtures"' EXIT
yq -o=json '.' "$workflow" >"$model"

prepare_runner="\${{ format('ix-ci-run-{0}-{1}-update-rust-nightly-prepare', github.run_id, github.run_attempt) }}"
publish_runner="\${{ format('ix-ci-run-{0}-{1}-update-rust-nightly-publish', github.run_id, github.run_attempt) }}"
readonly expected_workflow_sha256=da3ca207a1d6338678940b5c292c47b78505eae8fcc20b3c567105bf5a5ad7db
validate_model() {
  local workflow_sha256
  workflow_sha256=$(
    jq -cS '.' "$1" | sha256sum | cut -d ' ' -f1
  )
  # The publish job inherits workflow-level defaults and environment. Hashing
  # the complete model keeps those credential-relevant inputs in the boundary.
  if [[ $workflow_sha256 != "$expected_workflow_sha256" ]]; then
    if [[ ${2:-} != quiet ]]; then
      printf 'workflow fingerprint changed: expected %s, observed %s\n' \
        "$expected_workflow_sha256" "$workflow_sha256" >&2
      printf 'review the complete workflow before updating expected_workflow_sha256\n' >&2
    fi
    return 1
  fi

  jq --exit-status \
    --arg prepare_runner "$prepare_runner" \
    --arg publish_runner "$publish_runner" '
  def uses($job): [ $job.steps[] | .uses? // empty ];
  def runs($job): [ $job.steps[] | .run? // empty ];
  def checkout_credentials($job):
    [ $job.steps[]
      | select((.uses? // "") | startswith("actions/checkout@"))
      | .with["persist-credentials"] ];
  def checkout_refs($job):
    [ $job.steps[]
      | select((.uses? // "") | startswith("actions/checkout@"))
      | .with.ref ];
  def pr_add_paths($job):
    [ $job.steps[]
      | select((.uses? // "") | startswith("peter-evans/create-pull-request@"))
      | .with["add-paths"]
      | split("\n")
      | map(select(length > 0)) ];

  .jobs as $jobs
  | $jobs["prepare-update"] as $prepare
  | $jobs["publish-update"] as $publish
  | (($jobs | keys) == ["prepare-update", "publish-update"])
    and ($prepare["runs-on"] == [$prepare_runner])
    and ($publish["runs-on"] == [$publish_runner])
    and ($prepare_runner != $publish_runner)
    and ($prepare.permissions == {"contents": "read"})
    and ($publish.permissions == {
      "actions": "read",
      "contents": "write",
      "pull-requests": "write"
    })
    and ($publish.needs == "prepare-update")
    and (checkout_credentials($prepare) == [false])
    and (checkout_credentials($publish) == [false])
    and (checkout_refs($prepare) == ["${{ github.sha }}"])
    and (checkout_refs($publish) == ["${{ github.sha }}"])
    and (pr_add_paths($publish) == [[
      "flake.lock",
      "rust-toolchain.toml",
      "tests/fixtures/cargo-unit-hello/unit-catalog"
    ]])
    and (uses($prepare) | all(test("@[0-9a-f]{40}$")))
    and (uses($publish) | all(test("@[0-9a-f]{40}$")))
    and (uses($prepare)
      | all((contains("install-nix") or contains("bootstrap-patched-nix")) | not))
    and (uses($publish)
      | all((contains("install-nix") or contains("bootstrap-patched-nix")) | not))
    and (runs($prepare) | any(contains("nix flake update rust-overlay")))
    and (runs($prepare) | any(contains("nix run .#bump-rust-nightly")))
    and (runs($prepare) | any(contains("nix run .#update-cargo-unit-catalog")))
    and ((runs($prepare) | join("\n")) | test("(^|[[:space:]])python3([[:space:]]|$)") | not)
    and (runs($publish)
      | all(contains("nix run") or contains("nix flake") or contains(".#") | not))
    and (($prepare | tostring) | contains("rust-nightly-candidate-${{ github.sha }}-${{ github.run_attempt }}"))
    and (($publish | tostring) | contains("rust-nightly-candidate-${{ github.sha }}-${{ github.run_attempt }}"))
    and (($prepare | tostring) | contains("AUTOBUMP_TOKEN") | not)
    and (($publish | tostring) | contains("AUTOBUMP_TOKEN"))
    and (($jobs | tostring) | contains("ubuntu-latest") | not)
' "$1" >/dev/null
}

validate_model "$model"

reject_mutation() {
  local name=$1 filter=$2
  jq "$filter" "$model" >"$mutated"
  if validate_model "$mutated" quiet; then
    printf 'workflow policy admitted mutation: %s\n' "$name" >&2
    exit 1
  fi
}

# Pin each security boundary with a negative fixture. These mutations model
# the tempting workflow edits that would silently reintroduce a hosted runner,
# shared credential/evaluation process, unpinned action, or broad PR staging.
reject_mutation hosted-runner '.jobs["prepare-update"]["runs-on"] = ["ubuntu-latest"]'
reject_mutation persisted-credentials \
  '(.jobs["publish-update"].steps[] | select((.uses? // "") | startswith("actions/checkout@")) | .with["persist-credentials"]) = true'
reject_mutation moving-action \
  '.jobs["prepare-update"].steps += [{"uses":"actions/cache@main"}]'
reject_mutation nix-installer \
  '.jobs["prepare-update"].steps += [{"uses":"example/install-nix@0000000000000000000000000000000000000000"}]'
reject_mutation publish-pinned-action \
  '.jobs["publish-update"].steps += [{"uses":"example/exfiltrate@0000000000000000000000000000000000000000"}]'
reject_mutation publish-credential-shell \
  '.jobs["publish-update"].steps += [{"env":{"TOKEN":"${{ secrets.AUTOBUMP_TOKEN || github.token }}"},"run":"curl https://example.invalid"}]'
reject_mutation inherited-publish-token \
  '.env.PUBLISH_TOKEN = "${{ secrets.AUTOBUMP_TOKEN || github.token }}"'
reject_mutation publish-evaluation \
  '.jobs["publish-update"].steps += [{"run":"nix run .#artifact-code"}]'
reject_mutation broad-pr-staging \
  '(.jobs["publish-update"].steps[] | select((.uses? // "") | startswith("peter-evans/create-pull-request@")) | .with["add-paths"]) = "."'

publisher_validator=$(jq -er '
  [.jobs["publish-update"].steps[]
    | select(.name == "Validate and install candidate data")
    | .run]
  | if length == 1 then .[0] else error("expected one publisher validator") end
' "$model")
source_root=$(cd -- "$(dirname -- "$workflow")/../.." && pwd -P)

prepare_validator_fixture() {
  local name=$1
  case_root="$validator_fixtures/$name"
  workspace="$case_root/workspace"
  runner_temp="$case_root/runner"
  candidate="$runner_temp/rust-nightly-candidate"
  mkdir -p \
    "$workspace/tests/fixtures/cargo-unit-hello" \
    "$candidate/tests/fixtures/cargo-unit-hello"
  for file in flake.lock rust-toolchain.toml; do
    install -m 0644 "$source_root/$file" "$workspace/$file"
    install -m 0644 "$source_root/$file" "$candidate/$file"
  done
  install -m 0644 "$source_root/tests/fixtures/cargo-unit-hello/unit-catalog" \
    "$workspace/tests/fixtures/cargo-unit-hello/unit-catalog"
  install -m 0644 "$source_root/tests/fixtures/cargo-unit-hello/unit-catalog" \
    "$candidate/tests/fixtures/cargo-unit-hello/unit-catalog"
  git -C "$workspace" init -q
  git -C "$workspace" add flake.lock rust-toolchain.toml \
    tests/fixtures/cargo-unit-hello/unit-catalog
  git -C "$workspace" -c user.name=fixture -c user.email=fixture.invalid \
    commit -qm base
  fixture_sha=$(git -C "$workspace" rev-parse HEAD)
}

run_publisher_validator() {
  (
    export GITHUB_SHA="$fixture_sha"
    export GITHUB_WORKSPACE="$workspace"
    export RUNNER_TEMP="$runner_temp"
    cd "$workspace"
    bash -c "$publisher_validator"
  )
}

reject_validator_fixture() {
  local name=$1 log="$case_root/validator.log"
  if run_publisher_validator >"$log" 2>&1; then
    printf 'publisher validator admitted fixture: %s\n' "$name" >&2
    sed -n '1,120p' "$log" >&2
    exit 1
  fi
}

# Exercise the exact privileged inline script as behavior, not variable names.
prepare_validator_fixture scoped-update
temporary_lock=$(mktemp "$candidate/flake.lock.XXXXXX")
jq '
  .nodes["rust-overlay"].locked.lastModified += 1
  | .nodes["rust-overlay"].locked.rev = "0000000000000000000000000000000000000001"
  | .nodes["rust-overlay"].locked.narHash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
' "$candidate/flake.lock" >"$temporary_lock"
mv -f "$temporary_lock" "$candidate/flake.lock"

toolchain_text=$(<"$candidate/rust-toolchain.toml")
if [[ ! $toolchain_text =~ nightly-([0-9]{4})-([0-9]{2})-([0-9]{2}) ]]; then
  printf 'fixture toolchain has no dated nightly channel\n' >&2
  exit 1
fi
current_channel=${BASH_REMATCH[0]}
next_channel=$(date -d "${current_channel#nightly-} + 1 day" +nightly-%F)
toolchain_text=${toolchain_text/$current_channel/$next_channel}
printf '%s\n' "$toolchain_text" >"$candidate/rust-toolchain.toml"

catalog_text=$(<"$candidate/tests/fixtures/cargo-unit-hello/unit-catalog")
if [[ ! $catalog_text =~ [0-9a-f]{16} ]]; then
  printf 'fixture catalog has no unit identity\n' >&2
  exit 1
fi
catalog_identity=${BASH_REMATCH[0]}
replacement_identity=0000000000000000
[[ $catalog_identity != "$replacement_identity" ]] || replacement_identity=1111111111111111
catalog_text=${catalog_text/$catalog_identity/$replacement_identity}
printf '%s\n' "$catalog_text" \
  >"$candidate/tests/fixtures/cargo-unit-hello/unit-catalog"
run_publisher_validator

prepare_validator_fixture structural-lock
temporary_lock=$(mktemp "$candidate/flake.lock.XXXXXX")
jq '.unexpected = true' "$candidate/flake.lock" >"$temporary_lock"
mv -f "$temporary_lock" "$candidate/flake.lock"
reject_validator_fixture structural-lock

prepare_validator_fixture invalid-revision
temporary_lock=$(mktemp "$candidate/flake.lock.XXXXXX")
jq '.nodes["rust-overlay"].locked.rev = "invalid"' \
  "$candidate/flake.lock" >"$temporary_lock"
mv -f "$temporary_lock" "$candidate/flake.lock"
reject_validator_fixture invalid-revision

prepare_validator_fixture invalid-sri
temporary_lock=$(mktemp "$candidate/flake.lock.XXXXXX")
jq '.nodes["rust-overlay"].locked.narHash = "sha256-invalid"' \
  "$candidate/flake.lock" >"$temporary_lock"
mv -f "$temporary_lock" "$candidate/flake.lock"
reject_validator_fixture invalid-sri

prepare_validator_fixture structural-catalog
catalog_text=$(<"$candidate/tests/fixtures/cargo-unit-hello/unit-catalog")
catalog_text=${catalog_text/"units = (rec {"/"units = ({"}
printf '%s\n' "$catalog_text" \
  >"$candidate/tests/fixtures/cargo-unit-hello/unit-catalog"
reject_validator_fixture structural-catalog

prepare_validator_fixture untracked-checkout
touch "$workspace/untracked"
reject_validator_fixture untracked-checkout
