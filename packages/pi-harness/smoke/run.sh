#!/usr/bin/env bash
# Local smoke + negative check for the Pi harness (ENG-2262 validation).
#
# Proves three things from the ticket without Room:
#   1. one prompt runs through the harness,
#   2. the model has NO built-in bash/read/write/edit tools (they are absent,
#      not denied), only the ix-mcp surface,
#   3. the turn produces a stable JSON event stream.
#
# Needs network + an API key for the selected model (ANTHROPIC_API_KEY by
# default). Run it yourself - it can exceed a couple of minutes on first build.
#
#   ANTHROPIC_API_KEY=... ./packages/pi-harness/smoke/run.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pkg="$(dirname "$here")"
repo_root="$(cd "$pkg/../.." && pwd)"

# 1. Build ix-mcp and expose it to the bridge (it spawns `ix-mcp serve`).
echo "[smoke] building ix-mcp..." >&2
mcp_out="$(nix build "$repo_root#mcp" --no-link --print-out-paths)"
export IX_MCP_BIN="$mcp_out/bin/ix-mcp"
[ -x "$IX_MCP_BIN" ] || { echo "[smoke] ix-mcp binary not found at $IX_MCP_BIN" >&2; exit 1; }

# 2. Resolve the bridge extension's npm deps (dev-time; pure-nix is a follow-up).
if [ ! -d "$pkg/extension/node_modules" ]; then
  echo "[smoke] installing bridge npm deps..." >&2
  (cd "$pkg/extension" && npm install --omit=dev --silent)
fi

# 3. Run one prompt through Pi with the harness posture and capture JSON events.
events="$(mktemp)"
trap 'rm -f "$events"' EXIT
echo "[smoke] running one turn through pi..." >&2
pi \
  --no-builtin-tools --no-extensions --no-skills --no-session \
  --mode json --print \
  --provider "${PI_PROVIDER:-anthropic}" \
  --model "${PI_MODEL:-claude-opus-4-8}" \
  --system-prompt "Use python_exec for everything." \
  -e "$pkg/extension/ix-mcp-bridge.ts" \
  "What is 2+2? Compute it with python_exec." | tee "$events" >&2

# 4. Assertions. agent_start carries the active tool list.
echo "[smoke] checking tool surface..." >&2
fail=0
for forbidden in '"bash"' '"read"' '"write"' '"edit"'; do
  if grep -q "$forbidden" "$events"; then
    echo "[smoke] FAIL: built-in tool $forbidden present in stream" >&2
    fail=1
  fi
done
grep -q "python_exec" "$events" || { echo "[smoke] FAIL: python_exec not exposed" >&2; fail=1; }
grep -q '"type":"turn_' "$events" || { echo "[smoke] FAIL: no turn lifecycle events" >&2; fail=1; }

if [ "$fail" -eq 0 ]; then
  echo "[smoke] PASS: built-ins absent, ix-mcp exposed, JSON events emitted" >&2
fi
exit "$fail"
