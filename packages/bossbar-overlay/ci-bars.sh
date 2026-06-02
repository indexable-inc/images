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
# Draws ONE Minecraft boss bar per in-flight HEAD COMMIT across the watched repos
# (not one per workflow run), so a commit with five checks shows a single bar that
# fills as its checks complete. This is the live-progress companion to the
# [[ix-downtime]] outage bars (red/yellow/blue) and the [[pr-watch]] karma feed
# (merge orb / failure villager): those react to discrete events, this one
# reconciles a continuous "what's building" view. It is SILENT, because pr-watch
# already owns the success/failure sound; here the bar fill is the whole signal.
#
# Color is deliberately OUTSIDE the downtime palette so the two never read alike:
#   - any check running or finished -> green;
#   - all of the commit's checks still queued / unpicked -> purple, a thin bar.
#
# A commit's fill = (finished checks + summed partial progress of the running
# ones) / total checks. Each running check's partial is elapsed / the mean
# wall-clock of recent SUCCESSFUL runs of that workflow (cached in SQLite,
# refreshed at most once per CI_BARS_AVG_TTL), clamped so the bar never shows
# empty or full until the commit's CI actually finishes. The overlay ticks a live
# elapsed timer from the bar's `since` (the earliest running check's start).
#
# A bar's identity is the commit URL (https://github.com/<repo>/commit/<sha>), so
# all of a commit's checks share one bar, CI bars never collide with the downtime
# bars (status-page url), and a commit heals in place across polls. When a commit
# leaves the in-flight set (all its checks finished or were cancelled) its bar is
# removed on the next poll: we enumerate every overlay row whose url is a commit
# page and drop any not in the current active set (the overlay poofs it out).
# pr-watch handles the completion sound, so this just clears the visual. The bars
# are boxless (--box 0): many compact commit bars with no hover pop-down.

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
  local repo="$1" wf="$2" row cached upd rows st en se ee d total count avg
  # One read for both cached value and its age (tab-separated), not two.
  row="$(sqlite3 -noheader -separator "	" "$db" \
    "SELECT avg_secs, updated_at FROM wf_avg WHERE repo='$(sq "$repo")' AND wf='$(sq "$wf")';" 2>/dev/null || printf '')"
  if [ -n "$row" ]; then
    cached="${row%%	*}"
    upd="${row#*	}"
  else
    cached=""
    upd=0
  fi
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

# Commit urls with in-flight checks this poll (newline-separated), used to prune
# bars for commits whose CI has finished. Identity is the commit, so all of a
# commit's checks share ONE bar.
active_urls=""

for repo in "${repos[@]}"; do
  short="${repo##*/}"
  # One list call per repo per poll: recent runs across all branches and
  # workflows, grouped below by head commit (headSha). The window must hold all
  # of an in-flight commit's checks (completed + running) for its done/total to be
  # right; an in-flight commit's runs are recent, so 200 is comfortably enough.
  runs="$(gh run list --repo "$repo" --limit 200 \
    --json workflowName,headBranch,headSha,status,conclusion,startedAt,createdAt,url \
    2>/dev/null)" || continue
  [ -n "$runs" ] || continue

  # Aggregate every run by its head commit. One bar per commit: total checks,
  # how many have finished, and the summed partial progress of the running ones,
  # so the fill is the commit's overall CI progress rather than a single check's.
  # Reset before declaring: `declare -A` does NOT clear an already-set assoc
  # array, so a prior repo's entries would otherwise leak into this one.
  unset c_total c_done c_running c_queued c_branch c_minstart c_partmilli c_created
  declare -A c_total c_done c_running c_queued c_branch c_minstart c_partmilli c_created
  while IFS=$'\t' read -r sha branch status wf started created; do
    [ -n "$sha" ] || continue
    c_total["$sha"]=$((${c_total["$sha"]:-0} + 1))
    [ -n "${c_branch[$sha]:-}" ] || c_branch["$sha"]="$branch"
    cc="$(iso_to_epoch "${created:-}")"
    [ "${cc:-0}" -gt "${c_created[$sha]:-0}" ] 2>/dev/null && c_created["$sha"]="$cc"
    case "$status" in
      completed)
        c_done["$sha"]=$((${c_done["$sha"]:-0} + 1))
        ;;
      queued | requested | waiting | pending)
        c_queued["$sha"]=$((${c_queued["$sha"]:-0} + 1))
        ;;
      *) # in_progress (and any other non-terminal state)
        c_running["$sha"]=$((${c_running["$sha"]:-0} + 1))
        st="$(iso_to_epoch "${started:-}")"
        [ "${st:-0}" -gt 0 ] 2>/dev/null || st="$(iso_to_epoch "${created:-}")"
        avg="$(get_avg "$repo" "$wf")"
        [ "${avg:-0}" -gt 0 ] 2>/dev/null || avg="$default_avg"
        el=$((now - st))
        [ "$el" -ge 0 ] || el=0
        pm=$((el * 1000 / avg))
        [ "$pm" -lt 0 ] && pm=0
        [ "$pm" -gt 970 ] && pm=970
        c_partmilli["$sha"]=$((${c_partmilli["$sha"]:-0} + pm))
        cm="${c_minstart[$sha]:-0}"
        if [ "$cm" -eq 0 ] || { [ "${st:-0}" -gt 0 ] && [ "$st" -lt "$cm" ]; }; then
          c_minstart["$sha"]="$st"
        fi
        ;;
    esac
  done < <(printf '%s' "$runs" | jq -r '
    .[] | [ .headSha, .headBranch, .status, .workflowName,
            (.startedAt // ""), (.createdAt // "") ] | @tsv')

  # Commits that still have in-flight work (running or queued). EVERY such commit
  # is recorded in active_urls (so the prune below never removes a still-building
  # commit), but we only DRAW the newest $max_bars of them, so a busy moment
  # cannot flood the screen. A commit past the cap simply keeps whatever bar it
  # had (or none) without flapping; it gets drawn once it re-enters the top N, and
  # pruned only once its checks all finish (it leaves active_urls).
  eligible=()
  for sha in "${!c_total[@]}"; do
    if [ $((${c_running[$sha]:-0} + ${c_queued[$sha]:-0})) -gt 0 ]; then
      eligible+=("${c_created[$sha]:-0}	$sha")
      active_urls="$active_urls
https://github.com/$repo/commit/$sha"
    fi
  done
  selected=""
  [ "${#eligible[@]}" -gt 0 ] && selected="$(printf '%s\n' "${eligible[@]}" | sort -rn -k1 | head -n "$max_bars" | cut -f2)"

  while IFS= read -r sha; do
    [ -n "$sha" ] || continue
    running=${c_running[$sha]:-0}
    done_n=${c_done[$sha]:-0}
    total=${c_total[$sha]:-1}
    branch="${c_branch[$sha]:-?}"
    sha7="${sha:0:7}"
    url="https://github.com/$repo/commit/$sha"

    # Fill = (finished checks + summed partial of running ones) / total checks.
    progmilli=$(((done_n * 1000 + ${c_partmilli[$sha]:-0}) / total))
    [ "$progmilli" -lt 20 ] && progmilli=20
    [ "$progmilli" -gt 970 ] && progmilli=970
    prog="$(printf '0.%03d' "$progmilli")"

    # Green once any check is running or finished; purple while the commit's
    # checks are all still queued / not yet picked up by a runner.
    if [ "$running" -gt 0 ] || [ "$done_n" -gt 0 ]; then color="green"; else color="purple"; fi
    minstart=${c_minstart[$sha]:-0}

    # ASCII-only title (bitmap font): repo, branch, finished/total, short sha.
    bar_title="$short: $branch ($done_n/$total) $sha7"

    id="$(bar_id_for_url "$url")"
    if [ -z "$id" ]; then
      if [ "${minstart:-0}" -gt 0 ] 2>/dev/null; then
        bossbar add "$bar_title" --color "$color" --overlay progress --progress "$prog" --position -1 --since "$minstart" --url "$url" --box 0 2>/dev/null || true
      else
        bossbar add "$bar_title" --color "$color" --overlay progress --progress "$prog" --position -1 --url "$url" --box 0 2>/dev/null || true
      fi
    else
      # Heal in place: refresh fill/color/title. Stamp `since` only once (when the
      # commit's first check starts running) so the live timer survives polls.
      cur_since="$(sqlite3 "$bdb" "SELECT COALESCE(since,0) FROM bossbars WHERE id = $id;" 2>/dev/null || printf '0')"
      if [ "${minstart:-0}" -gt 0 ] 2>/dev/null && [ "${cur_since:-0}" -le 0 ] 2>/dev/null; then
        bossbar set "$id" --title "$bar_title" --color "$color" --progress "$prog" --since "$minstart" --box 0 2>/dev/null || true
      else
        bossbar set "$id" --title "$bar_title" --color "$color" --progress "$prog" --box 0 2>/dev/null || true
      fi
    fi
  done <<EOF
$selected
EOF
done

# Prune bars for commits no longer in flight: any overlay row whose url is a
# commit page but is not in this poll's active set has finished all its checks
# (or was cancelled), so drop it (the overlay poofs it out). Scoped to commit
# urls so downtime bars (status-page url) and hand-added bars are never touched.
if [ -n "$bdb" ]; then
  existing="$(sqlite3 -noheader -separator "	" "$bdb" \
    "SELECT id, url FROM bossbars WHERE url LIKE 'https://github.com/%/commit/%';" 2>/dev/null || true)"
  while IFS=$'\t' read -r eid eurl; do
    [ -n "${eid:-}" ] || continue
    if ! printf '%s\n' "$active_urls" | grep -qxF "$eurl"; then
      bossbar rm "$eid" 2>/dev/null || true
    fi
  done <<EOF
$existing
EOF

  # Legacy migration: an earlier version of this watcher drew one bar per
  # workflow RUN (url .../actions/runs/<id>). The per-commit scheme never creates
  # those, and nothing else does (pr-watch uses the xp-orb feed, ix-downtime uses
  # the status-page url), so any such row is a stale leftover from before the
  # upgrade. Drop them so a host that ran the old version self-heals on first poll.
  legacy="$(sqlite3 -noheader "$bdb" \
    "SELECT id FROM bossbars WHERE url LIKE '%/actions/runs/%';" 2>/dev/null || true)"
  while IFS= read -r eid; do
    [ -n "${eid:-}" ] || continue
    bossbar rm "$eid" 2>/dev/null || true
  done <<EOF
$legacy
EOF
fi
