#!/usr/bin/env bash
# Overseer tick: collect a JSON snapshot of agent activity on this host,
# ask a read-only headless codex for a terse digest, and regenerate the
# HTML report. The script owns the collectors and the page markup; the
# agent only authors the digest text (a read-only sandbox cannot write
# the page anyway).
set -euo pipefail
trap 'echo "overseer.sh: failed at line $LINENO (exit $?)" >&2' ERR

state_dir="$HOME/.local/share/symphony/overseer"
html="$state_dir/index.html"
snap="$state_dir/snapshot.json"
notes="$state_dir/notes.md"   # the overseer's own memory across ticks
mkdir -p "$state_dir/snapshots"
touch "$notes"

# Exec nodes run with the pack directory as cwd (ExecRunner).
prompt_file="$PWD/prompts/overseer.md"
last_msg="$(mktemp)"
workdir="$(mktemp -d)" # empty cwd for codex; --skip-git-repo-check below
cleanup() { rm -rf "$last_msg" "$last_msg.envelope" "$workdir" "${report_file:-}" "${data_json:-}"; }
trap cleanup EXIT

now_iso="$(date +%Y-%m-%dT%H:%M:%S%z)"

# ---- collectors: each emits JSON on stdout ----------------------------

# Every process, parsed into fields once; agent/hot/stall views are jq
# filters over this. tty="??" separates headless (launchd, cron, exec
# nodes) from interactive terminal sessions.
ps_json="$(ps axo pid=,ppid=,pcpu=,etime=,tty=,args= | jq -Rs '
  [split("\n")[]
   | capture("^ *(?<pid>[0-9]+) +(?<ppid>[0-9]+) +(?<pcpu>[0-9.]+) +(?<etime>[0-9:-]+) +(?<tty>[^ ]+) +(?<args>.*)$")?
   | .pid |= tonumber | .ppid |= tonumber | .pcpu |= tonumber
   | .args |= .[0:220]
   # elapsed minutes from etime (mm:ss, hh:mm:ss, or d-hh:mm:ss)
   | .minutes = (.etime | capture("^((?<d>[0-9]+)-)?((?<h>[0-9]+):)?(?<m>[0-9]+):(?<s>[0-9]+)$")?
                 | (((.d // "0") | tonumber) * 1440 + ((.h // "0") | tonumber) * 60 + (.m | tonumber)))
   | .minutes //= 0]')"

agents="$(jq '[.[] | select(.args | test("(^|/)(claude|codex)( |$)|beam\\.smp|codex exec"))
               | select(.args | test("overseer|ps axo") | not)]' <<<"$ps_json")"

# cwd per agent pid (lsof), so the digest can tie a process to the
# session transcript working in that directory.
cwd_map="$(
  jq -r '.[].pid' <<<"$agents" | while read -r pid; do
    d="$(lsof -a -p "$pid" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' | head -1)"
    [ -n "$d" ] && jq -n --argjson pid "$pid" --arg cwd "$d" '{($pid | tostring): $cwd}'
  done | jq -s 'add // {}'
)"
agents="$(jq --argjson cwds "$cwd_map" 'map(. + {cwd: ($cwds[.pid | tostring] // null)})' <<<"$agents")"

# parent_chain: ancestor command lines, child-first. Root helpers show as
# bare "(name)" with no args, so ancestry is the only evidence of ownership
# the judge gets (#2169: a root fs_usage owned by nix-web-monitor was
# flagged as an orphan and a fixer was dispatched to kill it).
parents_def='def parents($by): [.ppid | tostring | $by[.]? // empty
  | recurse($by[.ppid | tostring]? // empty) | .args] | .[0:5];'
hot="$(jq "$parents_def"'
  INDEX(.pid | tostring) as $by
  | [.[] | select(.pcpu >= 30) | . + {parent_chain: parents($by)}]
  | sort_by(-.pcpu) | .[0:8]' <<<"$ps_json")"

# Wedge heuristic (#2011 signature): a headless agent process idling at
# ~0% CPU for a long time. Interactive agents idle at 0% legitimately
# (waiting on the human), so only tty-less processes qualify.
stalled="$(jq --argjson all "$ps_json" "$parents_def"'
  INDEX($all[]; .pid | tostring) as $by
  | [.[] | select(.tty == "??" and .pcpu < 1 and .minutes >= 15)
     | . + {parent_chain: parents($by)}]' <<<"$agents")"

# Claude Code sessions active in the last 12h: cwd and texts come from
# the transcript itself (the dir-name encoding is lossy). age_min against
# the file mtime is the progress signal the digest correlates with live
# processes: a running agent whose transcript stopped moving is suspect.
claude_sessions="$(
  find "$HOME/.claude/projects" -name '*.jsonl' -mmin -720 -print0 2>/dev/null |
  perl -0ne 'my @s = stat($_); print "$s[9] $_\n" if @s' | sort -rn | awk 'NR <= 15' |
  while read -r mtime f; do
    tail -n 80 "$f" | jq -Rrs --arg mtime "$mtime" --arg now "$(date +%s)" '
      [split("\n")[] | fromjson?] | {
        last_active: ($mtime | tonumber | todate),
        age_min: ((($now | tonumber) - ($mtime | tonumber)) / 60 | floor),
        cwd: ([.[] | .cwd // empty] | last // "?"),
        last_user: ([.[] | select(.type == "user") | .message.content
                     | if type == "string" then . else ([.[]? | select(.type == "text") | .text] | join(" ")) end
                     | select(length > 0)] | last // "" | .[0:200]),
        last_text: ([.[] | select(.type == "assistant") | .message.content[]?
                     | select(.type == "text") | .text] | last // "" | .[0:240]),
        recent_tool_errors: ([.[] | .. | objects | select(.is_error? == true)] | length)
      }' 2>/dev/null || true
  done | jq -s '.'
)"

# Codex sessions active in the last 12h (rollout transcripts). jq builds
# the JSON (never hand-printf a serialized form: an awk-printf here fed
# jq raw control bytes under the launchd locale and failed every tick).
codex_sessions="$(
  find "$HOME/.codex/sessions" -name '*.jsonl' -mmin -720 -print0 2>/dev/null |
  perl -0ne 'my @s = stat($_); print "$s[9] $_\n" if @s' | sort -rn | awk 'NR <= 10' |
  while read -r mtime f; do
    jq -cn --arg m "$mtime" --arg p "$f" --arg now "$(date +%s)" \
      '{last_active: ($m | tonumber | todate),
        age_min: ((($now | tonumber) - ($m | tonumber)) / 60 | floor),
        path: $p}'
  done | jq -s '.'
)"

# Symphony runs from the local runtime. An unreachable API is itself a
# finding, so the failure is recorded in the snapshot, never swallowed.
# The API lists runs grouped by workflow, oldest-first; sort to keep the
# NEWEST runs, or the window freezes in the past as the store grows and
# the judge sees phantom cron outages (#2183).
symphony_runs="$(curl -s --max-time 5 http://127.0.0.1:4040/api/v1/ir/runs |
  jq '[.runs[] | {run_id, status, states, trigger, created_at, updated_at}]
      | sort_by(.created_at) | reverse | .[0:12]' ||
  echo '{"error": "symphony runtime unreachable on 127.0.0.1:4040"}')"

# Sleep/wake context. A cron run that straddles a sleep window dies by
# wall-clock timeout (#2216) or by DNS-retry exhaustion (a DarkWake has
# no SSID), and the run record alone cannot show that: without this
# signal the judge diagnosed "failures while awake" on a host pmset
# shows was in Clamshell Sleep for the whole window and dispatched a
# fixer on that phantom premise. Last transitions only; the grep streams
# the full pmset log (~seconds), so keep this the only pass over it.
power="$(
  {
    pmset -g batt | head -1
    # A host that has not slept since boot greps empty; that is a real,
    # non-fatal state under pipefail.
    pmset -g log | grep -E 'Entering Sleep|DarkWake|Wake from' | tail -25 || true
  } | jq -Rs 'split("\n") | map(select(length > 0) | gsub(" +$"; "") | .[0:160])
              | {batt: .[0], transitions: .[1:]}'
)"

loadavg="$(sysctl -n vm.loadavg | tr -d '{}' | awk '{print $1}')"
ncpu="$(sysctl -n hw.ncpu)"
cpu_pct="$(jq '([.[].pcpu] | add // 0) / '"$ncpu"' | round' <<<"$ps_json")"
mem_pct="$(vm_stat | awk -v total="$(sysctl -n hw.memsize)" '
  /page size of/ { psize = $8 }
  /Pages (active|wired down|occupied by compressor)/ { used += $NF }
  END { printf "%d", used * psize / total * 100 }')"

jq -n \
  --arg now "$now_iso" --arg loadavg "$loadavg" --argjson ncpu "$ncpu" \
  --argjson cpu_pct "$cpu_pct" --argjson mem_pct "$mem_pct" \
  --argjson agents "$agents" --argjson hot "$hot" --argjson stalled "$stalled" \
  --argjson claude_sessions "$claude_sessions" --argjson codex_sessions "$codex_sessions" \
  --argjson symphony_runs "$symphony_runs" --argjson power "$power" \
  '{now: $now, load_1m: ($loadavg | tonumber), ncpu: $ncpu, cpu_pct: $cpu_pct, mem_pct: $mem_pct,
    agent_processes: $agents, hot_processes: $hot, stalled_suspects: $stalled,
    claude_sessions: $claude_sessions, codex_sessions: $codex_sessions,
    symphony_runs: $symphony_runs, power: $power}' > "$snap"

# ---- digest: headless codex over the snapshot --------------------------

# Same secret scrub as insights.sh: ExecRunner inherits the full BEAM env;
# codex only needs its own API key.
# The judge is the claude harness on fable at high effort. Plain `claude`
# from PATH (the pack unit provides upstream claude-code), NOT the
# operator's ~/.local/bin wrapper: the wrapper bakes ix-mcp bootstrap
# argv that wedges and chats on stdout in unattended launchd runs, the
# same reason insights.sh pins plain codex. No tools: the whole world it
# needs is in the prompt, so --allowedTools "" doubles as the read-only
# sandbox. The -p JSON envelope is checked before the reply is trusted:
# an is_error envelope carries a partial .result (the 04:30Z truncation),
# which must fail the tick with the envelope's own error.
# --settings '{"hooks":{}}': the tick judge is a pure function call, not an
# agent session. Without it the nix claude wrapper injects the default
# settings (Stop hooks included), so every tick's meta-transcript was sliced
# by the friction-report hook and fed to the extractor (index#2275).
(
  cd "$workdir"
  env -u SLACK_BOT_OAUTH_TOKEN -u SLACK_SIGNING_SECRET \
    -u GH_TOKEN -u GITHUB_TOKEN -u GITHUB_WEBHOOK_SECRET \
    -u LINEAR_API_KEY -u LINEAR_WEBHOOK_SECRET \
    -u SYMPHONY_GITHUB_APP_PRIVATE_KEY_BASE64 -u SYMPHONY_ROOM_REGISTRY_TOKEN \
    claude -p --model fable --effort high \
    --allowedTools "" --output-format json \
    --settings '{"hooks":{}}' \
    "$(cat "$prompt_file")

Your notes from previous ticks:
$(cat "$notes")

Snapshot ($now_iso):
$(cat "$snap")" </dev/null > "$last_msg.envelope"
  jq -er 'if .is_error then error("claude -p errored: " + (.subtype // "unknown")) else .result end' \
    "$last_msg.envelope" > "$last_msg"
)

# The reply must be the {digest, attention, agents, notes} JSON object;
# anything else fails the run loudly rather than publishing a garbled page.
report_file="$(mktemp)"
tr -d '\000-\010\013\014\016-\037' < "$last_msg" > "$last_msg.clean"
mv "$last_msg.clean" "$last_msg"
# On a bad reply, keep the raw bytes as evidence before failing loudly.
if ! jq -e . "$last_msg" > /dev/null 2>&1; then
  cp "$last_msg" "$state_dir/last-reply.rejected"
  echo "overseer: reply is not valid JSON; saved to $state_dir/last-reply.rejected" >&2
  exit 5
fi
jq -e '{digest: .digest, attention: (.attention // []), agents: (.agents // [])}' "$last_msg" > "$report_file"
jq -er '.notes' "$last_msg" > "$notes"

# Act on "fix" items: dispatch one background claude fixer per new
# problem (keyed by title, 6h dedupe via dispatched.json) and record the
# handle so the page can say what is being done. Dispatch is best-effort
# observability of the overseer acting, never a reason to fail the tick,
# so a spawn failure is recorded as such rather than aborting the report.
dispatched="$state_dir/dispatched.json"
[ -f "$dispatched" ] || echo '{}' > "$dispatched"
now_epoch="$(date +%s)"
while IFS=$'\t' read -r key title action; do
  [ -n "$key" ] || continue
  last="$(jq -r --arg k "$key" '.[$k].at // 0' "$dispatched")"
  if [ $((now_epoch - last)) -lt 21600 ]; then continue; fi
  agent_name="overseer-fix-$(printf '%s' "$key" | head -c 12)"
  # </dev/null: claude reads stdin, which inside this while-read loop is
  # the remaining TSV problem rows; without it the first dispatch swallows
  # every later row (index#2157). cd "$HOME": fixers inherit this script's
  # cwd (the pack dir), so their sessions showed up in later snapshots as
  # claude sessions inside workflows/indexable and the judge re-diagnosed
  # its own just-spawned fixers as a silent workflow agent (index#2188).
  session=""
  if out="$(cd "$HOME" && "$HOME/.local/bin/claude" --bg -p "You are $agent_name, dispatched by the overseer. Problem: $title. Suggested action: $action. Investigate, fix it properly (worktree + PR when it is a repo change), and report." </dev/null 2>&1)"; then
    # `claude --bg` prints "backgrounded · <session-id>". That id plus the
    # exact $agent_name label IS the dispatch handle; record it verbatim.
    # Tick-time tracking joins on this recorded handle, never on a label
    # the judge restates in its own notes: a self-invented label was
    # declared "never materialized" for two ticks while the real fixer
    # ran and finished, and a duplicate was dispatched (index#2288).
    session="$(printf '%s\n' "$out" | awk '/backgrounded/ {print $NF; exit}')"
    note="dispatched $agent_name session=${session:-unknown}"
  else
    note="dispatch failed: $(printf '%s' "$out" | head -c 120)"
  fi
  jq --arg k "$key" --argjson at "$now_epoch" --arg note "$note" \
    --arg agent "$agent_name" --arg session "$session" \
    '.[$k] = {at: $at, note: $note, agent: $agent,
              session: (if $session == "" then null else $session end)}' \
    "$dispatched" > "$dispatched.tmp"
  mv "$dispatched.tmp" "$dispatched"
done < <(jq -r '.attention[]? | select(.severity == "fix")
  | [(.title | ascii_downcase | gsub("[^a-z0-9]+"; "-")), .title, .action] | @tsv' "$report_file")

# Append the dispatch ledger to the notes mechanically. The notes are
# otherwise model-authored, and a restated label drifts (index#2288), so
# the authoritative handles (exact label + spawned session id) are
# re-derived from dispatched.json on every tick and appended after the
# judge's own text; the next tick joins fixer tracking on these handles.
ledger="$(jq -r --argjson now "$now_epoch" '
  to_entries
  | map(select($now - .value.at < 86400))
  | sort_by(-.value.at)
  | .[] | "- \(.value.at | todate) \(.value.note) [key: \(.key)]"' "$dispatched")"
if [ -n "$ledger" ]; then
  printf '\n\nDISPATCH LEDGER (machine-written by overseer.sh; the authoritative record of dispatched fixers -- do not restate):\n%s\n' "$ledger" >> "$notes"
fi

# fold the dispatch notes into the report the page renders
jq --slurpfile d "$dispatched" '.attention = [.attention[]?
  | .dispatched = ($d[0][(.title | ascii_downcase | gsub("[^a-z0-9]+"; "-"))].note // null)]' \
  "$report_file" > "$report_file.tmp"
mv "$report_file.tmp" "$report_file"

# Snapshot history for the drill-down trail (3 days at 10-min ticks).
cp "$snap" "$state_dir/snapshots/$(date +%Y%m%dT%H%M%S).json"
ls -1 "$state_dir/snapshots" | sort | head -n -432 2>/dev/null | while read -r f; do
  rm -f "$state_dir/snapshots/$f"
done

# ---- render: data.json spliced into the Svelte report template ---------

# The compiled report app (template.html + bundle.js) comes from the
# overseer-report nix package; the symphony home module wires its store
# path through OVERSEER_APP. No app, no page: fail loudly.
[ -d "${OVERSEER_APP:?OVERSEER_APP must point at the overseer-report package}" ]

# Trend history from the archived snapshots (last 48 ticks = 8h).
history="$(ls -1 "$state_dir/snapshots" | sort | tail -48 | while read -r f; do
  jq -c '{ts: .now, load: .load_1m, cpu: (.cpu_pct // 0), mem: (.mem_pct // 0),
          sessions: (.claude_sessions | length),
          stuck: (.stalled_suspects | length)}' "$state_dir/snapshots/$f"
done | jq -s '.')"

data_json="$(mktemp)"
jq -n \
  --arg now "$now_iso" --arg loadavg "$loadavg" --argjson ncpu "$ncpu" \
  --argjson cpu_pct "$cpu_pct" --argjson mem_pct "$mem_pct" \
  --slurpfile report "$report_file" --rawfile notes_text "$notes" \
  --argjson history "$history" --argjson runs "$symphony_runs" \
  '{generated_at: $now, load_1m: ($loadavg | tonumber), ncpu: $ncpu, cpu_pct: $cpu_pct, mem_pct: $mem_pct,
    report: $report[0], history: $history,
    runs: (if ($runs | type) == "array" then $runs else [] end),
    notes: $notes_text}' > "$data_json"

first_run=0
[ -f "$html" ] || first_run=1

# Splice the JSON over the template marker byte-exactly (no regex, so the
# payload cannot corrupt the substitution), escaping "<" so transcript
# text can never close the <script> tag early.
tmp_html="$(mktemp "$state_dir/.index.XXXXXX")"
sed 's/</\\u003c/g' "$data_json" | perl -0777 -e '
  local $/;
  open my $t, "<", $ARGV[0] or die "template: $!";
  my $html = <$t>;
  my $json = <STDIN>;
  my $m = "__OVERSEER_DATA__";
  my $i = index($html, $m);
  die "marker $m missing from template" if $i < 0;
  substr($html, $i, length($m)) = $json;
  print $html;
' "$OVERSEER_APP/template.html" > "$tmp_html"
mv "$tmp_html" "$html"
cp -f "$OVERSEER_APP/bundle.js" "$state_dir/bundle.js"

# Surface the page the first time it exists; later ticks update in place
# (the meta refresh keeps an open tab current without re-stealing focus).
if [ "$first_run" = 1 ]; then
  /usr/bin/open "$html"
fi

jq -n --arg path "$html" '{overseer_page: $path}' > "$SYMPHONY_OUTPUT_FILE"
