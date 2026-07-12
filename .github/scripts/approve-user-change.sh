#!/usr/bin/env bash
set -euo pipefail

approval_tag="[approve-user-change:v1]"
approval_body="${approval_tag} Every changed path belongs to the pull request author’s registered users directory."

valid_login() {
  [[ "$1" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,37}[A-Za-z0-9])?$ ]]
}

changes_belong_to_user() {
  local login="$1"
  local prefix="users/${login}/"

  jq -e --arg prefix "$prefix" '
    [ .[][] ] as $files
    | ($files | length) > 0
      and all(
        $files[];
        (.filename | startswith($prefix))
          and (
            (has("previous_filename") | not)
              or (.previous_filename | startswith($prefix))
          )
      )
  ' >/dev/null
}

api_pages() {
  local endpoint="$1"
  gh api --method GET --paginate --slurp -f per_page=100 "$endpoint"
}

dismiss_review() {
  local reviews_endpoint="$1"
  local review_id="$2"
  gh api --method PUT \
    "${reviews_endpoint}/${review_id}/dismissals" \
    -f message="Revalidating the current pull request commit." \
    -f event=DISMISS \
    >/dev/null
}

main() {
  : "${GH_TOKEN:?GH_TOKEN is required}"
  : "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
  : "${GITHUB_WORKSPACE:?GITHUB_WORKSPACE is required}"
  : "${PR_AUTHOR:?PR_AUTHOR is required}"
  : "${PR_AUTHOR_ID:?PR_AUTHOR_ID is required}"
  : "${PR_NUMBER:?PR_NUMBER is required}"
  : "${EXPECTED_BASE_SHA:?EXPECTED_BASE_SHA is required}"
  : "${EXPECTED_HEAD_SHA:?EXPECTED_HEAD_SHA is required}"

  if ! valid_login "$PR_AUTHOR"; then
    echo "Refusing invalid GitHub login: $PR_AUTHOR" >&2
    exit 1
  fi

  local login="${PR_AUTHOR,,}"
  local registered_id
  registered_id="$(jq -er --arg login "$login" '.[$login]' "${GITHUB_WORKSPACE}/.github/user-owners.json")" || {
    echo "No immutable owner registered for users/${login}; not approving."
    exit 0
  }
  if [ "$registered_id" != "$PR_AUTHOR_ID" ]; then
    echo "GitHub user id ${PR_AUTHOR_ID} does not own users/${login}; not approving."
    exit 0
  fi

  local pr_endpoint="repos/${GITHUB_REPOSITORY}/pulls/${PR_NUMBER}"
  local reviews_endpoint="${pr_endpoint}/reviews"
  local pr
  pr="$(gh api "$pr_endpoint")"

  jq -e \
    --arg author "$login" \
    --arg base_sha "$EXPECTED_BASE_SHA" \
    --arg head_sha "$EXPECTED_HEAD_SHA" \
    '(.user.login | ascii_downcase) == $author
      and .base.ref == "main"
      and .base.sha == $base_sha
      and .head.sha == $head_sha' \
    <<<"$pr" >/dev/null || {
      echo "Pull request identity, base, or head changed before validation." >&2
      exit 1
    }

  local reviews review_id
  reviews="$(api_pages "$reviews_endpoint")"
  while IFS= read -r review_id; do
    dismiss_review "$reviews_endpoint" "$review_id"
  done < <(
    jq -r --arg tag "$approval_tag" '
      .[][]
      | select(
          .user.login == "github-actions[bot]"
            and .state == "APPROVED"
            and (.body | startswith($tag))
        )
      | .id
    ' <<<"$reviews"
  )

  if [ "$(jq -r '.draft' <<<"$pr")" = true ]; then
    echo "Draft pull requests are not approved."
    exit 0
  fi

  local user_dir="${GITHUB_WORKSPACE}/users/${login}"
  if [ ! -d "$user_dir" ] || [ -L "$user_dir" ]; then
    echo "No existing trusted directory at users/${login}; not approving."
    exit 0
  fi

  local files expected_file_count actual_file_count
  files="$(api_pages "${pr_endpoint}/files")"
  expected_file_count="$(jq -r '.changed_files' <<<"$pr")"
  actual_file_count="$(jq '[.[][]] | length' <<<"$files")"
  if [ "$actual_file_count" != "$expected_file_count" ]; then
    echo "GitHub returned ${actual_file_count} of ${expected_file_count} changed files; not approving." >&2
    exit 1
  fi
  if ! changes_belong_to_user "$login" <<<"$files"; then
    echo "At least one changed or renamed path is outside users/${login}/; not approving."
    exit 0
  fi

  local current_base_sha current_head_sha
  read -r current_base_sha current_head_sha < <(gh api --jq '[.base.sha, .head.sha] | @tsv' "$pr_endpoint")
  if [ "$current_base_sha" != "$EXPECTED_BASE_SHA" ] || [ "$current_head_sha" != "$EXPECTED_HEAD_SHA" ]; then
    echo "Pull request base or head changed during validation; a newer run must decide." >&2
    exit 1
  fi

  local review review_id_after
  review="$(
    gh api --method POST "$reviews_endpoint" \
      -f event=APPROVE \
      -f commit_id="$EXPECTED_HEAD_SHA" \
      -f body="$approval_body"
  )"
  review_id_after="$(jq -r '.id' <<<"$review")"

  read -r current_base_sha current_head_sha < <(gh api --jq '[.base.sha, .head.sha] | @tsv' "$pr_endpoint")
  if [ "$current_base_sha" != "$EXPECTED_BASE_SHA" ] || [ "$current_head_sha" != "$EXPECTED_HEAD_SHA" ]; then
    dismiss_review "$reviews_endpoint" "$review_id_after"
    echo "Pull request base or head changed after approval; dismissed the stale review." >&2
    exit 1
  fi

  echo "Approved ${PR_AUTHOR}'s current commit for users/${login}/."
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
