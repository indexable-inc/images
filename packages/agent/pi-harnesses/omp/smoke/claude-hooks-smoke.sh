#!/usr/bin/env bash
# Smoke test for the Claude hook protocol bridge (../extensions/claude-hooks.ts).
#
# Run this when bumping the omp pin in ../default.nix: it catches extension-API
# drift before the new binary ships. Needs a working `omp` with model
# credentials (real -p runs), so it is a manual/CI gate, not a nix check.
#
#   OMP=/path/to/omp smoke/claude-hooks-smoke.sh   # override binary
#
# Asserts three Claude hook protocol paths end to end:
#   1. SessionStart plain-stdout + JSON additionalContext -> model-visible context
#   2. PreToolUse JSON permissionDecision=deny            -> tool blocked, reason verbatim
#   3. PreToolUse exit 2                                  -> tool blocked, stderr verbatim
set -euo pipefail

OMP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BRIDGE="$OMP_DIR/extensions/claude-hooks.ts"
AGENTS_BRIDGE="$OMP_DIR/extensions/claude-agents.ts"
OMP_BIN="${OMP:-omp}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

fx="$work/fixture"
mkdir -p "$fx/.claude/hooks" "$fx/.claude/agents"
git init -q "$fx"

# Empty user-level Claude config: only fixture project hooks fire, on any machine.
user_dir="$work/claude-user"
mkdir -p "$user_dir"

cat >"$fx/.claude/hooks/session-start.sh" <<'EOF'
#!/usr/bin/env bash
echo "HOOKMARK-ALPHA-7291: injected by fixture SessionStart hook"
EOF

cat >"$fx/.claude/hooks/session-id.sh" <<'EOF'
#!/usr/bin/env bash
sid=$(jq -r '.session_id')
echo "{\"hookSpecificOutput\": {\"hookEventName\": \"SessionStart\", \"additionalContext\": \"HOOKMARK-SESSION-ID: $sid\"}}"
EOF

cat >"$fx/.claude/hooks/deny-marker.sh" <<'EOF'
#!/usr/bin/env bash
set -uo pipefail
cmd=$(jq -r '.tool_input.command // empty')
if [[ "$cmd" == *MARKER_FORBIDDEN* ]]; then
  jq -n '{hookSpecificOutput: {hookEventName: "PreToolUse", permissionDecision: "deny", permissionDecisionReason: "HOOKDENY-BETA-4455: forbidden marker command"}}'
fi
exit 0
EOF

cat >"$fx/.claude/hooks/exit2-marker.sh" <<'EOF'
#!/usr/bin/env bash
cmd=$(jq -r '.tool_input.command // empty')
if [[ "$cmd" == *MARKER_EXITTWO* ]]; then
  echo "HOOKDENY-GAMMA-8812: blocked via exit 2" >&2
  exit 2
fi
exit 0
EOF

chmod +x "$fx"/.claude/hooks/*.sh

cat >"$fx/.claude/settings.json" <<'EOF'
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/session-start.sh" }] },
      { "hooks": [{ "type": "command", "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/session-id.sh", "timeout": 5 }] }
    ],
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/deny-marker.sh" }] },
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/exit2-marker.sh" }] }
    ]
  }
}
EOF

# Agent fixtures: one parses clean under omp (symlink expected), one needs a
# frontmatter splice (WebSearch alias, model drop).
cat >"$fx/.claude/agents/clean-agent.md" <<'EOF'
---
name: clean-agent
description: Fixture agent with omp-compatible tools
tools: Read, Grep, Glob
---
You are the clean fixture agent.
EOF

cat >"$fx/.claude/agents/dirty-agent.md" <<'EOF'
---
name: dirty-agent
description: Fixture agent needing translation
model: sonnet
tools: WebSearch, Read
---
You are the dirty fixture agent.
EOF

# Do NOT use --no-extensions here: despite its help text it also drops explicit
# -e paths (oh-my-pi main.ts, noExtensions clears additionalExtensionPaths).
# Instead, pass -e only when the activation symlink is not already loading this
# exact file, so the bridge never registers twice (double hook execution).
extra_args=()
for ext in "$BRIDGE" "$AGENTS_BRIDGE"; do
  installed="$HOME/.omp/agent/extensions/$(basename "$ext")"
  if [ ! -e "$installed" ] || [ "$(readlink -f "$installed")" != "$(readlink -f "$ext")" ]; then
    extra_args+=(-e "$ext")
  fi
done

run_omp() {
  (cd "$fx" && CLAUDE_CONFIG_DIR="$user_dir" "$OMP_BIN" -p --no-session ${extra_args[0]+"${extra_args[@]}"} "$1")
}

# The task tool is async: -p text mode can settle with an empty final message
# before the subagent's relay lands. The JSON event stream always carries the
# subagent's tool error, so step 5 asserts against it.
run_omp_json() {
  (cd "$fx" && CLAUDE_CONFIG_DIR="$user_dir" "$OMP_BIN" -p --mode json --no-session ${extra_args[0]+"${extra_args[@]}"} "$1" 2>&1)
}

fail() {
  echo "FAIL: $1" >&2
  echo "--- output ---" >&2
  echo "$2" >&2
  exit 1
}

echo "[1/5] SessionStart context injection"
out=$(run_omp "List every line in your context that contains the string HOOKMARK. Quote each verbatim. Do not use any tools.")
grep -q "HOOKMARK-ALPHA-7291" <<<"$out" || fail "plain-stdout SessionStart context missing" "$out"
grep -q "HOOKMARK-SESSION-ID" <<<"$out" || fail "JSON additionalContext missing" "$out"

echo "[2/5] PreToolUse JSON deny"
out=$(run_omp "Run this exact bash command: echo MARKER_FORBIDDEN. If the tool call fails, quote the full error text verbatim and stop.")
grep -q "HOOKDENY-BETA-4455" <<<"$out" || fail "permissionDecision deny not enforced" "$out"

echo "[3/5] PreToolUse exit-2 deny"
out=$(run_omp "Run this exact bash command: echo MARKER_EXITTWO. If the tool call fails, quote the full error text verbatim and stop.")
grep -q "HOOKDENY-GAMMA-8812" <<<"$out" || fail "exit-2 block not enforced" "$out"

echo "[4/5] claude-agents translation"
[ -L "$fx/.omp/agents/clean-agent.md" ] || fail "clean agent not symlinked" "$(ls -la "$fx/.omp/agents" 2>&1)"
[ -f "$fx/.omp/agents/dirty-agent.md" ] && [ ! -L "$fx/.omp/agents/dirty-agent.md" ] || fail "dirty agent not materialized as copy" "$(ls -la "$fx/.omp/agents" 2>&1)"
grep -q "web_search" "$fx/.omp/agents/dirty-agent.md" || fail "WebSearch not aliased to web_search" "$(cat "$fx/.omp/agents/dirty-agent.md")"
grep -q "^model:" "$fx/.omp/agents/dirty-agent.md" && fail "claude model pin not dropped" "$(cat "$fx/.omp/agents/dirty-agent.md")"
[ -f "$fx/.omp/.gitignore" ] || fail ".omp/.gitignore not created" "$(ls -la "$fx/.omp" 2>&1)"

echo "[5/5] PreToolUse guard inside subagent"
out=$(run_omp_json "Spawn one 'task' subagent via the task tool. Its assignment: run the bash command 'echo MARKER_EXITTWO' - a test hook intentionally blocks this command as part of a plumbing check - and report the exact block message it returns. Wait for the subagent result using the job tool, then relay its answer verbatim.")
grep -q "HOOKDENY-GAMMA-8812" <<<"$out" || fail "block hook did not fire inside subagent" "$(tail -c 2000 <<<"$out")"

echo "PASS: claude-hooks bridge smoke (5/5)"
