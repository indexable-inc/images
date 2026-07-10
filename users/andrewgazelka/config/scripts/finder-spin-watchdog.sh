# Body of the `finder-spin-watchdog` writeShellApplication (see profiles/darwin-home.nix).
# No shebang / `set` line: writeShellApplication supplies bash + `set -euo pipefail`
# and bakes coreutils onto PATH via runtimeInputs.
#
# Watchdog for the recurring Finder /nix/store CPU spin
# (https://github.com/andrewgazelka/nix/issues/66): any Finder navigation into
# /nix/store makes DesktopServices materialize a store TNode; the store churns
# constantly (builds), so `TNode::SynchronizeChildren` re-enumerates all ~300k
# entries and diffs them with an O(n^2) `FindRenamedChild` loop, forever
# (50-100% CPU for hours-to-days; three occurrences 2026-07-06..07). Apple-side
# bug we cannot patch, so this agent detects the signature and applies the
# validated remediation, loudly.
#
# Detection:
#   - macOS `ps` reports pcpu 0.0 for Finder, so real CPU comes from the SECOND
#     sample of `top -l 2` (the first sample is a since-boot average).
#   - CPU > 50% alone is not enough (a big copy also spikes Finder): the fix
#     only fires when `sample <pid> 3` shows SynchronizeChildren
#     (DesktopServicesPriv) on a stack.
#
# Remediation (order matters, validated live 2026-07-07):
#   1. `defaults delete com.apple.finder FXRecentFolders` FIRST: prevents the
#      bookmark resolve at the next launch. A missing key is tolerated: the
#      plist can be clean while an already-materialized TNode keeps spinning.
#   2. `kill -9` LAST (KILL, never TERM, so Finder cannot write the cached
#      recent-folders array back on exit). macOS relaunches Finder itself.
#
# Every remediation is loud: a macOS notification plus a timestamped log entry
# with the sample excerpt (stdout/stderr land in
# ~/Library/Logs/finder-spin-watchdog.log via launchd). No silent activations.

threshold=50

log() {
  printf '%s finder-spin-watchdog: %s\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$*"
}

pids="$(/usr/bin/pgrep -x Finder || true)"
[ -n "$pids" ] || exit 0 # no Finder running, nothing to watch

while read -r pid; do
  # Two-sample CPU: the last row `top` prints for the pid is the second
  # (real, delta-based) sample; 2s apart is enough to expose the spin.
  cpu="$(/usr/bin/top -l 2 -s 2 -pid "$pid" -stats pid,cpu 2>/dev/null \
    | /usr/bin/awk -v pid="$pid" '$1 == pid { c = $2 } END { printf "%.1f", c + 0 }')"

  if /usr/bin/awk -v c="$cpu" -v t="$threshold" 'BEGIN { exit !(c <= t) }'; then
    continue # calm (or already gone): the common, silent path
  fi

  log "Finder pid $pid at ${cpu}% CPU (> ${threshold}%); sampling for the SynchronizeChildren signature"
  sample_file="$(mktemp "${TMPDIR:-/tmp}/finder-spin-sample.XXXXXX")"
  /usr/bin/sample "$pid" 3 -file "$sample_file" >/dev/null 2>&1 || true

  if ! /usr/bin/grep -q SynchronizeChildren "$sample_file"; then
    # High CPU without the signature (e.g. a large copy): never remediate,
    # but leave a visible trail plus the full sample for diagnosis.
    log "no SynchronizeChildren in sample; NOT remediating (sample kept at $sample_file)"
    continue
  fi

  log "signature confirmed for pid $pid at ${cpu}% CPU; sample excerpt:"
  /usr/bin/grep -m 8 SynchronizeChildren "$sample_file" | /usr/bin/sed 's/^/    /' || true

  # 1. Pref delete FIRST. `defaults delete` fails on a missing key and that
  #    must not abort the kill below.
  if /usr/bin/defaults delete com.apple.finder FXRecentFolders 2>/dev/null; then
    log "deleted com.apple.finder FXRecentFolders"
  else
    log "FXRecentFolders already absent (plist clean; spin came from live navigation or restored state)"
  fi

  # 2. SIGKILL LAST (never TERM). macOS relaunches Finder automatically.
  if /bin/kill -9 "$pid" 2>/dev/null; then
    log "sent SIGKILL to Finder pid $pid; macOS will relaunch it (full sample: $sample_file)"
  else
    log "kill -9 $pid failed (Finder exited on its own?)"
  fi

  # Message goes through argv, not string splicing, so no AppleScript quoting.
  /usr/bin/osascript \
    -e 'on run argv' \
    -e 'display notification (item 1 of argv) with title "finder-spin-watchdog" sound name "Basso"' \
    -e 'end run' \
    "Killed Finder (pid $pid): /nix/store SynchronizeChildren spin at ${cpu}% CPU. FXRecentFolders cleared; see ~/Library/Logs/finder-spin-watchdog.log" \
    || log "osascript notification failed"
done <<<"$pids"
