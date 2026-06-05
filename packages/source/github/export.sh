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

# jq programs for the per-item REST passes, kept in files so the regex below
# carries no extra shell-quoting layers. Each receives `--arg n` and the merged
# page array on stdin, and emits a single `{ "<n>": <projected> }` object.
#
# Author normalization matches across every surface: the REST API suffixes bot
# logins with `[bot]` (the GraphQL list calls do not), so strip it, and a deleted
# account has a null `user`, which must stay null rather than abort the `sub`.
cat > "$work/comments.jq" <<'JQ'
{ ($n): (add | map({
    author: (.user.login | if . then sub("\\[bot\\]$"; "") else null end),
    body,
    created_at
})) }
JQ
cat > "$work/threads.jq" <<'JQ'
{ ($n): (add | group_by(.in_reply_to_id // .id) | map({
    path: .[0].path,
    line: (.[0].line // .[0].original_line),
    comments: [ .[] | {
      author: (.user.login | if . then sub("\\[bot\\]$"; "") else null end),
      body,
      created_at
    } ]
})) }
JQ
# Drop PENDING reviews (a reviewer's own unsubmitted draft, visible only to that
# token): they have a null `submitted_at`, which the adapter requires as a string.
cat > "$work/reviews.jq" <<'JQ'
{ ($n): (add | map(select(.state != "PENDING")) | map({
    author: (.user.login | if . then sub("\\[bot\\]$"; "") else null end),
    body,
    state,
    submitted_at
})) }
JQ

# Fetch a fully-paginated REST collection for every item number, in parallel,
# and merge the per-item results into one number-keyed object.
#   $1 repo, $2 file holding a JSON array with `.number`, $3 endpoint
#   ("issues"|"pulls"), $4 collection ("comments"|"reviews"), $5 jq program file,
#   $6 output file.
# `gh api --paginate` walks every page, so no item is lost on busy PRs (the
# GraphQL list calls return only the first page of a nested connection).
# Each worker writes its own file rather than a shared stdout pipe: concurrent
# writes larger than PIPE_BUF would interleave and corrupt the JSON stream.
fetch_per_item() {
  local repo=$1 numbers=$2 endpoint=$3 collection=$4 prog=$5 out=$6
  local dir
  dir=$(mktemp -d "$work/per-item.XXXXXX")
  jq -r '.[].number' "$numbers" \
    | ITEM_REPO="$repo" ITEM_ENDPOINT="$endpoint" ITEM_COLLECTION="$collection" ITEM_DIR="$dir" ITEM_PROG="$prog" \
      xargs -P "${EXPORT_JOBS:-8}" -I {} bash -c '
        set -euo pipefail
        n=$1
        gh api --paginate --slurp \
          "repos/${ITEM_REPO%%/*}/${ITEM_REPO##*/}/$ITEM_ENDPOINT/$n/$ITEM_COLLECTION" \
          | jq --arg n "$n" -f "$ITEM_PROG" > "$ITEM_DIR/$n.json"
      ' _ {}
  if compgen -G "$dir/*.json" > /dev/null; then
    jq -s 'add // {}' "$dir"/*.json > "$out"
  else
    echo '{}' > "$out"
  fi
}

emit_repo() {
  local repo=$1
  local issues_raw="$work/issues.json"
  local prs_raw="$work/prs.json"

  # Issue/PR metadata only. Comments, reviews, and review threads are each fetched
  # per item from the paginated REST endpoints below: gh requests only the first
  # page of a nested connection on these list calls and never paginates it, so a
  # busy issue or PR would silently lose comments/reviews past that page.
  gh issue list --repo "$repo" --state all --limit "$limit" \
    --json number,title,body,state,author,labels,assignees,createdAt,updatedAt,closedAt,url \
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
        url
      } ]' > "$issues_raw"

  gh pr list --repo "$repo" --state all --limit "$limit" \
    --json number,title,body,state,author,labels,assignees,isDraft,baseRefName,headRefName,createdAt,updatedAt,closedAt,mergedAt,url \
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
        url
      } ]' > "$prs_raw"

  # Conversation comments for issues and PRs alike (PRs are issues for this
  # endpoint), then reviews and inline review threads for PRs only.
  local all_items="$work/all.json"
  jq -s 'add' "$issues_raw" "$prs_raw" > "$all_items"

  fetch_per_item "$repo" "$all_items" issues comments "$work/comments.jq" "$work/comments.json"
  fetch_per_item "$repo" "$prs_raw" pulls reviews "$work/reviews.jq" "$work/reviews.json"
  fetch_per_item "$repo" "$prs_raw" pulls comments "$work/threads.jq" "$work/threads.json"

  jq --slurpfile comments "$work/comments.json" \
     --slurpfile reviews "$work/reviews.json" \
     --slurpfile threads "$work/threads.json" '
    [ .[]
      | .comments = ($comments[0][(.number | tostring)] // [])
      | if .kind == "pr"
        then .reviews = ($reviews[0][(.number | tostring)] // [])
           | .review_threads = ($threads[0][(.number | tostring)] // [])
        else . end
    ]' "$all_items"
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
