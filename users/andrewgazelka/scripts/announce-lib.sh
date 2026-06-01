# Sourced library for pr-watch (see users/andrewgazelka/home.nix). Not a
# standalone program: no shebang / `set` line; it is `source`d into a mkBashApp
# that already supplies bash + `set -euo pipefail` and bakes claude/coreutils/
# say-detached onto PATH.
#
# Factors out the "isolated headless `claude -p` summarize -> speak it with a
# Minecraft sound" block the stage-1 CI-failure announcement uses. Call
# announce() with a system prompt, context, sound, and fallback.

# announce <sound> <system_prompt> <user_prompt> <context> <fallback> [model]
#
# Runs an isolated, headless claude (no settings/memory/hooks, no tools) over
# <context> with <system_prompt>/<user_prompt>, then speaks the result with
# say-detached and <sound>. Falls back to <fallback> if claude returns nothing.
# <model> defaults to the fast haiku model used elsewhere. Echoes "SAY: ..." for
# the agent log. Detaches via say-detached so a launchd/systemd reload can't clip
# the speech.
announce() {
  local sound="$1" sys="$2" user="$3" ctx="$4" fallback="$5"
  local model="${6:-claude-haiku-4-5-20251001}"
  local summary

  # Isolated, headless summarizer: no settings/memory/hooks, no tools.
  summary="$(cd "$HOME" && printf '%s' "$ctx" \
    | timeout 90 claude -p \
        --model "$model" \
        --allowedTools "" \
        --setting-sources "" \
        --system-prompt "$sys" \
        "$user" 2>/dev/null)" || summary=""

  [ -n "$summary" ] || summary="$fallback"

  echo "SAY: $summary"
  say-detached "$sound" "$summary"
}
