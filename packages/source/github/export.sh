#!/usr/bin/env bash
# Produce a GitHub export directory for the `source-github` search adapter.
#
# Usage:
#   export.sh OUTPUT_DIR OWNER/REPO [OWNER/REPO ...]
#
# Writes OUTPUT_DIR/metadata.json (provenance + repos covered) and
# OUTPUT_DIR/items.json (a single combined array of issues and pull requests,
# each tagged with its repo and kind). Pull requests carry their reviews and
# inline review threads nested in place, so the Rust adapter does no joins.
#
# Requires: gh (authenticated), jq.
#
# Note: the indexer pass uploads and updates only; it does not delete items that
# disappear from a later export. A closed or removed issue/PR (or a repo dropped
# from the repo list) keeps its last-exported version searchable until a separate
# garbage-collection pass runs against the `github` source.
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: export.sh OUTPUT_DIR OWNER/REPO [OWNER/REPO ...]" >&2
  exit 2
fi

out_dir=$1
shift
repos=("$@")

mkdir -p "$out_dir"
items_file="$out_dir/items.json"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# `gh` issue/PR list caps `--limit`; pick a ceiling well above any single repo.
limit=100000

emit_repo() {
  local repo=$1
  local issues_raw="$work/issues.json"
  local prs_raw="$work/prs.json"

  # Issues (gh excludes pull requests from `issue list`). Normalize to the
  # adapter's snake_case schema; flatten author/label/assignee objects to logins
  # and names.
  gh issue list --repo "$repo" --state all --limit "$limit" \
    --json number,title,body,state,author,labels,assignees,comments,createdAt,updatedAt,closedAt,url \
    | jq --arg repo "$repo" '[ .[] | {
        kind: "issue",
        repo: $repo,
        number, title,
        body: (.body // ""),
        state: (.state | ascii_downcase),
        author: (.author.login // null),
        labels: [ .labels[].name ],
        assignees: [ .assignees[].login ],
        created_at: .createdAt,
        updated_at: .updatedAt,
        closed_at: .closedAt,
        url,
        comments: [ .comments[] | { author: (.author.login // null), body, created_at: .createdAt } ]
      } ]' > "$issues_raw"

  # Pull requests, with review-level bodies inline (reviews is a list-level
  # field). Inline review threads come from a separate REST endpoint below.
  gh pr list --repo "$repo" --state all --limit "$limit" \
    --json number,title,body,state,author,labels,assignees,comments,reviews,isDraft,baseRefName,headRefName,createdAt,updatedAt,closedAt,mergedAt,url \
    | jq --arg repo "$repo" '[ .[] | {
        kind: "pr",
        repo: $repo,
        number, title,
        body: (.body // ""),
        state: (.state | ascii_downcase),
        author: (.author.login // null),
        labels: [ .labels[].name ],
        assignees: [ .assignees[].login ],
        created_at: .createdAt,
        updated_at: .updatedAt,
        closed_at: .closedAt,
        merged_at: .mergedAt,
        is_draft: .isDraft,
        base_ref: .baseRefName,
        head_ref: .headRefName,
        url,
        comments: [ .comments[] | { author: (.author.login // null), body, created_at: .createdAt } ],
        reviews: [ .reviews[] | { author: (.author.login // null), body, state, submitted_at: .submittedAt } ],
        review_threads: []
      } ]' > "$prs_raw"

  # Inline review threads: one REST call per PR. The REST endpoint has no
  # "all PRs" variant, so fetch per PR, in parallel (a repo can have hundreds of
  # PRs and a sequential loop is the export's bottleneck). `gh api --paginate`
  # merges every page of a PR's comments into one array, so no page is lost.
  # Each worker writes its own file rather than a shared stdout pipe: concurrent
  # writes larger than PIPE_BUF would interleave and corrupt the JSON stream.
  # The per-PR files are then merged sequentially into one number-keyed object.
  local threads_dir="$work/threads.d"
  mkdir -p "$threads_dir"
  jq -r '.[].number' "$prs_raw" \
    | THREAD_REPO="$repo" THREADS_DIR="$threads_dir" \
      xargs -P "${EXPORT_JOBS:-8}" -I {} bash -c '
        set -euo pipefail
        repo=$THREAD_REPO; n=$1
        gh api --paginate --slurp "repos/${repo%%/*}/${repo##*/}/pulls/$n/comments" \
          | jq --arg n "$n" "{ (\$n): (add | group_by(.in_reply_to_id // .id) | map({
              path: .[0].path,
              line: (.[0].line // .[0].original_line),
              comments: [ .[] | { author: (.user.login | sub(\"\\\\[bot\\\\]\$\"; \"\")), body, created_at } ]
            })) }" > "$THREADS_DIR/$n.json"
      ' _ {}

  local threads_file="$work/threads.json"
  if compgen -G "$threads_dir/*.json" > /dev/null; then
    jq -s "add // {}" "$threads_dir"/*.json > "$threads_file"
  else
    echo '{}' > "$threads_file"
  fi

  jq --slurpfile threads "$threads_file" \
    '[ .[] | .review_threads = ($threads[0][(.number | tostring)] // []) ]' "$prs_raw" \
    > "$prs_raw.tmp"
  mv "$prs_raw.tmp" "$prs_raw"

  jq -s 'add' "$issues_raw" "$prs_raw"
}

# Concatenate every repo's items into one combined array.
combined="$work/combined.json"
echo '[]' > "$combined"
for repo in "${repos[@]}"; do
  echo "exporting $repo" >&2
  emit_repo "$repo" | jq -s --slurpfile acc "$combined" '$acc[0] + .[0]' > "$combined.tmp"
  mv "$combined.tmp" "$combined"
done
mv "$combined" "$items_file"

# Provenance.
jq -n --argjson repos "$(printf '%s\n' "${repos[@]}" | jq -R . | jq -s .)" \
  '{ exported_at: (now | todate), repos: $repos }' > "$out_dir/metadata.json"

echo "wrote $items_file and $out_dir/metadata.json" >&2
