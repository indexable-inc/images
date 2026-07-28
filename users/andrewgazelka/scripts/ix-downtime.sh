# Body of the `ix-downtime` bash app (see users/andrewgazelka/home.nix).
# No shebang / `set` line: the writeBashApplication wrapper supplies bash + `set -euo
# pipefail` and bakes curl/jq/sqlite/minecraft-sound/claude/coreutils + the
# bossbar CLI + say-detached onto PATH via runtimeInputs.
#
# Mirrors the PUBLIC ix.dev status page onto a boss bar + an Ender Dragon growl,
# WITHOUT spamming while an outage continues. The source of truth is the status
# page's own `aggregate_state` (operational | degraded | downtime | maintenance),
# so the bar shows exactly when the page a human would look at shows a problem.
# This deliberately defers the "which monitors are public" decision to the status
# page rather than reimplementing it: the workspace also runs internal heartbeat
# monitors (hil-1/vint-1 vm-state, vm-launch, health-lifecycle) that fire while
# ix.dev itself serves 200, and the status page already decides which of those
# are visible. State lives in SQLite so the dedup logic is explicit and
# crash-safe. Rules:
#   - one ALARM per outage (the dragon growl), rate-limited to at most once per
#     IX_DOWNTIME_HORN_COOLDOWN seconds (default 1800);
#   - the growl + spoken root cause fire only when we actually alarm;
#   - on recovery, the Ender Dragon death sound (the "you won" cue) + a calm
#     spoken line, only for an outage we actually alarmed;
#   - the first run only seeds, so a pre-existing outage stays quiet;
#   - one boss bar PER non-operational service (region-qualified, e.g.
#     "US East: Long-running VMs (3m)"), colored by severity (downtime=red,
#     degraded=yellow, maintenance=blue); the overlay shows a live "down for X"
#     timer in the title (ticking on its own) and hovering shows a detail panel.
#     Each clears as that service recovers, independent of the alarm cooldown, so
#     the visual alert is always accurate to current per-service state.

state_dir="${IX_DOWNTIME_STATE:-$HOME/.cache/ix-downtime}"
mkdir -p "$state_dir"
db="$state_dir/state.db"
cooldown="${IX_DOWNTIME_HORN_COOLDOWN:-1800}"

# Non-overlap guard, intrinsic to the watcher so the launchd/systemd schedule
# can fire it on a fixed interval without two polls racing on the SQLite state
# (a slow `claude -p` summarize can take up to ~90s, longer than the 30s poll).
# A NON-BLOCKING exclusive flock on a lock file: if a previous run still holds
# it, this fire exits 0 silently (skipped) instead of overlapping; the kernel
# drops the lock the instant the holder dies, so a crash never wedges it. perl is
# always present on macOS (/usr/bin/perl) and on Linux. We re-exec the rest of
# the script under perl's flock, holding the fd in the perl parent (system, not
# exec, so close-on-exec does not drop the lock) and forwarding the child's exit.
if [ -z "${IX_DOWNTIME_LOCKED:-}" ]; then
  exec perl -MFcntl=:flock -e '
    my $lock = shift;
    open(my $fh, ">", $lock) or exit 1;
    unless (flock($fh, LOCK_EX | LOCK_NB)) { exit 0 }   # held -> skip quietly
    $ENV{IX_DOWNTIME_LOCKED} = "1";
    my $rc = system(@ARGV);
    exit($rc == -1 ? 1 : ($rc >> 8));
  ' "$state_dir/poll.lock" "$0" "$@"
fi

# Better Stack Uptime API token. Resolution order:
#   1. $IX_BETTERSTACK_TOKEN, if the consuming config exports one (e.g. a Linux
#      host that seeds it from its own secret store).
#   2. the macOS login Keychain (service `ix-betterstack-token`): a launchd agent
#      runs headless and cannot unlock `rbw`/`op` interactively, so the token is
#      staged in the Keychain with no secret in the Nix store or git. Seed it
#      idempotently from Vaultwarden with the host's `seed-launchd-secrets`.
token="${IX_BETTERSTACK_TOKEN:-}"
if [ -z "$token" ] && command -v security >/dev/null 2>&1; then
  token="$(security find-generic-password -s ix-betterstack-token -w 2>/dev/null)" || token=""
fi
if [ -z "$token" ]; then
  echo "ix-downtime: no Better Stack token (set IX_BETTERSTACK_TOKEN or seed the macOS Keychain); skipping"
  exit 0
fi

api() {
  curl -s -m 15 -H "Authorization: Bearer $token" \
    "https://uptime.betterstack.com/api/v2$1"
}

sqlite3 "$db" >/dev/null <<'SQL'
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT);
SQL

now="$(date +%s)"

# Double single quotes so a service title carrying an apostrophe cannot break out
# of a SQL string literal in the read-only bossbar lookups below. Fixed-key kv
# callers do not need it but are unharmed.
sq() { printf "%s" "${1//\'/\'\'}"; }
kv_get() { sqlite3 "$db" "SELECT value FROM kv WHERE key='$1';"; }
kv_set() { sqlite3 "$db" "INSERT INTO kv(key,value) VALUES('$1','$2') ON CONFLICT(key) DO UPDATE SET value=excluded.value;"; }

# Title-case status word for the pop-down description.
status_word_for() {
  case "$1" in
    degraded) echo "Degraded" ;;
    maintenance) echo "Maintenance" ;;
    downtime) echo "Downtime" ;;
    *) echo "Down" ;;
  esac
}

# Mirror the outage onto the Minecraft boss bar overlay: one bar per service the
# status page shows as not operational, so a glance says exactly WHAT is down, not
# just that something is. Titles are region-qualified ("US East: Long-running VMs")
# because the same monitor name repeats across sections (US West / US East). The
# bar color tracks severity.
#
# The title stays the stable service name; the OVERLAY appends a live "down for X"
# timer from the bar's `since` (set once when first seen) and ticks it itself, so
# we never rewrite the title to advance a clock. Hovering unfolds a panel (the
# bossbar `description`) with the status and source.
#
# A bar's identity is its row id. We look it up in the overlay's SQLite by service
# name (read-only; writes still go through the `bossbar` CLI, which owns the
# schema), matching either the clean name or a legacy "name (suffix)" title an
# earlier version of this script wrote, so a scheme change heals in place instead
# of leaving a duplicate.
bar_color_for() { case "$1" in degraded) echo yellow ;; maintenance) echo blue ;; *) echo red ;; esac; }
reconcile_bars() {
  local rows base status color word desc bdb id cur_since url_args
  bdb="$(bossbar db 2>/dev/null || true)"
  # Click action: open the public ix.dev status page ($status_url, derived from
  # the Better Stack page). Only pass --url when we have one, so a transient
  # empty does not clear an existing link.
  url_args=()
  [ -n "${status_url:-}" ] && url_args=(--url "$status_url")
  # Row id for a service, matching the clean name or a legacy suffixed title.
  bar_id_for() {
    [ -n "$bdb" ] || { printf ''; return; }
    sqlite3 "$bdb" "SELECT id FROM bossbars WHERE title = '$(sq "$1")' OR title LIKE '$(sq "$1") (%' ORDER BY id LIMIT 1;" 2>/dev/null || printf ''
  }
  rows="$(jq -rn --argjson res "$res" --argjson secs "$secs" '
    (($secs.data // []) | map({ (.id|tostring): .attributes.name }) | add // {}) as $sn
    | ($res.data // [])[]
    | ( ($sn[(.attributes.status_page_section_id|tostring)] // "ix")
        + ": " + (.attributes.public_name // .attributes.explanation // "service") )
      + "\t" + (.attributes.status // "operational")
  ')" || return 0
  while IFS=$'\t' read -r base status; do
    [ -n "$base" ] || continue
    id="$(bar_id_for "$base")"
    if [ "$status" = "operational" ]; then
      [ -n "$id" ] && bossbar rm "$id" 2>/dev/null || true # recovered: clear it
      continue
    fi

    color="$(bar_color_for "$status")"
    word="$(status_word_for "$status")"
    desc="$word on the ix.dev status page.

The title shows how long it has been down. This clears automatically when the service recovers."

    if [ -z "$id" ]; then
      # New outage: stamp since=now so the overlay counts up from here.
      bossbar add "$base" --color "$color" --overlay progress --progress 1.0 --position -1 --since "$now" --description "$desc" "${url_args[@]}" 2>/dev/null || true
    else
      # Already shown: refresh color/description/url and normalize the title to the
      # clean name. Start the timer only if it has none yet (legacy bar), so an
      # already-running clock is preserved across polls and restarts.
      cur_since="$(sqlite3 "$bdb" "SELECT COALESCE(since,0) FROM bossbars WHERE id = $id;" 2>/dev/null || printf '0')"
      if [ "${cur_since:-0}" -gt 0 ] 2>/dev/null; then
        bossbar set "$id" --title "$base" --color "$color" --progress 1.0 --description "$desc" "${url_args[@]}" 2>/dev/null || true
      else
        bossbar set "$id" --title "$base" --color "$color" --progress 1.0 --since "$now" --description "$desc" "${url_args[@]}" 2>/dev/null || true
      fi
    fi
  done <<EOF
$rows
EOF
}

# The public ix.dev status page is the source of truth. Find it by subdomain so a
# renamed/extra page does not silently swap which page we watch.
pages="$(api '/status-pages')" || exit 0
# Bail on a malformed body (rate-limit text, HTML error, truncated read) rather
# than silently treating it as all-clear and yanking the bar mid-outage.
printf '%s' "$pages" | jq -e 'has("data")' >/dev/null 2>&1 || exit 0
page="$(printf '%s' "$pages" | jq -r '.data[] | select(.attributes.subdomain=="ix") | .id' | head -1)"
[ -n "$page" ] || page="$(printf '%s' "$pages" | jq -r '.data[0].id // empty')"
[ -n "$page" ] || exit 0
agg="$(printf '%s' "$pages" | jq -r --arg id "$page" \
  '.data[] | select(.id==$id) | .attributes.aggregate_state // empty')"
# An empty state means we could not read it; bail rather than false-clear.
[ -n "$agg" ] || exit 0

# Public URL of the status page, for the bar's click action ("open Better Stack").
# Prefer the custom domain (status.ix.dev), else the Better Stack subdomain host.
# Derived from the API so it tracks the page rather than being hardcoded.
status_url="$(printf '%s' "$pages" | jq -r --arg id "$page" '
  .data[] | select(.id==$id) | .attributes
  | if (.custom_domain // "") != "" then "https://" + .custom_domain
    elif (.subdomain // "") != "" then "https://" + .subdomain + ".betteruptime.com"
    else "" end' 2>/dev/null || echo "")"

# `operational` is the only all-green state; anything else (degraded, downtime,
# maintenance) is the page showing a problem, so the bars show.
if [ "$agg" = "operational" ]; then down=0; else down=1; fi

# Per-resource breakdown + section names, fetched once and reused for both the
# boss bars and the spoken summary. Region-qualified because monitor names repeat
# across sections. bars_ok gates on a well-formed read of BOTH so a malformed body
# never false-clears an active outage's bars.
res="$(api "/status-pages/$page/resources")" || res=""
secs="$(api "/status-pages/$page/sections")" || secs=""
bars_ok=0
# Require `.data` to be an ARRAY, not merely present: a valid body like
# {"data":null} satisfies has("data") but makes the jq below iterate null and
# fail, which under `set -euo pipefail` would abort the watcher mid-outage.
if printf '%s' "$res" | jq -e '(.data | type) == "array"' >/dev/null 2>&1 \
  && printf '%s' "$secs" | jq -e '(.data | type) == "array"' >/dev/null 2>&1; then
  bars_ok=1
fi

# Migration: drop the legacy single aggregate bar from the old format, always.
bossbar rm "ix.dev down" 2>/dev/null || true

# Draw exactly the right per-service bars whenever we have a good read, on every
# poll and in every state (operational clears them, an outage shows them), so the
# overlay is always accurate independent of the horn cooldown below. Skipped on a
# bad read to avoid false-clearing during an outage.
[ "$bars_ok" = 1 ] && reconcile_bars

# Region-qualified list of currently-down services for the spoken summary/horn.
# Every pipeline here is `|| true`-guarded: on an operational poll down_names is
# empty, so `grep -ve '^$'` exits 1, and under `set -o pipefail` an unguarded
# command substitution would abort the script before the seed/recovery logic.
if [ "$bars_ok" = 1 ]; then
  down_names="$(jq -rn --argjson res "$res" --argjson secs "$secs" '
    (($secs.data // []) | map({ (.id|tostring): .attributes.name }) | add // {}) as $sn
    | ($res.data // [])[]
    | select((.attributes.status // "operational") != "operational")
    | ( ($sn[(.attributes.status_page_section_id|tostring)] // "ix")
        + ": " + (.attributes.public_name // .attributes.explanation // "service") )' \
    2>/dev/null | sort -u || true)"
else
  down_names=""
fi
down_list="$(printf '%s' "$down_names" | grep -ve '^$' | paste -sd ', ' - || true)"
[ -n "$down_list" ] || down_list="one or more services"
down_count="$(printf '%s' "$down_names" | grep -cve '^$' || true)"
down_count="${down_count:-0}"

# First run only seeds current state, no alert, so a pre-existing outage stays
# quiet but still gets a recovery line when it clears.
if [ "$(kv_get seeded)" != "1" ]; then
  kv_set seeded 1
  if [ "$down" = 1 ]; then
    kv_set outage_active 1
    kv_set horned_this_outage 1
    kv_set last_horn_at "$now"
  else
    kv_set outage_active 0
    kv_set horned_this_outage 0
    kv_set last_horn_at 0
  fi
  echo "ix-downtime: seeded (state=$agg); staying quiet on existing"
  exit 0
fi

outage_active="$(kv_get outage_active)"
outage_active="${outage_active:-0}"
horned="$(kv_get horned_this_outage)"
horned="${horned:-0}"
last_horn_at="$(kv_get last_horn_at)"
last_horn_at="${last_horn_at:-0}"

if [ "$down" = 0 ]; then
  # Operational: reconcile_bars above already cleared every service bar; here we
  # only handle the recovery voice once.
  if [ "$outage_active" = "1" ]; then
    kv_set outage_active 0
    kv_set horned_this_outage 0
    if [ "$horned" = "1" ]; then
      echo "RECOVERED"
      # Victory: the Ender Dragon death sound, the same "you won" cue the game
      # plays when the dragon dies. Only fires for an outage we actually alarmed.
      say-detached mob/enderdragon/end "Recovered. ix dot dev is back up."
    fi
  fi
  exit 0
fi

kv_set outage_active 1
# Per-service bars were already reconciled above (independent of horn rate) and
# $down_list/$down_count are set; this path only decides whether to horn.

# Horn at most once per outage (horned_this_outage), and never more often than the
# cooldown (so a flapping page separated by brief recoveries stays quiet).
if [ "$horned" = "1" ] || [ $((now - last_horn_at)) -lt "$cooldown" ]; then
  echo "DOWN (state=$agg: $down_list); horned=$horned, within ${cooldown}s cooldown -> quiet"
  exit 0
fi
kv_set last_horn_at "$now"
kv_set horned_this_outage 1

if [ "$down_count" -gt 1 ]; then
  horn_line="Downtime. $down_count services down: $down_list."
else
  horn_line="Downtime. $down_list is down."
fi
echo "HORN: state=$agg ($down_list)"

# Immediate intense alarm: the Ender Dragon growl (an ominous boss roar that fits
# "a boss is here / something is down") + a terse spoken flag.
say-detached mob/enderdragon/growl1 "$horn_line"

# Richer detail for the spoken root cause.
detail="Status page state: $agg
Down resources: $down_list"

sys='You turn a Better Stack status-page outage (overall state plus the list of resources currently down) into a short spoken alert for a busy on-call engineer who cannot see the screen. Output ONLY one or two calm, plain-English sentences, about 25 words total. No preamble, no questions, no offer to help, no markdown, no code. Say what is down, like: ix dot dev is in downtime; the Registry and Long-running VMs are affected.'

# Isolated, headless summarizer: no settings/memory/hooks, no tools, fast model.
summary="$(cd "$HOME" && printf 'Outage:\n%s' "$detail" \
  | timeout 90 claude -p \
    --model claude-haiku-4-5-20251001 \
    --allowedTools "" \
    --setting-sources "" \
    --system-prompt "$sys" \
    'Announce this downtime and what is affected.' 2>/dev/null)" || summary=""

[ -n "$summary" ] || summary="$down_list down. Cause unknown so far."

echo "SAY: $summary"
# Normal voice for the detail (no second horn).
say-detached "" "$summary"
