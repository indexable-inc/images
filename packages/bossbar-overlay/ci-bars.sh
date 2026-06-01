# Body of the `ci-bars` bash app (see ci-bars-home-module.nix).
# No shebang / `set` line: the mkBashApp wrapper supplies bash + `set -euo
# pipefail` and bakes gh/jq/sqlite/coreutils/perl + the bossbar CLI onto PATH via
# runtimeInputs. Everything tunable comes from the environment so the script is a
# plain, testable file with no build-time string baking:
#   CI_BARS_REPOS        space-separated `owner/name` repos to watch (required)
#   CI_BARS_AVG_TTL      seconds to cache a workflow's average duration (3600)
#   CI_BARS_DEFAULT_AVG  fallback average when a workflow has no green history (300)
#   CI_BARS_MAX          max bars per repo per poll (12)
#   CI_BARS_STATE        state dir for the average cache + lock ($HOME/.cache/ci-bars)
#
# Draws a Minecraft boss bar PER in-flight GitHub Actions run across the watched
# repos, so a glance says what CI is doing right now. This is the live-progress
# companion to the [[ix-downtime]] outage bars (red/yellow/blue) and the
# [[pr-watch]] karma feed (merge orb / failure villager): those react to discrete
# events, this one reconciles a continuous "what's running" view. It is SILENT,
# because pr-watch already owns the success/failure sound; here the bar fill is
# the whole signal.
#
# Color is deliberately OUTSIDE the downtime palette so the two never read as the
# same thing:
#   - in_progress -> green, progress = elapsed / historical-average duration;
#   - queued/waiting -> purple, a thin bar (no runner yet, so no elapsed clock).
#
# Progress for a running job is estimated from the mean wall-clock of the last
# few SUCCESSFUL runs of that same workflow (cached in SQLite, refreshed at most
# once per CI_BARS_AVG_TTL), clamped to [0.02, 0.97] so a bar never shows empty
# and never shows full until the run actually finishes. The overlay also ticks a
# live elapsed timer in the title from the bar's `since` (set to the run's start),
# so the human sees both "about this far" and "for this long".
#
# A bar's identity is the run URL (unique per run), so CI bars never collide with
# the downtime bars (which point at the status page) and a run heals in place
# across polls. When a run leaves the in-flight set (it completed, was cancelled,
# or fell off the list) its bar is removed on the next poll: we enumerate every
# overlay row whose url is an Actions run and drop any not in the current active
# set. pr-watch handles the completion sound, so this just clears the visual.

state_dir="${CI_BARS_STATE:-$HOME/.cache/ci-bars}"
mkdir -p "$state_dir"
db="$state_dir/state.db" # historical per-workflow average cache (not the overlay DB)
avg_ttl="${CI_BARS_AVG_TTL:-3600}"
default_avg="${CI_BARS_DEFAULT_AVG:-300}"
# `:-` only defaults unset/empty, so an explicit CI_BARS_DEFAULT_AVG=0 would slip
# through and later divide-by-zero (set -e aborts the watcher). Clamp to a sane
# floor so the standalone/env-driven path can't be wedged by a 0.
[ "${default_avg:-0}" -gt 0 ] 2>/dev/null || default_avg=300
max_bars="${CI_BARS_MAX:-12}"

# Non-overlap guard, intrinsic to the watcher so the portable service can fire it
# on a fixed interval with no external lock wrapper. Take a NON-BLOCKING
# exclusive flock on fd 9; if a previous fire still holds it, skip this one
# silently. bash keeps fd 9 open for the whole run, so the lock is held until
# exit/crash (the kernel drops it the instant the holder dies). perl is always
# present on macOS (/usr/bin/perl) and on Linux; there is no flock(1) on macOS.
exec 9>"$state_dir/poll.lock"
perl -e 'use Fcntl ":flock"; flock(STDIN, LOCK_EX | LOCK_NB) or exit 1' <&9 || exit 0

# Repos to watch come from the environment (the module sets CI_BARS_REPOS from its
# `repos` option). `owner/name` slugs never contain spaces, so a plain word-split
# is safe and keeps the script free of build-time baking.
read -ra repos <<<"${CI_BARS_REPOS:-}"
if [ "${#repos[@]}" -eq 0 ]; then
  echo "ci-bars: no repos configured (set CI_BARS_REPOS); nothing to do"
  exit 0
fi

now="$(date +%s)"

# Double single quotes so a value carrying an apostrophe (a branch or workflow
# name can) cannot break out of a SQL string literal in the read-only lookups.
sq() { printf "%s" "${1//\'/\'\'}"; }

# The overlay DB the `bossbar` CLI writes; we read it directly (read-only
# SELECTs) to find a bar by url and to enumerate stale CI bars, but every write
# still goes through the CLI, which owns the schema.
bdb="$(bossbar db 2>/dev/null || true)"
# `bossbar db` computes the path even when the file does not exist yet, so an
# empty result means the `bossbar` binary itself is missing/broken. Bail rather
# than fall through: without the DB path we cannot match a run to its existing
# bar, so `add` would re-INSERT a duplicate row every poll (a bar storm).
if [ -z "$bdb" ]; then
  echo "ci-bars: cannot resolve the overlay DB (bossbar db); nothing to do"
  exit 0
fi

bar_id_for_url() {
  sqlite3 "$bdb" "SELECT id FROM bossbars WHERE url = '$(sq "$1")' ORDER BY id LIMIT 1;" 2>/dev/null || printf ''
}

# ISO-8601 (e.g. 2026-06-01T22:10:05Z) -> epoch. coreutils' GNU `date` is on
# PATH ahead of macOS's BSD date via runtimeInputs, so `-d` works on both; keep
# the BSD form as a belt-and-braces fallback in case PATH ordering ever shifts.
iso_to_epoch() {
  local s="${1:-}"
  [ -n "$s" ] || { printf '0'; return; }
  date -u -d "$s" +%s 2>/dev/null && return
  date -u -j -f "%Y-%m-%dT%H:%M:%SZ" "$s" +%s 2>/dev/null || printf '0'
}

# Average wall-clock of recent successful runs of one workflow, cached so we do
# not re-derive it every poll. Falls back to $default_avg when the workflow has
# no green history yet (a brand-new or always-red workflow), so a bar still
# advances at a sane rate instead of pinning to the floor.
sqlite3 "$db" >/dev/null <<'SQL'
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS wf_avg(
  repo       TEXT NOT NULL,
  wf         TEXT NOT NULL,
  avg_secs   INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (repo, wf)
);
SQL

get_avg() {
  local repo="$1" wf="$2" cached upd rows st en se ee d total count avg
  cached="$(sqlite3 "$db" "SELECT avg_secs FROM wf_avg WHERE repo='$(sq "$repo")' AND wf='$(sq "$wf")';" 2>/dev/null || printf '')"
  upd="$(sqlite3 "$db" "SELECT updated_at FROM wf_avg WHERE repo='$(sq "$repo")' AND wf='$(sq "$wf")';" 2>/dev/null || printf '0')"
  upd="${upd:-0}"
  if [ -n "$cached" ] && [ "$((now - upd))" -lt "$avg_ttl" ]; then
    printf '%s' "$cached"
    return
  fi
  rows="$(gh run list --repo "$repo" --workflow "$wf" --status success \
    --json startedAt,updatedAt --limit 20 2>/dev/null \
    | jq -r '.[] | [.startedAt, .updatedAt] | @tsv')" || rows=""
  total=0
  count=0
  while IFS=$'\t' read -r st en; do
    [ -n "$st" ] && [ -n "$en" ] || continue
    se="$(iso_to_epoch "$st")"
    ee="$(iso_to_epoch "$en")"
    d=$((ee - se))
    [ "$d" -gt 0 ] || continue
    total=$((total + d))
    count=$((count + 1))
  done <<EOF
$rows
EOF
  if [ "$count" -gt 0 ]; then
    avg=$((total / count))
  else
    avg="$default_avg"
  fi
  sqlite3 "$db" "INSERT INTO wf_avg(repo,wf,avg_secs,updated_at) VALUES('$(sq "$repo")','$(sq "$wf")',$avg,$now) ON CONFLICT(repo,wf) DO UPDATE SET avg_secs=excluded.avg_secs, updated_at=excluded.updated_at;" 2>/dev/null || true
  printf '%s' "$avg"
}

# Active run urls seen this poll (newline-separated), used to prune finished bars.
active_urls=""

for repo in "${repos[@]}"; do
  short="${repo##*/}"
  # One list call per repo per poll: every recent run, all branches, all
  # workflows. Non-completed runs are filtered client-side and capped to the most
  # recent $max_bars so a busy moment cannot flood the screen with bars.
  runs="$(gh run list --repo "$repo" --limit 100 \
    --json databaseId,workflowName,displayTitle,headBranch,status,startedAt,createdAt,url \
    2>/dev/null)" || continue
  [ -n "$runs" ] || continue

  while IFS=$'\t' read -r run_id wf title branch status started created url; do
    [ -n "${run_id:-}" ] || continue
    [ -n "$url" ] || url="https://github.com/$repo/actions/runs/$run_id"
    active_urls="$active_urls
$url"

    # Title carries the bitmap (ASCII-only) font, so stick to ASCII separators.
    bar_title="$short: $wf ($branch)"

    startsec=0
    case "$status" in
      queued | requested | waiting | pending)
        color="purple"
        prog="0.02"
        desc="Queued on GitHub Actions, waiting for a runner.

$title"
        ;;
      *) # in_progress (and any other non-terminal state)
        color="green"
        startsec="$(iso_to_epoch "${started:-}")"
        [ "${startsec:-0}" -gt 0 ] 2>/dev/null || startsec="$(iso_to_epoch "${created:-}")"
        avg="$(get_avg "$repo" "$wf")"
        [ "${avg:-0}" -gt 0 ] 2>/dev/null || avg="$default_avg"
        elapsed=$((now - startsec))
        [ "$elapsed" -ge 0 ] || elapsed=0
        # Integer math to avoid an awk/gawk dependency: fill in thousandths,
        # clamped to [0.020, 0.970]. prog_milli is always < 1000, so the "0."
        # prefix is correct.
        prog_milli=$((elapsed * 1000 / avg))
        [ "$prog_milli" -lt 20 ] && prog_milli=20
        [ "$prog_milli" -gt 970 ] && prog_milli=970
        prog="$(printf '0.%03d' "$prog_milli")"
        desc="Running on GitHub Actions.

Progress is estimated from the average of recent successful runs of this workflow. The title shows elapsed time; it clears when the run finishes."
        ;;
    esac

    id="$(bar_id_for_url "$url")"
    if [ -z "$id" ]; then
      if [ "${startsec:-0}" -gt 0 ] 2>/dev/null; then
        bossbar add "$bar_title" --color "$color" --overlay progress --progress "$prog" --position -1 --since "$startsec" --url "$url" --description "$desc" 2>/dev/null || true
      else
        bossbar add "$bar_title" --color "$color" --overlay progress --progress "$prog" --position -1 --url "$url" --description "$desc" 2>/dev/null || true
      fi
    else
      # Existing bar: refresh fill/color/title in place. A run first seen while
      # QUEUED was added with no `since` (purple, no clock); once it starts
      # running we must stamp `since` so the live elapsed timer begins. Only stamp
      # when the bar has none yet, so an already-ticking clock survives polls and
      # restarts (mirrors ix-downtime.sh's heal-in-place rule).
      cur_since="$(sqlite3 "$bdb" "SELECT COALESCE(since,0) FROM bossbars WHERE id = $id;" 2>/dev/null || printf '0')"
      if [ "${startsec:-0}" -gt 0 ] 2>/dev/null && [ "${cur_since:-0}" -le 0 ] 2>/dev/null; then
        bossbar set "$id" --title "$bar_title" --color "$color" --progress "$prog" --since "$startsec" --description "$desc" 2>/dev/null || true
      else
        bossbar set "$id" --title "$bar_title" --color "$color" --progress "$prog" --description "$desc" 2>/dev/null || true
      fi
    fi
  done < <(printf '%s' "$runs" | jq -r --argjson max "$max_bars" '
    [ .[] | select((.status // "completed") != "completed") ]
    | sort_by(.createdAt) | reverse | .[0:$max]
    | .[]
    | [ (.databaseId|tostring), .workflowName, .displayTitle, .headBranch,
        .status, (.startedAt // ""), (.createdAt // ""), (.url // "") ] | @tsv')
done

# Prune bars for runs no longer in flight: any overlay row whose url is an Actions
# run but is not in this poll's active set has completed (or was cancelled), so
# drop it. Scoped to /actions/runs/ urls so downtime bars (status-page url) and
# any hand-added bars are never touched.
if [ -n "$bdb" ]; then
  existing="$(sqlite3 -noheader -separator "	" "$bdb" \
    "SELECT id, url FROM bossbars WHERE url LIKE '%/actions/runs/%';" 2>/dev/null || true)"
  while IFS=$'\t' read -r eid eurl; do
    [ -n "${eid:-}" ] || continue
    if ! printf '%s\n' "$active_urls" | grep -qxF "$eurl"; then
      bossbar rm "$eid" 2>/dev/null || true
    fi
  done <<EOF
$existing
EOF
fi
