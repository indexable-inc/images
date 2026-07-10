#!/usr/bin/env bash
# Daily insights digest: run a read-only headless codex agent over the
# primary repository and hand its final message to the runtime as the
# structured output {"slack_summary": ...}. The reserved "slack_summary"
# key is what IR.RunNotifier posts to Slack, so delivery stays owned by
# the notifier and this script never touches the Slack API.
set -euo pipefail

# Exec nodes run with the pack directory as cwd (ExecRunner), so the prompt
# resolves pack-relative before we move into the repo.
prompt_file="$PWD/prompts/insights.md"
last_msg="$(mktemp)"

# The agent reads a detached worktree of HEAD, not the mutable checkout,
# so untracked files (.env, local scratch) in the operator's tree are out
# of the default view. Defense-in-depth only: codex's read-only sandbox
# restricts writes, not absolute-path reads (openai/codex#5237), so the
# env scrub below is the actual secret control. The worktree is torn down
# whether the agent succeeds or not; the prune reaps predecessors leaked
# by hard crashes (SIGKILL/reboot skip the EXIT trap).
git -C "$SYMPHONY_PRIMARY_REPO" worktree prune
digest_root="$(mktemp -d)"
cleanup() {
  git -C "$SYMPHONY_PRIMARY_REPO" worktree remove --force "$digest_root/repo" || true
  rm -rf "$digest_root" "$last_msg"
}
trap cleanup EXIT
git -C "$SYMPHONY_PRIMARY_REPO" worktree add --detach "$digest_root/repo" HEAD

# Strip every secret the symphony unit env carries (ExecRunner inherits the
# full BEAM env and injects GH_TOKEN); codex only needs its own API key.
# Containment beyond that relies on codex's read-only sandbox (network off)
# and its default *KEY*/*SECRET*/*TOKEN* env scrub for model-spawned shells;
# --ignore-user-config keeps a host config.toml from re-enabling network.
(
  cd "$digest_root/repo"
  env -u SLACK_BOT_OAUTH_TOKEN -u SLACK_SIGNING_SECRET \
    -u GH_TOKEN -u GITHUB_TOKEN -u GITHUB_WEBHOOK_SECRET \
    -u LINEAR_API_KEY -u LINEAR_WEBHOOK_SECRET \
    -u SYMPHONY_GITHUB_APP_PRIVATE_KEY_BASE64 -u SYMPHONY_ROOM_REGISTRY_TOKEN \
    codex exec --sandbox read-only --ignore-user-config \
    --output-last-message "$last_msg" \
    "$(cat "$prompt_file")" </dev/null # codex reads a non-tty stdin to EOF; the runner pipe never closes (#2011)
)

# An empty final message means nothing postable; fail the node loudly so
# the failure notification fires instead of a silent empty digest.
[ -s "$last_msg" ]

jq -n --rawfile summary "$last_msg" '{slack_summary: $summary}' > "$SYMPHONY_OUTPUT_FILE"
