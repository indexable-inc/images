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
  if valid_login "$1"; then
    echo "expected invalid login: $1" >&2
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

valid_login alice
valid_login Alice-7
expect_invalid_login --alice
expect_invalid_login 'alice/bob'
expect_invalid_login 'alice_'

echo "approve-user-change tests passed"
