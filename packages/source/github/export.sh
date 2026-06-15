#!/usr/bin/env bash
# Produce a GitHub export directory for the `source-github` search adapter.
#
# Usage:
#   export.sh OUTPUT_DIR OWNER/REPO [OWNER/REPO ...]
#
# Writes OUTPUT_DIR/metadata.json (provenance + repos covered),
# OUTPUT_DIR/items.json (a single combined array of issues and pull requests,
# each tagged with its repo and kind), and OUTPUT_DIR/ci_runs.json (completed
# workflow runs from the last CI_WINDOW_DAYS days, default 90, whose conclusion
# is failure/timed_out/cancelled, each carrying its failed jobs and their failed
# step names). Pull requests carry their reviews and inline review threads
# nested in place, and CI runs their failed jobs, so the Rust adapter does no
# joins.
#
# Requires: gh (authenticated), jq.
#
# Note: by default the indexer pass uploads and updates only; an item that
# disappears from a later export (or a repo dropped from the repo list) keeps
# its last-exported version searchable. Run the indexer with --gc to also
# delete `github` records that vanished from the export just indexed.
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
ci_file="$out_dir/ci_runs.json"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# `gh` issue/PR list caps `--limit`; pick a ceiling well above any single repo.
limit=100000

# The CI pass is windowed, not full-history: failed runs lose diagnostic value
# fast (the flaky test gets fixed, the branch moves on), and an unbounded
# `actions/runs` walk over a busy repo is thousands of pages.
ci_window_days=${CI_WINDOW_DAYS:-90}
ci_since=$(date -u -d "$ci_window_days days ago" +%Y-%m-%d)

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
# Per-run failed jobs. Unlike the collections above, each `actions/runs/<id>/jobs`
# page is an object wrapping a `jobs` array, so the pages are flattened through
# `.jobs[]` rather than `add`. Keep only failing jobs, and inside each only the
# failing steps' names: the green ones carry no diagnostic signal.
cat > "$work/ci_jobs.jq" <<'JQ'
def failing: .conclusion == "failure" or .conclusion == "timed_out" or .conclusion == "cancelled";
{ ($n): ([ .[].jobs[] ] | map(select(failing)) | map({
    name,
    conclusion,
    url: .html_url,
    failed_steps: [ (.steps // [])[] | select(failing) | .name ]
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

# Emit one repo's failed CI runs as a JSON array on stdout: completed workflow
# runs in the window whose conclusion is failure/timed_out/cancelled, each with
# its failed jobs joined in place. A repo where listing runs fails (Actions
# disabled returns 404) contributes an empty array rather than aborting the
# whole export: CI data is an enrichment, not the export's reason to exist.
emit_repo_ci() {
  local repo=$1
  local runs_raw="$work/ci-runs.json"
  local run_ids="$work/ci-run-ids.json"
  local conclusion pages

  # One listing pass per conclusion: the `status` query parameter accepts
  # conclusions, so the server does the filtering and a busy repo's thousands
  # of green runs are never paginated through.
  : > "$work/ci-runs-pages.json"
  for conclusion in failure timed_out cancelled; do
    if ! pages=$(gh api --paginate --slurp \
        "repos/$repo/actions/runs?status=$conclusion&created=%3E%3D$ci_since&per_page=100"); then
      echo "warning: listing $conclusion workflow runs for $repo failed; emitting no CI runs" >&2
      echo '[]'
      return
    fi
    printf '%s\n' "$pages" >> "$work/ci-runs-pages.json"
  done
  jq -s --arg repo "$repo" '[ .[][].workflow_runs[]
      | {
          repo: $repo,
          run_id: .id,
          run_number,
          workflow: .name,
          branch: .head_branch,
          head_sha,
          conclusion,
          event,
          created_at, updated_at,
          url: .html_url
        } ]' "$work/ci-runs-pages.json" > "$runs_raw"

  # `fetch_per_item` keys on `.number`, so present each run id as one; the
  # endpoint composes to repos/<owner>/<repo>/actions/runs/<id>/jobs.
  jq '[ .[] | { number: .run_id } ]' "$runs_raw" > "$run_ids"
  fetch_per_item "$repo" "$run_ids" "actions/runs" jobs "$work/ci_jobs.jq" "$work/ci-jobs.json"

  jq --slurpfile jobs "$work/ci-jobs.json" '
    [ .[] | .failed_jobs = ($jobs[0][(.run_id | tostring)] // []) ]' "$runs_raw"
}

# Concatenate every repo's items (and failed CI runs) into combined arrays.
combined="$work/combined.json"
combined_ci="$work/combined-ci.json"
echo '[]' > "$combined"
echo '[]' > "$combined_ci"
for repo in "${repos[@]}"; do
  echo "exporting $repo" >&2
  emit_repo "$repo" | jq -s --slurpfile acc "$combined" '$acc[0] + .[0]' > "$combined.tmp"
  mv "$combined.tmp" "$combined"

  echo "exporting $repo CI failures since $ci_since" >&2
  emit_repo_ci "$repo" | jq -s --slurpfile acc "$combined_ci" '$acc[0] + .[0]' > "$combined_ci.tmp"
  mv "$combined_ci.tmp" "$combined_ci"
done
mv "$combined" "$items_file"
mv "$combined_ci" "$ci_file"

# Provenance.
jq -n --argjson repos "$(printf '%s\n' "${repos[@]}" | jq -R . | jq -s .)" \
   --arg ci_since "$ci_since" \
  '{ exported_at: (now | todate), repos: $repos, ci_since: $ci_since }' > "$out_dir/metadata.json"

echo "wrote $items_file, $ci_file, and $out_dir/metadata.json" >&2
