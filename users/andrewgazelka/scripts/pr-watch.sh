# Body of the `pr-watch` bash app (see users/andrewgazelka/home.nix).
# No shebang / `set` line: the writeBashApplication wrapper supplies bash + `set -euo
# pipefail` and bakes gh/jq/ci-triage/claude/coreutils onto PATH via
# runtimeInputs. The module bakes these placeholders at build time:
#   @REPOS@            the watched repos as a quoted bash-array body
#   @ORB_BIN@          absolute path to the xp-orb-overlay binary (karma feed)
#   @LOG_DIR@          directory the detached ci-triage stage-2 log is appended to
#   @TRIAGE_COOLDOWN@  default seconds between stage-2 deep dives per repo+workflow
#
# This is the visual half of the "karma feed": successes float a green XP orb up
# the screen, failures pop a grey angry-villager puff. Nothing is spoken; the feed
# overlay plays the per-kind Minecraft sound (orb pickup / villager "no").
#
# Polls each watched repo for PRs newly merged into `main`. For each newly merged
# PR it queues a labelled XP orb in the feed overlay
# (`xp-orb-overlay push "<repo>: <title>"`), which floats up and fades ("rise &
# pop") with the orb pickup sound. It deliberately does NOT report the still-open
# / pending PR queue. The first run per repo only seeds the state file, so
# existing PRs stay quiet.
#
# It ALSO polls each repo for Actions runs that newly FAILED on `main` and
# responds in TWO stages per newly-failed run:
#   Stage 1 (immediate): queue a silent angry-villager pop in the feed overlay,
#     labelled with what failed (grey puff + villager "no" sound, no speech).
#   Stage 2 (deep dive): launch `ci-triage` DETACHED (own session, like
#     say-detached, wrapped in `timeout`) so it never blocks stage 1 for the
#     next failure and survives a reload. ci-triage fetches the failed logs, has
#     Opus 4.8 diagnose the root cause silently, and files (or dedupes) a
#     Linear ENG ticket when the failure is a genuine code break.
# A separate per-repo state file dedupes on the run's databaseId, and the first
# run per repo seeds it quietly so the existing backlog of past failures stays
# silent (no stage 1 and no stage 2 for already-seen runs).

state_dir="${PR_WATCH_STATE:-$HOME/.cache/pr-watch}"
mkdir -p "$state_dir"
# The detached stage-2 appends to @LOG_DIR@/ci-triage.log; ensure the dir exists
# (launchd pre-creates it for the agent's own StandardOutPath on macOS, but
# systemd does not guarantee a custom logDir parent).
mkdir -p "@LOG_DIR@"

# Non-overlap guard, intrinsic to the watcher so the portable service can fire it
# on a fixed interval with no external lock wrapper. Take a NON-BLOCKING
# exclusive flock on fd 9; if a previous fire still holds it, skip this one
# silently. bash keeps fd 9 open for the whole run, so the lock is held until
# exit/crash (the kernel drops it the instant the holder dies). The detached
# stage-2 ci-triage launch closes fd 9 (`9>&-`) so a long deep dive can never
# keep the lock and starve the next poll.
exec 9>"$state_dir/poll.lock"
perl -e 'use Fcntl ":flock"; flock(STDIN, LOCK_EX | LOCK_NB) or exit 1' <&9 || exit 0

repos=(@REPOS@)

for repo in "${repos[@]}"; do
  slug="${repo//\//_}"
  seen="$state_dir/$slug.seen"
  first_run=0
  if [ ! -f "$seen" ]; then
    first_run=1
    : >"$seen"
  fi

  json="$(gh pr list --repo "$repo" --state merged --base main \
            --json number,title,author --limit 50 2>/dev/null)" || continue

  while IFS=$'\t' read -r num title who; do
    [ -n "${num:-}" ] || continue
    grep -qxF "$num" "$seen" && continue
    echo "$num" >>"$seen"

    [ "$first_run" -eq 1 ] && continue

    short="${repo##*/}"
    echo "MERGED: [$short #$num] $title  (by $who)"

    # Queue a labelled XP orb in the feed overlay. It floats up and fades with
    # the orb pickup sound (played by the feed). Best-effort: if the feed overlay
    # is not running the event just sits in the DB (pruned after a few minutes),
    # so never fail the poll on a push error.
    @ORB_BIN@ push "$short: $title" || true
  done < <(printf '%s' "$json" | jq -r '
    .[] | [ (.number|tostring),
            .title,
            (if (.author.name // "") != "" then .author.name else .author.login end)
          ] | @tsv')

  # --- Newly FAILED Actions runs on main -----------------------------------
  # Same pattern as merges: a separate per-repo state file keyed on the run's
  # databaseId, seeded quietly on first run so old failures stay silent.
  runs_seen="$state_dir/$slug.runs.seen"
  runs_first_run=0
  if [ ! -f "$runs_seen" ]; then
    runs_first_run=1
    : >"$runs_seen"
  fi

  # --status failure means completed AND failed, so nothing in-progress here.
  runs_json="$(gh run list --repo "$repo" --branch main --status failure \
                 --json databaseId,displayTitle,workflowName,headSha,createdAt,url \
                 --limit 30 2>/dev/null)" || runs_json=""
  [ -n "$runs_json" ] || runs_json="[]"

  while IFS=$'\t' read -r run_id run_title run_wf run_url; do
    [ -n "${run_id:-}" ] || continue
    grep -qxF "$run_id" "$runs_seen" && continue
    echo "$run_id" >>"$runs_seen"

    [ "$runs_first_run" -eq 1 ] && continue

    short="${repo##*/}"
    echo "CI FAILED: [$short run $run_id] $run_wf  -  $run_title"

    # Names of jobs that failed (and any failed step), robust to empty jobs.
    failed_jobs="$(gh run view "$run_id" --repo "$repo" --json jobs 2>/dev/null \
                   | jq -r '
        [ (.jobs // [])[]
          | select((.conclusion // "" | ascii_downcase) == "failure")
          | (.name) as $job
          | ([ (.steps // [])[]
               | select((.conclusion // "" | ascii_downcase) == "failure")
               | .name ] | join(", ")) as $steps
          | if $steps == "" then $job else "\($job) (step: \($steps))" end ]
        | join("; ")')" || failed_jobs=""
    [ -n "$failed_jobs" ] || failed_jobs="(unknown jobs)"

    # --- Stage 1: immediate, silent angry-villager pop ---------------------
    # Visual only: queue a failure villager in the feed overlay, labelled with
    # what failed. The feed plays the villager "no" grunt; nothing is spoken.
    # Best-effort, like the merge orb: never fail the poll on a push error.
    @ORB_BIN@ push "$short: $run_wf failed: $failed_jobs" --kind villager || true

    # --- Stage 2: detached Opus deep dive (root cause + Linear ticket) ------
    # Cost/noise guard: when main stays red, runs fail back-to-back. Stage 1
    # (cheap) fires for each, but the expensive Opus deep dive + Linear ticket is
    # rate-limited to one per repo+workflow per cooldown window, so a sustained
    # outage can't spawn a storm of Opus runs or near-duplicate tickets.
    cooldown="${CI_TRIAGE_COOLDOWN:-@TRIAGE_COOLDOWN@}"
    wf_slug="$(printf '%s' "$run_wf" | tr -c '[:alnum:]' '_')"
    triage_ts="$state_dir/$slug.$wf_slug.triage.ts"
    now_ts="$(date +%s)"
    last_ts="$(cat "$triage_ts" 2>/dev/null)"; [ -n "$last_ts" ] || last_ts=0
    if [ "$((now_ts - last_ts))" -lt "$cooldown" ]; then
      echo "STAGE2 SKIPPED (cooldown ${cooldown}s): $short $run_wf run $run_id"
      continue
    fi
    printf '%s' "$now_ts" >"$triage_ts"

    # Launch ci-triage in its own session (POSIX setsid via perl, same
    # mechanism say-detached uses) so it never blocks stage 1 for the next
    # failure and survives a `bootout`/reload; `timeout` caps a stuck Opus run.
    # `9>&-` closes the inherited overlap-lock fd in the detached child so a long
    # deep dive can't hold the lock past this poll. CI_TRIAGE_DRY_RUN passes
    # through, so dry-run testing skips sound/voice/ticket here too.
    [ -n "$run_url" ] || run_url="https://github.com/$repo/actions/runs/$run_id"
    perl -e 'use POSIX qw(setsid); setsid() or exit 1; exec @ARGV or exit 1' \
      -- timeout 300 ci-triage "$repo" "$run_id" "$run_url" "$run_wf" "$failed_jobs" \
      >>"@LOG_DIR@/ci-triage.log" 2>&1 9>&- &
    # Don't wait on the backgrounded detached job; let stage 1 proceed.
    disown 2>/dev/null || true
  done < <(printf '%s' "$runs_json" | jq -r '
    .[] | [ (.databaseId|tostring),
            .displayTitle,
            .workflowName,
            (.url // "")
          ] | @tsv')
done
