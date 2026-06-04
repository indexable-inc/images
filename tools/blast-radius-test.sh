#!/usr/bin/env bash
# Tests the security-critical validate + render jq embedded in
# .github/workflows/blast-radius.yml (extracted from the workflow so the test can
# never drift from what the trusted comment job actually runs). The report-
# building logic itself lives in the `blast-radius` Rust crate and is covered by
# its own unit tests (packages/blast-radius/src/causes.rs).
# Needs jq and yq (yq-go) on PATH.
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

if [ "$fail" -ne 0 ]; then echo "blast-radius-test: FAILED"; exit 1; fi
echo "blast-radius-test: all passed"
