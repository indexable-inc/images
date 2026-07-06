#!/usr/bin/env bash
# Daily issue triage: run a headless codex agent that closes clear duplicate
# issues across index and ix via gh, then hand its final message to the
# runtime as the structured output {"slack_summary": ...}. The reserved
# "slack_summary" key is what IR.RunNotifier posts to Slack, so delivery
# stays owned by the notifier and this script never touches the Slack API.
set -euo pipefail

# Exec nodes run with the pack directory as cwd (ExecRunner), so the prompt
# resolves pack-relative before we move into the repo.
prompt_file="$PWD/prompts/triage.md"
last_msg="$(mktemp)"

# Hard crashes (SIGKILL, reboot) skip the EXIT trap, so reap any worktree a
# previous run leaked before adding a new one.
git -C "$SYMPHONY_PRIMARY_REPO" worktree prune

# The agent works in a detached worktree of HEAD rather than the mutable
# checkout so untracked files (.env, local scratch) stay out of its default
# cwd view. That is defense-in-depth, not a read boundary: codex's sandbox
# restricts writes, not absolute-path reads (openai/codex#5237), so the env
# scrub below is the real control. The worktree is torn down whether the
# agent succeeds or not.
triage_root="$(mktemp -d)"
cleanup() {
  git -C "$SYMPHONY_PRIMARY_REPO" worktree remove --force "$triage_root/repo" || true
  rm -rf "$triage_root" "$last_msg"
}
trap cleanup EXIT
git -C "$SYMPHONY_PRIMARY_REPO" worktree add --detach "$triage_root/repo" HEAD

# Unlike insights, this agent must close issues, so GH_TOKEN/GITHUB_TOKEN
# stay in the env (ExecRunner injects a GitHub App installation token as
# GH_TOKEN when the app is configured; otherwise gh falls back to ambient
# auth). Every other secret the symphony unit env carries is stripped.
# Sandbox: gh writes need the network and gh's own config/state writes, so
# read-only is out; workspace-write with network on is the least-privileged
# mode that permits them. Codex's default env scrub would drop *TOKEN* from
# model-spawned shells and starve gh of its credential, so the scrub is
# rebuilt by hand: keep *TOKEN* (only GH_TOKEN/GITHUB_TOKEN survive the
# env -u above) while still excluding *KEY*/*SECRET* so the model API key
# never reaches agent shells. --ignore-user-config keeps a host config.toml
# from widening any of this.
(
  cd "$triage_root/repo"
  env -u SLACK_BOT_OAUTH_TOKEN -u SLACK_SIGNING_SECRET \
    -u GITHUB_WEBHOOK_SECRET \
    -u LINEAR_API_KEY -u LINEAR_WEBHOOK_SECRET \
    -u SYMPHONY_GITHUB_APP_PRIVATE_KEY_BASE64 -u SYMPHONY_ROOM_REGISTRY_TOKEN \
    codex exec --sandbox workspace-write \
    -c sandbox_workspace_write.network_access=true \
    -c shell_environment_policy.ignore_default_excludes=true \
    -c 'shell_environment_policy.exclude=["*KEY*","*SECRET*"]' \
    --ignore-user-config \
    --output-last-message "$last_msg" \
    "$(cat "$prompt_file")" </dev/null # codex reads a non-tty stdin to EOF; the runner pipe never closes (#2011)
)

# An empty final message means nothing postable; fail the node loudly so
# the failure notification fires instead of a silent empty digest.
[ -s "$last_msg" ]

jq -n --rawfile summary "$last_msg" '{slack_summary: $summary}' > "$SYMPHONY_OUTPUT_FILE"
