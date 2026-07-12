#!/usr/bin/env bash
set -euo pipefail

# shellcheck source=.github/scripts/approve-user-change.sh
source "$(dirname "$0")/approve-user-change.sh"

manifest=.github/user-owners.json
jq -e '
  type == "object"
    and all(keys[]; test("^[A-Za-z0-9]([A-Za-z0-9-]{0,37}[A-Za-z0-9])?$") )
    and all(.[]; type == "number" and . > 0 and floor == .)
' "$manifest" >/dev/null

mapfile -t directories < <(find users -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort)
mapfile -t registered < <(jq -r 'keys[]' "$manifest" | sort)
if [ "${directories[*]}" != "${registered[*]}" ]; then
  echo "Every users directory must have exactly one immutable owner in $manifest." >&2
  exit 1
fi

expect_owned() {
  local login="$1"
  local files="$2"
  changes_belong_to_user "$login" <<<"$files"
}

expect_invalid_login() {
  local github_type="$1"
  local login="$2"
  if principal_kind "$login" "$github_type" >/dev/null; then
    echo "expected invalid principal: $github_type $login" >&2
    exit 1
  fi
}

expect_principal() {
  local expected_kind="$1"
  local github_type="$2"
  local login="$3"
  local actual_kind
  actual_kind="$(principal_kind "$login" "$github_type")"
  if [ "$actual_kind" != "$expected_kind" ]; then
    echo "expected $login to be a $expected_kind principal, got $actual_kind" >&2
    exit 1
  fi
}

expect_rejected() {
  local login="$1"
  local files="$2"
  if changes_belong_to_user "$login" <<<"$files"; then
    echo "expected changed files to be rejected: $files" >&2
    exit 1
  fi
}

expect_owned alice '[[{"filename":"users/alice/home.nix"}]]'
expect_owned alice '[[{"filename":"users/alice/new.nix","previous_filename":"users/alice/old.nix"}]]'
expect_owned alice '[[{"filename":"users/alice/a"}],[{"filename":"users/alice/b"}]]'

expect_rejected alice '[]'
expect_rejected alice '[[]]'
expect_rejected alice '[[{"filename":"flake.nix"}]]'
expect_rejected alice '[[{"filename":"users/alice-evil/home.nix"}]]'
expect_rejected alice '[[{"filename":"users/alice/new.nix","previous_filename":"flake.nix"}]]'
expect_rejected alice '[[{"filename":"users/alice/home.nix"},{"filename":"README.md"}]]'

expect_principal user User alice
expect_principal user User Alice-7
expect_principal app Bot 'dependabot[bot]'
expect_principal app Bot 'claude[bot]'
expect_invalid_login User --alice
expect_invalid_login User 'alice/bob'
expect_invalid_login User 'alice_'
expect_invalid_login Bot '[bot]'
expect_invalid_login Bot 'alice_[bot]'
expect_invalid_login User 'dependabot[bot]'
expect_invalid_login Bot alice

echo "approve-user-change tests passed"
