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
# Draws ONE Minecraft boss bar per in-flight BRANCH across the watched repos (not
# per workflow run, and not per commit): a branch with several checks in flight,
# even after a few force-pushes, shows a single bar. Within a branch only the
# LATEST commit (newest createdAt) counts, so superseded pushes don't pile up
# bars. This is the live-progress companion to the [[ix-downtime]] outage bars
# (red/yellow/blue) and the [[pr-watch]] karma feed (merge orb / failure
# villager): those react to discrete events, this reconciles a continuous "what's
# building" view. SILENT (pr-watch owns the success/failure sound); the fill is
# the signal.
#
# Color is deliberately OUTSIDE the downtime palette so the two never read alike:
#   - any check running or finished -> green;
#   - all of the latest commit's checks still queued / unpicked -> purple.
#
# Fill is TIME-EXTRAPOLATED by the overlay, not stepped by this poller: we write
# `since` (the commit's earliest check start) and `eta` (the slowest workflow's
# mean wall-clock of recent SUCCESSFUL runs, since checks run in parallel), and
# the overlay advances the bar as (now-since)/eta each second, capped near full
# on overrun. So between our ~20s polls the bar still moves smoothly. avg per
# workflow is cached in SQLite (refreshed at most once per CI_BARS_AVG_TTL). A
# static `--progress` is also written as a fallback for an overlay without eta
# support. The title is just "<repo>: <branch>" (no fraction).
#
# A bar's identity is the BRANCH url (https://github.com/<repo>/commits/<branch>),
# stable across pushes, so the bar heals in place as new commits land instead of
# spawning a new bar each push, and clicking opens the branch's commit/CI list.
# When a branch's latest commit finishes (no checks running or queued) its bar is
# pruned: we drop every overlay row with an https://github.com/ url not in this
# poll's active set (which also self-heals bars from older per-run/per-commit
# schemes). The overlay poofs the removed bar out. Bars are boxless (--box 0).

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

# GitHub avatars are cached on disk and reused across polls so a steady bar does
# not re-download a face every ~20s. One PNG per login under avatars/, refreshed
# only when older than this many seconds.
avatar_dir="$state_dir/avatars"
mkdir -p "$avatar_dir"
avatar_ttl=604800 # 7 days

# Resolve a GitHub login to a local avatar PNG path, downloading (and caching) it
# on first use. Prints the path on success, nothing on failure (an empty --icon
# clears the bar's icon, so a missing avatar just draws the bar without a face).
# `https://github.com/<login>.png` redirects to the user's avatar at the asked
# size; curl follows it (-L). Logins are [A-Za-z0-9-], safe as a filename.
avatar_for_login() {
  local login="$1" f
  [ -n "$login" ] || return 0
  case "$login" in *[!A-Za-z0-9-]*) return 0 ;; esac # paranoia: never a path
  f="$avatar_dir/$login.png"
  if [ ! -s "$f" ] || [ "$((now - $(date -r "$f" +%s 2>/dev/null || echo 0)))" -ge "$avatar_ttl" ]; then
    # Download to a temp then move, so a torn/failed download never leaves a
    # half-written PNG the overlay would fail to decode. -f: no body on HTTP error.
    if curl -fsSL "https://github.com/$login.png?size=128" -o "$f.tmp" 2>/dev/null; then
      mv -f "$f.tmp" "$f"
    else
      rm -f "$f.tmp"
      [ -s "$f" ] || return 0 # no fresh copy and no usable old one
    fi
  fi
  printf '%s' "$f"
}

# Squeeze a PR/commit title into one clean ASCII line for the bitmap font, which
# has no glyphs outside printable ASCII: drop non-ASCII (emoji, accents), collapse
# runs of whitespace, trim, and clip to a width that fits a boss bar without
# wrapping. Keeps the human-readable head of a long title with an ellipsis.
clean_title() {
  perl -CS -pe 's/[^\x20-\x7E]//g; s/\s+/ /g; s/^ //; s/ $//;' <<<"${1:-}" \
    | { read -r line || true; if [ "${#line}" -gt 56 ]; then printf '%s...' "${line:0:53}"; else printf '%s' "$line"; fi; }
}

# Login of the author of a commit, cached per sha for the poll (a main-branch bar
# has no PR, so its face comes from whoever pushed the commit). Empty on failure.
declare -A sha_login_cache
commit_login() {
  local repo="$1" sha="$2" login
  local key="$repo@$sha"
  if [ -n "${sha_login_cache[$key]+x}" ]; then
    printf '%s' "${sha_login_cache[$key]}"
    return
  fi
  login="$(gh api "repos/$repo/commits/$sha" --jq '.author.login // ""' 2>/dev/null || printf '')"
  sha_login_cache["$key"]="$login"
  printf '%s' "$login"
}

# Branch urls with in-flight CI this poll (newline-separated), used to prune bars
# for branches whose latest commit has finished. Identity is the branch, so a
# branch's checks (across pushes) share ONE bar.
active_urls=""

for repo in "${repos[@]}"; do
  short="${repo##*/}"
  # One list call per repo per poll: recent runs across all branches and
  # workflows, grouped below by branch -> latest commit. The window must hold all
  # of the latest commit's checks; an in-flight commit's runs are recent, so 200
  # is comfortably enough.
  runs="$(gh run list --repo "$repo" --limit 200 \
    --json workflowName,headBranch,headSha,status,conclusion,startedAt,createdAt,url \
    2>/dev/null)" || continue
  [ -n "$runs" ] || continue

  # Per-commit aggregates (keyed by headSha) plus, per branch, which commit is the
  # latest. Reset before declaring: `declare -A` does NOT clear an already-set
  # assoc array, so a prior repo's entries would otherwise leak into this one.
  unset c_total c_done c_running c_queued c_minstart c_partmilli c_created c_maxavg b_latest_sha b_latest_created
  declare -A c_total c_done c_running c_queued c_minstart c_partmilli c_created c_maxavg b_latest_sha b_latest_created

  # Open PRs for this repo, so a CI bar can show the PR title + author face
  # instead of an opaque ref. Keyed both by head branch name (for branch runs)
  # and by "#<number>" (for the `refs/pull/<N>/head` refs that PR-event runs
  # report as their branch). One list call per repo; failure leaves the maps
  # empty and the bars fall back to the branch label with no face.
  unset pr_title pr_login
  declare -A pr_title pr_login
  while IFS=$'\t' read -r prnum prhead prlogin prtitle; do
    [ -n "$prnum" ] || continue
    pr_title["#$prnum"]="$prtitle"
    pr_login["#$prnum"]="$prlogin"
    if [ -n "$prhead" ]; then
      pr_title["$prhead"]="$prtitle"
      pr_login["$prhead"]="$prlogin"
    fi
  done < <(gh pr list --repo "$repo" --state open --limit 200 \
    --json number,title,headRefName,author 2>/dev/null \
    | jq -r '.[] | [(.number|tostring), .headRefName, (.author.login // ""), .title] | @tsv' \
    2>/dev/null || true)
  while IFS=$'\t' read -r sha branch status wf started created; do
    [ -n "$sha" ] || continue
    c_total["$sha"]=$((${c_total["$sha"]:-0} + 1))
    cc="$(iso_to_epoch "${created:-}")"
    [ "${cc:-0}" -gt "${c_created[$sha]:-0}" ] 2>/dev/null && c_created["$sha"]="$cc"
    # Track the newest commit per branch (the one whose CI we actually show).
    # Strict `-gt` keeps the first-seen on equal/0 createdAt; gh lists runs
    # newest-first, so first-seen is the newest even when timestamps tie.
    if [ "${cc:-0}" -gt "${b_latest_created[$branch]:-0}" ] 2>/dev/null; then
      b_latest_created["$branch"]="$cc"
      b_latest_sha["$branch"]="$sha"
    fi
    # Earliest check start = the commit's CI start (over any run that has started,
    # finished or running), stable for the commit so the overlay timer/fill anchor
    # does not jump as checks finish.
    st="$(iso_to_epoch "${started:-}")"
    if [ "${st:-0}" -gt 0 ] 2>/dev/null; then
      cm="${c_minstart[$sha]:-0}"
      { [ "$cm" -eq 0 ] || [ "$st" -lt "$cm" ]; } && c_minstart["$sha"]="$st"
    fi
    case "$status" in
      completed)
        c_done["$sha"]=$((${c_done["$sha"]:-0} + 1))
        ;;
      *) # running or queued/unpicked: contributes to eta (the slowest still-to-
         # finish workflow governs the commit's expected wall-clock).
        if [ "$status" = "queued" ] || [ "$status" = "requested" ] || [ "$status" = "waiting" ] || [ "$status" = "pending" ]; then
          c_queued["$sha"]=$((${c_queued["$sha"]:-0} + 1))
        else
          c_running["$sha"]=$((${c_running["$sha"]:-0} + 1))
        fi
        avg="$(get_avg "$repo" "$wf")"
        [ "${avg:-0}" -gt 0 ] 2>/dev/null || avg="$default_avg"
        [ "$avg" -gt "${c_maxavg[$sha]:-0}" ] 2>/dev/null && c_maxavg["$sha"]="$avg"
        # Static fallback fill (for an overlay without eta support).
        bst="${st:-0}"
        [ "$bst" -gt 0 ] 2>/dev/null || bst="$cc"
        el=$((now - bst))
        [ "$el" -ge 0 ] || el=0
        pm=$((el * 1000 / avg))
        [ "$pm" -lt 0 ] && pm=0
        [ "$pm" -gt 970 ] && pm=970
        c_partmilli["$sha"]=$((${c_partmilli["$sha"]:-0} + pm))
        ;;
    esac
  done < <(printf '%s' "$runs" | jq -r '
    .[] | [ .headSha, .headBranch, .status, .workflowName,
            (.startedAt // ""), (.createdAt // "") ] | @tsv')

  # Commits with in-flight work, collapsed to ONE winner ref each. The per-commit
  # counters (c_running/c_queued/...) are keyed by headSha, and a PR's head commit
  # is reachable under TWO refs that share that sha: the source branch (e.g.
  # `feature`) and the PR head ref (`refs/pull/<N>/head`, how some PR-event runs
  # report headBranch). Keying eligibility on the shared sha makes BOTH refs
  # eligible off the same in-flight runs, so one PR would draw two bars. So pick a
  # single winner ref per sha, preferring a non-`refs/pull/*/*` ref (the source
  # branch) so the bar heals across pushes and clicks through to the branch.
  #
  # Only the winner's url goes into active_urls, so the prune drops the bar of any
  # other ref sharing the sha (e.g. the superseded `refs/pull/<N>/head`) by
  # design: one in-flight commit == one bar. Of the winners only the newest
  # $max_bars are DRAWN; the rest still sit in active_urls so prune keeps them.
  unset sha_winner
  declare -A sha_winner
  for branch in "${!b_latest_sha[@]}"; do
    sha="${b_latest_sha[$branch]}"
    [ $((${c_running[$sha]:-0} + ${c_queued[$sha]:-0})) -gt 0 ] || continue
    cur="${sha_winner[$sha]:-}"
    if [ -z "$cur" ]; then
      sha_winner["$sha"]="$branch"
    else
      # First ref seen for a sha wins, except any non-refs/pull ref (the source
      # branch in practice) always beats a refs/pull/<N>/head ref already chosen.
      case "$cur" in
        refs/pull/*/*)
          case "$branch" in
            refs/pull/*/*) : ;; # two PR refs for one sha: keep the first
            *) sha_winner["$sha"]="$branch" ;;
          esac
          ;;
      esac
    fi
  done
  eligible=()
  for sha in "${!sha_winner[@]}"; do
    branch="${sha_winner[$sha]}"
    eligible+=("${c_created[$sha]:-0}	$branch")
    active_urls="$active_urls
https://github.com/$repo/commits/$branch"
  done
  selected=""
  [ "${#eligible[@]}" -gt 0 ] && selected="$(printf '%s\n' "${eligible[@]}" | sort -rn -k1 | head -n "$max_bars" | cut -f2-)"

  while IFS= read -r branch; do
    [ -n "$branch" ] || continue
    sha="${b_latest_sha[$branch]}"
    running=${c_running[$sha]:-0}
    done_n=${c_done[$sha]:-0}
    total=${c_total[$sha]:-1}
    url="https://github.com/$repo/commits/$branch"

    # Static fallback fill (the overlay extrapolates from since+eta when it can).
    progmilli=$(((done_n * 1000 + ${c_partmilli[$sha]:-0}) / total))
    [ "$progmilli" -lt 20 ] && progmilli=20
    [ "$progmilli" -gt 970 ] && progmilli=970
    prog="$(printf '0.%03d' "$progmilli")"

    # Green once any check is running or finished; purple while the latest commit's
    # checks are all still queued / not yet picked up by a runner.
    if [ "$running" -gt 0 ] || [ "$done_n" -gt 0 ]; then color="green"; else color="purple"; fi
    minstart=${c_minstart[$sha]:-0}
    eta=${c_maxavg[$sha]:-0}
    [ "$eta" -gt 0 ] 2>/dev/null || eta="$default_avg"

    # Human-readable title + author face. A `refs/pull/<N>/head` ref (how PR-event
    # runs report their branch) maps to PR #N; a plain branch maps to an open PR
    # by head name. With a PR we show its title and the author's avatar; without
    # one (e.g. a push to main) we keep the "<repo>: <branch>" label and use the
    # commit author's face. Titles are squeezed to one clean ASCII line for the
    # bitmap font.
    pr_key=""
    num=""
    case "$branch" in
      refs/pull/*/*)
        num="${branch#refs/pull/}"
        num="${num%%/*}"
        [ -n "$num" ] && pr_key="#$num"
        ;;
      *) pr_key="$branch" ;;
    esac
    title="${pr_title[$pr_key]:-}"
    login="${pr_login[$pr_key]:-}"
    # A `refs/pull/<N>/head` ref whose PR is already merged/closed is not in the
    # open-PR list, but the number is right there, so resolve #N directly (any
    # state). This is the common eyesore: a merged PR still running post-merge CI
    # would otherwise show the raw ref. Cache the lookup in the same per-poll maps.
    if [ -z "$title" ] && [ -n "$num" ]; then
      prow="$(gh pr view "$num" --repo "$repo" --json title,author \
        --jq '[.title, (.author.login // "")] | @tsv' 2>/dev/null || printf '')"
      if [ -n "$prow" ]; then
        title="${prow%%	*}"
        login="${prow#*	}"
        pr_title["$pr_key"]="$title"
        pr_login["$pr_key"]="$login"
      fi
    fi
    if [ -n "$title" ]; then
      bar_title="$(clean_title "$title")"
    else
      # No PR at all (e.g. a push to main): fall back to the readable branch label
      # and the commit author's face.
      bar_title="$short: $branch"
      [ -n "$sha" ] && login="$(commit_login "$repo" "$sha")"
    fi
    [ -n "$bar_title" ] || bar_title="$short: $branch"
    icon="$(avatar_for_login "$login")"

    # Always pass --eta, and --since once the commit's CI has started, so the
    # overlay extrapolates the fill live; both update if a newer commit lands on
    # the branch (the bar heals in place via the stable branch url).
    since_args=()
    [ "${minstart:-0}" -gt 0 ] 2>/dev/null && since_args=(--since "$minstart")
    # Expand with the `+` guard so an empty array is not an "unbound variable"
    # under `set -u` (matters only if run under an old bash that errors on an
    # empty `"${a[@]}"`; the Nix wrapper's bash 5 does not).
    id="$(bar_id_for_url "$url")"
    if [ -z "$id" ]; then
      bossbar add "$bar_title" --color "$color" --overlay progress --progress "$prog" --position -1 --eta "$eta" ${since_args[@]+"${since_args[@]}"} --url "$url" --icon "$icon" --box 0 2>/dev/null || true
    else
      bossbar set "$id" --title "$bar_title" --color "$color" --progress "$prog" --eta "$eta" ${since_args[@]+"${since_args[@]}"} --icon "$icon" --box 0 2>/dev/null || true
    fi
  done <<EOF
$selected
EOF
done

# Prune bars for branches no longer in flight: drop every overlay row whose url is
# a github.com page but is not in this poll's active set. This watcher owns ALL
# github.com bars (downtime uses the status-page url, the Ender Dragon seed uses
# minecraft.wiki, pr-watch uses the xp-orb feed not bossbars), so this also
# self-heals stale bars from older per-run (/actions/runs/) and per-commit
# (/commit/) schemes. The overlay poofs each removed bar out.
if [ -n "$bdb" ]; then
  existing="$(sqlite3 -noheader -separator "	" "$bdb" \
    "SELECT id, url FROM bossbars WHERE url LIKE 'https://github.com/%';" 2>/dev/null || true)"
  while IFS=$'\t' read -r eid eurl; do
    [ -n "${eid:-}" ] || continue
    if ! printf '%s\n' "$active_urls" | grep -qxF "$eurl"; then
      bossbar rm "$eid" 2>/dev/null || true
    fi
  done <<EOF
$existing
EOF
fi
