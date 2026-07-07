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
cleanup() { rm -rf "$last_msg" "$workdir" "${report_file:-}" "${data_json:-}"; }
trap cleanup EXIT

now_iso="$(date +%Y-%m-%dT%H:%M:%S%z)"

# ---- collectors: each emits JSON on stdout ----------------------------

# Every process, parsed into fields once; agent/hot/stall views are jq
# filters over this. tty="??" separates headless (launchd, cron, exec
# nodes) from interactive terminal sessions.
ps_json="$(ps axo pid=,pcpu=,etime=,tty=,args= | jq -Rs '
  [split("\n")[]
   | capture("^ *(?<pid>[0-9]+) +(?<pcpu>[0-9.]+) +(?<etime>[0-9:-]+) +(?<tty>[^ ]+) +(?<args>.*)$")?
   | .pid |= tonumber | .pcpu |= tonumber
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

hot="$(jq '[.[] | select(.pcpu >= 30)] | sort_by(-.pcpu) | .[0:8]' <<<"$ps_json")"

# Wedge heuristic (#2011 signature): a headless agent process idling at
# ~0% CPU for a long time. Interactive agents idle at 0% legitimately
# (waiting on the human), so only tty-less processes qualify.
stalled="$(jq '[.[] | select(.tty == "??" and .pcpu < 1 and .minutes >= 15)]' <<<"$agents")"

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

# Codex sessions active in the last 12h (rollout transcripts).
codex_sessions="$(
  find "$HOME/.codex/sessions" -name '*.jsonl' -mmin -720 -print0 2>/dev/null |
  perl -0ne 'my @s = stat($_); print "$s[9] $_\n" if @s' | sort -rn | awk 'NR <= 10' |
  awk -v now="$(date +%s)" '{printf "{\"last_active\": %d, \"age_min\": %d, \"path\": \"%s\"}\n", $1, (now - $1) / 60, $2}' |
  jq -s '[.[] | .last_active |= todate]'
)"

# Symphony runs from the local runtime. An unreachable API is itself a
# finding, so the failure is recorded in the snapshot, never swallowed.
symphony_runs="$(curl -s --max-time 5 http://127.0.0.1:4040/api/v1/ir/runs |
  jq '[.runs[] | {run_id, status, states, trigger, created_at, updated_at}] | .[0:12]' ||
  echo '{"error": "symphony runtime unreachable on 127.0.0.1:4040"}')"

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
  --argjson symphony_runs "$symphony_runs" \
  '{now: $now, load_1m: ($loadavg | tonumber), ncpu: $ncpu, cpu_pct: $cpu_pct, mem_pct: $mem_pct,
    agent_processes: $agents, hot_processes: $hot, stalled_suspects: $stalled,
    claude_sessions: $claude_sessions, codex_sessions: $codex_sessions,
    symphony_runs: $symphony_runs}' > "$snap"

# ---- digest: headless codex over the snapshot --------------------------

# Same secret scrub as insights.sh: ExecRunner inherits the full BEAM env;
# codex only needs its own API key.
(
  cd "$workdir"
  env -u SLACK_BOT_OAUTH_TOKEN -u SLACK_SIGNING_SECRET \
    -u GH_TOKEN -u GITHUB_TOKEN -u GITHUB_WEBHOOK_SECRET \
    -u LINEAR_API_KEY -u LINEAR_WEBHOOK_SECRET \
    -u SYMPHONY_GITHUB_APP_PRIVATE_KEY_BASE64 -u SYMPHONY_ROOM_REGISTRY_TOKEN \
    codex exec --sandbox read-only --ignore-user-config \
    --skip-git-repo-check \
    --output-last-message "$last_msg" \
    "$(cat "$prompt_file")

Your notes from previous ticks:
$(cat "$notes")

Snapshot ($now_iso):
$(cat "$snap")" </dev/null # codex reads a non-tty stdin to EOF; the runner pipe never closes (#2011)
)

# The reply must be the {digest, attention, agents, notes} JSON object;
# anything else fails the run loudly rather than publishing a garbled page.
report_file="$(mktemp)"
tr -d '\000-\010\013\014\016-\037' < "$last_msg" > "$last_msg.clean"
mv "$last_msg.clean" "$last_msg"
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
  if out="$("$HOME/.local/bin/claude" --bg -p "You are $agent_name, dispatched by the overseer. Problem: $title. Suggested action: $action. Investigate, fix it properly (worktree + PR when it is a repo change), and report." 2>&1)"; then
    note="dispatched $agent_name"
  else
    note="dispatch failed: $(printf '%s' "$out" | head -c 120)"
  fi
  jq --arg k "$key" --argjson at "$now_epoch" --arg note "$note" \
    '.[$k] = {at: $at, note: $note}' "$dispatched" > "$dispatched.tmp"
  mv "$dispatched.tmp" "$dispatched"
done < <(jq -r '.attention[]? | select(.severity == "fix")
  | [(.title | ascii_downcase | gsub("[^a-z0-9]+"; "-")), .title, .action] | @tsv' "$report_file")

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
