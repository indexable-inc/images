#!/usr/bin/env bash
# Overseer tick: collect a JSON snapshot of agent activity on this host,
# ask a read-only headless codex for a terse digest, and regenerate the
# HTML report. The script owns the collectors and the page markup; the
# agent only authors the digest text (a read-only sandbox cannot write
# the page anyway).
set -euo pipefail

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
cleanup() { rm -rf "$last_msg" "$workdir" "${digest_file:-}"; }
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

jq -n \
  --arg now "$now_iso" --arg loadavg "$loadavg" --argjson ncpu "$ncpu" \
  --argjson agents "$agents" --argjson hot "$hot" --argjson stalled "$stalled" \
  --argjson claude_sessions "$claude_sessions" --argjson codex_sessions "$codex_sessions" \
  --argjson symphony_runs "$symphony_runs" \
  '{now: $now, load_1m: ($loadavg | tonumber), ncpu: $ncpu,
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

# The reply must be the {digest, notes} JSON object; anything else fails
# the run loudly rather than publishing a garbled page.
digest_file="$(mktemp)"
jq -er '.digest' "$last_msg" > "$digest_file"
jq -er '.notes' "$last_msg" > "$notes"

# Snapshot history for the drill-down trail (3 days at 10-min ticks).
cp "$snap" "$state_dir/snapshots/$(date +%Y%m%dT%H%M%S).json"
ls -1 "$state_dir/snapshots" | sort | head -n -432 2>/dev/null | while read -r f; do
  rm -f "$state_dir/snapshots/$f"
done

# ---- render: digest on top, one <details> row per item -----------------

first_run=0
[ -f "$html" ] || first_run=1

tmp_html="$(mktemp "$state_dir/.index.XXXXXX")"
{
  cat <<'EOF'
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="refresh" content="120">
<title>overseer</title>
<style>
  body { max-width: 46rem; margin: 3rem auto; padding: 0 1rem;
         font: 15px/1.55 -apple-system, "Helvetica Neue", sans-serif;
         color: #1a1a1a; background: #fcfcfa; }
  h1 { font-size: 1rem; font-weight: 600; letter-spacing: 0.02em; }
  h2 { font-size: 0.8rem; font-weight: 600; color: #8a8a86;
       text-transform: uppercase; letter-spacing: 0.06em; margin: 2rem 0 0.5rem; }
  .digest { white-space: pre-wrap; margin: 1rem 0 0; }
  .meta { font-size: 0.8rem; color: #8a8a86; font-variant-numeric: tabular-nums; }
  details { border-top: 1px solid #e6e6e2; padding: 0.4rem 0; }
  summary { cursor: pointer; }
  summary .n { color: #8a8a86; font-size: 0.85rem; font-variant-numeric: tabular-nums; }
  pre { font-size: 12px; overflow-x: auto; background: #f4f4f0; padding: 0.6rem; }
  .warn summary { color: #a04000; }
  a { color: inherit; }
  @media (prefers-color-scheme: dark) {
    body { color: #e6e6e2; background: #161615; }
    details { border-color: #2c2c2a; }
    pre { background: #1f1f1d; }
    .warn summary { color: #e0956a; }
  }
</style>
</head>
<body>
EOF

  jq -r --rawfile digest "$digest_file" --rawfile notes "$notes" '
    def esc: @html;
    def row(cls; head; body): "<details class=\"\(cls)\"><summary>\(head)</summary><pre>\(body | esc)</pre></details>";

    "<h1>overseer</h1>",
    "<p class=\"meta\">updated \(.now | esc) · load \(.load_1m)/\(.ncpu) · \(.agent_processes | length) agent processes · \(.claude_sessions | length) claude sessions · \(.hot_processes | length) hot · \(.stalled_suspects | length) stalled suspects</p>",
    "<p class=\"digest\">\($digest | esc)</p>",

    (if (.stalled_suspects | length) > 0 then
      "<h2>stalled suspects</h2>",
      (.stalled_suspects[] | row("warn"; "pid \(.pid) · \(.etime) at \(.pcpu)% <span class=\"n\">headless</span>"; (. | tojson)))
    else empty end),

    (if (.hot_processes | length) > 0 then
      "<h2>hot processes</h2>",
      (.hot_processes[] | row(""; "\(.pcpu)% · pid \(.pid) · \(.args[0:80] | esc)"; (. | tojson)))
    else empty end),

    "<h2>agent processes</h2>",
    (if (.agent_processes | length) == 0 then "<p class=\"meta\">none</p>" else
      (.agent_processes[] | row(""; "pid \(.pid) · \(.pcpu)% · up \(.etime) · \(.args[0:80] | esc)"; (. | tojson))) end),

    "<h2>claude sessions (12h)</h2>",
    (if (.claude_sessions | length) == 0 then "<p class=\"meta\">none</p>" else
      (.claude_sessions[] | row(""; "\(.cwd | esc) <span class=\"n\">\(.last_active | esc)</span>"; .last_text)) end),

    "<h2>overseer notes</h2>",
    row(""; "working notes carried to the next tick"; $notes),

    "<h2>symphony runs</h2>",
    (if (.symphony_runs | type) != "array" then "<p class=\"meta\">\(.symphony_runs.error // "unavailable" | esc)</p>" else
      (.symphony_runs[] | "<details class=\"\(if .status == "failed" then "warn" else "" end)\"><summary>\(.run_id | esc) · \(.status | esc) <span class=\"n\">\(.updated_at | esc)</span></summary><pre>\(. | tojson | esc)</pre><p class=\"meta\"><a href=\"http://127.0.0.1:4040/ir/\(.run_id)\">full run detail</a></p></details>") end)
  ' "$snap"

  printf '</body>\n</html>\n'
} > "$tmp_html"
mv "$tmp_html" "$html"

# Surface the page the first time it exists; later ticks update in place
# (the meta refresh keeps an open tab current without re-stealing focus).
if [ "$first_run" = 1 ]; then
  /usr/bin/open "$html"
fi

jq -n --arg path "$html" '{overseer_page: $path}' > "$SYMPHONY_OUTPUT_FILE"
