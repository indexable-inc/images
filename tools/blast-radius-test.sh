#!/usr/bin/env bash
# Tests the blast-radius PR comment end to end:
#   * the security-critical validate + render jq embedded in
#     .github/workflows/blast-radius.yml (extracted from the workflow so the test
#     can never drift from what the trusted comment job actually runs), and
#   * the report-building logic in tools/blast-radius.nu (categories + the
#     cause/fan-out reference diff), with a stubbed `nix-store`.
# Needs jq, yq (yq-go), and nu on PATH.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixtures="$here/blast-radius-fixtures"
workflow="$here/../.github/workflows/blast-radius.yml"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fail=0
note() { printf '  %s\n' "$*"; }

# Extract the exact run-scripts the trusted comment job executes.
yq '.jobs.comment.steps[] | select(.name == "Validate report schema").run' "$workflow" > "$tmp/validate.sh"
yq '.jobs.comment.steps[] | select(.name == "Render comment").run' "$workflow" > "$tmp/render.sh"

validate() { ( cd "$tmp" && cp "$1" report.json && bash validate.sh ); }

# Schema validation: the good report passes; hostile names and old (missing
# categories/causes) reports are rejected fail-closed.
if validate "$fixtures/good.json" >/dev/null 2>&1; then note "validate good: ok"; else note "validate good: FAIL"; fail=1; fi
for bad in bad-name bad-check missing; do
  if validate "$fixtures/$bad.json" >/dev/null 2>&1; then
    note "validate $bad: FAIL (accepted hostile/old report)"; fail=1
  else
    note "validate $bad: rejected ok"
  fi
done

# Render: the good report produces the golden comment (pie + flowchart + list).
( cd "$tmp" && cp "$fixtures/good.json" report.json && bash render.sh )
if diff -u "$fixtures/good.expected.md" "$tmp/comment.md"; then note "render good: ok"; else note "render good: FAIL (output drift)"; fail=1; fi

# Report-building logic with a stubbed nix-store (head vs base refs differ only
# in the ix-rust-workspace hash, so it is the changed root cause). The shebang
# points at the bash already running this script (`command -v bash`) instead of
# `/usr/bin/env bash`: this test runs inside the nix build sandbox, which has no
# `/usr/bin/env`, so a hard-coded env shebang silently fails to exec, the stub
# produces no refs, and `causes-for` returns [] (passes on a dev box, fails in
# CI). Resolving bash from PATH execs in both places.
{
  printf '#!%s\n' "$(command -v bash)"
  cat <<'STUB'
drv="${!#}"
case "$drv" in
  *head-rust-*) echo /nix/store/h1111111111111111111111111111111-ix-rust-workspace.drv
                echo /nix/store/g2222222222222222222222222222222-glibc.drv ;;
  *base-rust-*) echo /nix/store/b0000000000000000000000000000000-ix-rust-workspace.drv
                echo /nix/store/g2222222222222222222222222222222-glibc.drv ;;
esac
STUB
} > "$tmp/nix-store"
chmod +x "$tmp/nix-store"
# Run via `-c "source ..."`, not `nu logic-test.nu`: executing a file auto-runs
# any `main` in scope, and the test sources blast-radius.nu (which defines one),
# so a plain file run would fire the real report build. `-c` does not auto-run.
if PATH="$tmp:$PATH" nu --no-config-file -c "source '$fixtures/logic-test.nu'"; then note "logic-test: ok"; else note "logic-test: FAIL"; fail=1; fi

if [ "$fail" -ne 0 ]; then echo "blast-radius-test: FAILED"; exit 1; fi
echo "blast-radius-test: all passed"
