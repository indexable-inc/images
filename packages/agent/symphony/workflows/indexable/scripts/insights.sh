#!/usr/bin/env bash
# Daily insights digest: run a read-only headless codex agent over the
# primary repository and hand its final message to the runtime as the
# structured output {"summary": ...}. The reserved "summary" key is what
# IR.RunNotifier posts to Slack, so delivery stays owned by the notifier
# and this script never touches the Slack API.
set -euo pipefail

# Exec nodes run with the pack directory as cwd (ExecRunner), so the prompt
# resolves pack-relative before we cd into the repo.
prompt_file="$PWD/prompts/insights.md"
last_msg="$(mktemp)"
trap 'rm -f "$last_msg"' EXIT

# Strip the Slack secret: the agent is open-ended, and posting is the
# notifier's job, so it must not be able to speak as the bot.
(
  cd "$SYMPHONY_PRIMARY_REPO"
  env -u SLACK_BOT_OAUTH_TOKEN \
    codex exec --sandbox read-only --output-last-message "$last_msg" \
    "$(cat "$prompt_file")"
)

# An empty final message means nothing postable; fail the node loudly so
# the failure notification fires instead of a silent empty digest.
[ -s "$last_msg" ]

jq -n --rawfile summary "$last_msg" '{summary: $summary}' > "$SYMPHONY_OUTPUT_FILE"
