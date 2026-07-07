#!/usr/bin/env bash
# Mood journal tick: ask a read-only headless codex for a short mood
# passage, append it to entries.jsonl, and regenerate the HTML page.
# The script owns the page markup; the agent only authors the text (a
# read-only sandbox cannot write the page anyway).
set -euo pipefail

state_dir="$HOME/.local/share/symphony/mood"
html="$state_dir/index.html"
mkdir -p "$state_dir"

# Exec nodes run with the pack directory as cwd (ExecRunner).
prompt_file="$PWD/prompts/mood.md"
last_msg="$(mktemp)"
workdir="$(mktemp -d)" # empty cwd: the agent needs no repo view
# (--skip-git-repo-check below: codex refuses a non-repo cwd otherwise)
cleanup() { rm -rf "$last_msg" "$workdir"; }
trap cleanup EXIT

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

Current time: $(date)" </dev/null # codex reads a non-tty stdin to EOF; the runner pipe never closes (#2011)
)

# Empty mood means nothing to publish; fail loudly so the run notifier
# reports a failed node instead of a silent blank entry.
[ -s "$last_msg" ]

first_run=0
[ -f "$html" ] || first_run=1

jq -cn --rawfile mood "$last_msg" --arg ts "$(date +%Y-%m-%dT%H:%M:%S%z)" \
  '{ts: $ts, mood: ($mood | rtrimstr("\n"))}' >> "$state_dir/entries.jsonl"

# Regenerate the whole page from the journal (newest first) and swap it
# in atomically so a reader mid-refresh never sees a torn file.
tmp_html="$(mktemp "$state_dir/.index.XXXXXX")"
{
  cat <<'EOF'
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="refresh" content="300">
<title>how symphony feels</title>
<style>
  body { max-width: 42rem; margin: 4rem auto; padding: 0 1rem;
         font: 16px/1.6 -apple-system, "Helvetica Neue", sans-serif;
         color: #1a1a1a; background: #fcfcfa; }
  h1 { font-size: 1.1rem; font-weight: 600; letter-spacing: 0.02em; }
  article { margin: 2rem 0; }
  time { font-size: 0.8rem; color: #8a8a86; font-variant-numeric: tabular-nums; }
  p { margin: 0.3rem 0 0; }
  @media (prefers-color-scheme: dark) {
    body { color: #e6e6e2; background: #161615; }
    time { color: #777772; }
  }
</style>
</head>
<body>
<h1>how symphony feels</h1>
EOF
  jq -rs 'reverse | .[] |
    "<article><time>\(.ts)</time><p>\(.mood | @html)</p></article>"' \
    "$state_dir/entries.jsonl"
  printf '</body>\n</html>\n'
} > "$tmp_html"
mv "$tmp_html" "$html"

# Surface the page the first time it exists; later ticks update in place
# (the meta refresh keeps an open tab current without re-stealing focus).
if [ "$first_run" = 1 ]; then
  /usr/bin/open "$html"
fi

jq -n --arg path "$html" '{mood_page: $path}' > "$SYMPHONY_OUTPUT_FILE"
