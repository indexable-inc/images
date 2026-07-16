#!/usr/bin/env bash
set -euo pipefail

workflow=${1:?usage: update-rust-nightly-test.sh WORKFLOW}
model=$(mktemp)
mutated=$(mktemp)
trap 'rm -f "$model" "$mutated"' EXIT
yq -o=json '.' "$workflow" >"$model"

prepare_runner="\${{ format('ix-ci-run-{0}-{1}-update-rust-nightly-prepare', github.run_id, github.run_attempt) }}"
publish_runner="\${{ format('ix-ci-run-{0}-{1}-update-rust-nightly-publish', github.run_id, github.run_attempt) }}"
validate_model() {
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
    and (($publish | tostring) | contains("withoutPin"))
    and (($publish | tostring) | contains("withoutChannel"))
    and (($publish | tostring) | contains("normalize"))
    and (($prepare | tostring) | contains("AUTOBUMP_TOKEN") | not)
    and (($publish | tostring) | contains("AUTOBUMP_TOKEN"))
    and (($jobs | tostring) | contains("ubuntu-latest") | not)
' "$1" >/dev/null
}

validate_model "$model"

reject_mutation() {
  local name=$1 filter=$2
  jq "$filter" "$model" >"$mutated"
  if validate_model "$mutated"; then
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
reject_mutation publish-evaluation \
  '.jobs["publish-update"].steps += [{"run":"nix run .#artifact-code"}]'
reject_mutation broad-pr-staging \
  '(.jobs["publish-update"].steps[] | select((.uses? // "") | startswith("peter-evans/create-pull-request@")) | .with["add-paths"]) = "."'
