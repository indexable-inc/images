#!/usr/bin/env bash
# Local smoke + negative check for the Pi harness (ENG-2262 validation).
#
# Runs the SHIPPED artifact - builds `.#pi-harness` and runs its `bin/pi-harness`
# - so it exercises the generated Nushell wrapper, the model-alias table, and the
# Nix-packaged bridge (with its bundled node_modules), not a hand-rolled pi call.
#
# Proves three things from the ticket without Room:
#   1. one prompt runs through the packaged harness,
#   2. the model has NO built-in bash/read/write/edit tools (absent, not denied),
#      only the ix-mcp surface,
#   3. the turn produces a stable JSON event stream.
#
# Needs network + an API key for the selected model (ANTHROPIC_API_KEY by
# default; set PI_HARNESS_MODEL=codex + OPENAI_API_KEY for gpt-5.5). `pi` must be
# on PATH. Run it yourself - first build can exceed a couple of minutes.
#
#   ANTHROPIC_API_KEY=... ./packages/pi-harness/smoke/run.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/../../.." && pwd)"

# 1. Build ix-mcp and expose it to the bridge (it spawns `ix-mcp serve`).
echo "[smoke] building ix-mcp..." >&2
mcp_out="$(nix build "$repo_root#mcp" --no-link --print-out-paths --accept-flake-config)"
export IX_MCP_BIN="$mcp_out/bin/ix-mcp"
[ -x "$IX_MCP_BIN" ] || { echo "[smoke] ix-mcp binary not found at $IX_MCP_BIN" >&2; exit 1; }

# 2. Build the shipped harness (this packages the bridge + its node_modules).
echo "[smoke] building pi-harness..." >&2
harness="$(nix build "$repo_root#pi-harness" --no-link --print-out-paths --accept-flake-config)"
[ -x "$harness/bin/pi-harness" ] || { echo "[smoke] pi-harness not built" >&2; exit 1; }

# 3. Run one prompt through the packaged wrapper and capture its JSON events.
events="$(mktemp)"
trap 'rm -f "$events"' EXIT
echo "[smoke] running one turn through bin/pi-harness..." >&2
"$harness/bin/pi-harness" "What is 2+2? Compute it with python_exec." | tee "$events" >&2

# 4. Assertions. agent_start + the model's tool list ride the JSON stream.
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
  echo "[smoke] PASS: built-ins absent, ix-mcp exposed, JSON events emitted via the shipped artifact" >&2
fi
exit "$fail"
