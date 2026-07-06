#!/usr/bin/env bash
# Validate and sync declarative GitHub repo metadata (About sidebar:
# description, homepage, topics) for every repo `.#lib.repoMetadata` covers:
# the monorepo itself (lib/repo-metadata.nix) plus one entry per package
# mirror (`mirror` attr in package.nix). Driven by
# .github/workflows/repo-metadata.yml.
#
#   repo-metadata.sh check <rendered.json>   validate the rendered config
#   repo-metadata.sh sync  <rendered.json>   PATCH it to GitHub (needs GH_TOKEN)
#
# `check` is offline and run on pull requests: it fails when any covered repo
# is missing a description or topics (the nix eval that renders the JSON
# already throws for those, so this is the second, jq-level belt), when a
# topic is not in GitHub's accepted format, or when a covered package has no
# README.md (the mirror repo's front page would be a generated stub).
#
# `sync` runs on pushes to main: for each entry it PATCHes
# description/homepage via `gh api repos/<repo>` and PUTs the topic list via
# `gh api repos/<repo>/topics`, writing only when the live values differ so a
# quiet day is a no-op in the audit log. GH_TOKEN needs repository
# Administration: write on the covered repos -- the ix-mirror-sync GitHub App
# token the workflow mints (see packages/mirror/README.md, "Permissions").

set -euo pipefail

mode="${1:?usage: repo-metadata.sh <check|sync> <rendered.json>}"
rendered="${2:?usage: repo-metadata.sh <check|sync> <rendered.json>}"

# GitHub accepts topics of 1-50 lowercase alphanumerics/hyphens, starting
# alphanumeric. Mirrors the eval-time validation in packages/registry.nix.
topic_re='^[a-z0-9][a-z0-9-]{0,49}$'

check() {
  local failures=0
  fail() {
    echo "::error::$1"
    failures=$((failures + 1))
  }

  local count
  count="$(jq 'length' "$rendered")"
  echo "repo-metadata: checking $count covered repos"

  local dupes
  dupes="$(jq -r 'group_by(.repo) | map(select(length > 1) | .[0].repo) | .[]' "$rendered")"
  [ -z "$dupes" ] || fail "duplicate repo entries: $dupes"

  local entry repo description homepage path
  while IFS= read -r entry; do
    repo="$(jq -r '.repo' <<<"$entry")"
    description="$(jq -r '.description // ""' <<<"$entry")"
    homepage="$(jq -r '.homepage // ""' <<<"$entry")"
    path="$(jq -r '.path // "."' <<<"$entry")"

    [[ "$repo" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]] \
      || fail "$repo: not an owner/name GitHub repo"
    [ -n "$description" ] \
      || fail "$repo: missing description (set mirror.description in the package.nix, or lib/repo-metadata.nix)"
    [ "$(jq '.topics | length' <<<"$entry")" -gt 0 ] \
      || fail "$repo: missing topics (set mirror.topics in the package.nix, or lib/repo-metadata.nix)"
    while IFS= read -r topic; do
      [[ "$topic" =~ $topic_re ]] \
        || fail "$repo: topic '$topic' is not 1-50 lowercase alphanumeric/hyphen characters"
    done < <(jq -r '.topics[]' <<<"$entry")
    [[ "$homepage" == https://* ]] \
      || fail "$repo: homepage '$homepage' is not an https URL"
    [ -f "$path/README.md" ] \
      || fail "$repo: no README.md under '$path' (the published repo's front page; see CONTRIBUTING.md, READMEs)"
  done < <(jq -c '.[]' "$rendered")

  if [ "$failures" -gt 0 ]; then
    echo "repo-metadata: $failures problem(s) found"
    return 1
  fi
  echo "repo-metadata: all covered repos have description, homepage, topics, and a README"
}

sync() {
  : "${GH_TOKEN:?sync needs GH_TOKEN (Administration: write on the covered repos)}"

  local entry repo description homepage live live_desc live_home live_topics want_topics
  while IFS= read -r entry; do
    repo="$(jq -r '.repo' <<<"$entry")"
    description="$(jq -r '.description' <<<"$entry")"
    homepage="$(jq -r '.homepage' <<<"$entry")"
    want_topics="$(jq -c '.topics | sort' <<<"$entry")"

    live="$(gh api "repos/$repo")"
    live_desc="$(jq -r '.description // ""' <<<"$live")"
    live_home="$(jq -r '.homepage // ""' <<<"$live")"
    live_topics="$(jq -c '.topics | sort' <<<"$live")"

    if [ "$live_desc" != "$description" ] || [ "$live_home" != "$homepage" ]; then
      echo "$repo: updating description/homepage"
      gh api "repos/$repo" -X PATCH \
        -f description="$description" \
        -f homepage="$homepage" >/dev/null
    else
      echo "$repo: description/homepage up to date"
    fi

    if [ "$live_topics" != "$want_topics" ]; then
      echo "$repo: replacing topics with $want_topics"
      jq -c '{names: .topics}' <<<"$entry" \
        | gh api "repos/$repo/topics" -X PUT --input - >/dev/null
    else
      echo "$repo: topics up to date"
    fi
  done < <(jq -c '.[]' "$rendered")
}

case "$mode" in
  check) check ;;
  sync) sync ;;
  *)
    echo "unknown mode '$mode' (expected check or sync)" >&2
    exit 2
    ;;
esac
