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

# Schema validation: the good report passes; hostile and old (missing
# causes/phaseTimings) reports are rejected fail-closed. The two
# `bad-phase-*` fixtures pin the kebab-case key constraint and the number-
# typed value constraint that keep an attacker from smuggling shapes into
# the artifact.
if validate "$fixtures/good.json" >/dev/null 2>&1; then note "validate good: ok"; else note "validate good: FAIL"; fail=1; fi
for bad in bad-name bad-check missing bad-phase-key bad-phase-value; do
  if validate "$fixtures/$bad.json" >/dev/null 2>&1; then
    note "validate $bad: FAIL (accepted hostile/old report)"; fail=1
  else
    note "validate $bad: rejected ok"
  fi
done

# Render: the good report produces the golden comment (flowchart + list).
# `phaseTimings` is observability-only and never renders, so the golden
# comment from a report carrying phaseTimings has no trace of those keys;
# any drift here means the renderer leaked them.
( cd "$tmp" && cp "$fixtures/good.json" report.json && bash render.sh )
if diff -u "$fixtures/good.expected.md" "$tmp/comment.md"; then note "render good: ok"; else note "render good: FAIL (output drift)"; fail=1; fi

# Regression (#1421): a cause name is a nix derivation name, and fixed-output
# patch/fetch drvs legally carry `?` and `=` (e.g. `<sha>.patch?full_index=1`,
# `webkitgtk-2.52.4+abi=4.1`). The validator's name_ok used to omit those two
# glyphs and fail-closed the WHOLE report, so the comment job exited 1 and posted
# no comment at all -- the "sometimes empty" symptom. The report must now both
# validate and render with the name intact (validate and safename stay lockstep).
if validate "$fixtures/special-name.json" >/dev/null 2>&1; then
  note "validate special-name: ok"
else
  note "validate special-name: FAIL (rejected a legal nix derivation name)"; fail=1
fi
( cd "$tmp" && cp "$fixtures/special-name.json" report.json && bash render.sh )
if grep -qF 'e67caa006c75181b45b761cd50294cb3c8e18f1a.patch?full_index=1' "$tmp/comment.md"; then
  note "render special-name: name preserved ok"
else
  note "render special-name: FAIL (legal name dropped from comment)"; fail=1
fi

# Overflow guard: a PR touching a shared input rebuilds thousands of checks, and
# an uncapped changed-checks list overflows GitHub's 65536-char comment limit
# (HTTP 422), so no comment posts. Synthesize a large report and assert the body
# stays bounded with an "...and N more" note. Behavior assertion, not a re-spell
# of the cap constant.
big="$tmp/big.json"
jq '.changed = [range(0; 4000) | "rust-test-crate-\(.)-unit-tests"]' "$fixtures/good.json" > "$big"
( cd "$tmp" && cp "$big" report.json && bash render.sh )
big_bytes="$(wc -c < "$tmp/comment.md")"
if [ "$big_bytes" -lt 65536 ]; then
  note "render overflow: body bounded (${big_bytes} B < 65536)"
else
  note "render overflow: FAIL (${big_bytes} B >= 65536)"; fail=1
fi
if grep -qE '^- \.\.\.and 3800 more ' "$tmp/comment.md"; then
  note "render overflow: cap note ok"
else
  note "render overflow: FAIL (missing/incorrect cap note)"; fail=1
fi
if grep -q '<summary>changed checks (4000)</summary>' "$tmp/comment.md"; then
  note "render overflow: total count ok"
else
  note "render overflow: FAIL (summary missing true total)"; fail=1
fi

# Backstop guard: the changed-checks cap does NOT bound the mermaid sections,
# which the render sizes from the (PR-controlled) report's causes. A schema-valid
# but pathological report with huge causes must still render under the limit via
# the byte-budget backstop, and the leading marker (the post job keys the sticky
# comment on it) must survive the tail truncation.
huge="$tmp/huge.json"
jq '
  .changed = [] |
  .causes = [range(0; 400) | {
    name: "ix-rust-workspace-\(.)",
    checks: [range(0; 5) | "rust-test-crate-\(.)-pads-the-body-out-to-exceed-the-limit-\(.)"]
  }]
' "$fixtures/good.json" > "$huge"
( cd "$tmp" && cp "$huge" report.json && bash render.sh )
huge_bytes="$(wc -c < "$tmp/comment.md")"
if [ "$huge_bytes" -lt 65536 ]; then
  note "render backstop: body bounded (${huge_bytes} B < 65536)"
else
  note "render backstop: FAIL (${huge_bytes} B >= 65536; backstop did not fire)"; fail=1
fi
if head -c 64 "$tmp/comment.md" | grep -q '^<!-- blast-radius -->'; then
  note "render backstop: marker survived truncation ok"
else
  note "render backstop: FAIL (marker lost; sticky-comment keying breaks)"; fail=1
fi

# Fallback path: when the `evaluate` job fails, the `comment` job composes a
# "could not compute" note (same `<!-- blast-radius -->` marker) instead of
# leaving the PR with no comment at all (#1415). Extract that step's verbatim
# script and assert the marker survives (sticky keying) and the run link and
# explanation are present. RUN_URL stands in for the workflow expression the
# step reads from its env.
yq '.jobs.comment.steps[] | select(.name == "Fallback comment (eval failed)").run' "$workflow" > "$tmp/fallback.sh"
( cd "$tmp" && rm -f comment.md && RUN_URL="https://github.com/indexable-inc/index/actions/runs/123" bash fallback.sh )
if head -c 64 "$tmp/comment.md" | grep -q '^<!-- blast-radius -->'; then
  note "fallback: marker present ok"
else
  note "fallback: FAIL (marker missing; sticky-comment keying breaks)"; fail=1
fi
if grep -q 'Could not compute the blast radius' "$tmp/comment.md"; then
  note "fallback: explanation present ok"
else
  note "fallback: FAIL (explanation missing)"; fail=1
fi
if grep -q 'actions/runs/123' "$tmp/comment.md"; then
  note "fallback: run link present ok"
else
  note "fallback: FAIL (run link missing)"; fail=1
fi

if [ "$fail" -ne 0 ]; then echo "blast-radius-test: FAILED"; exit 1; fi
echo "blast-radius-test: all passed"
