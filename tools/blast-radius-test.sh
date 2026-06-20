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
yq '.jobs.comment.steps[] | select(.name == "Compose fallback comment (no usable report)").run' "$workflow" > "$tmp/fallback.sh"

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

# Fallback comment: when there is no usable report (eval bailed, the artifact
# download flaked, the report was rejected, or render failed) the comment job
# posts an explicit "could not compute" note instead of skipping and leaving the
# PR with no blast-radius comment (the "comes up empty" bug, #1415 / #1429).
# Assert it produces a marker-prefixed body so the sticky-comment keying still
# finds and overwrites it on the next successful run.
( cd "$tmp" && rm -f report.json comment.md && RUN_URL="https://github.com/indexable-inc/index/actions/runs/123" bash fallback.sh )
if head -c 21 "$tmp/comment.md" | grep -q '^<!-- blast-radius -->'; then
  note "fallback: marker present ok"
else
  note "fallback: FAIL (missing marker; sticky-comment keying breaks)"; fail=1
fi
if grep -q 'Could not compute the blast radius' "$tmp/comment.md" \
   && grep -q 'actions/runs/123' "$tmp/comment.md"; then
  note "fallback: note + run link ok"
else
  note "fallback: FAIL (missing explanation or run link)"; fail=1
fi

# Orchestration invariants (#1429). The "always post a comment, but stay fail-
# closed" guarantee lives in the comment job's step `if:`/`continue-on-error`
# wiring, which jq cannot exercise. Assert it structurally with yq so it can't
# silently regress:
#   1. Validate + Render are `continue-on-error: true`, so a rejected report,
#      a download flake, or a render hiccup routes to the fallback instead of
#      killing the job with no comment.
#   2. Render is gated on `steps.validate.outcome == 'success'` -- this (not the
#      implicit success()) is what keeps an unvalidated/hostile report from ever
#      being rendered now that Validate is continue-on-error.
#   3. The fallback fires whenever Render did not succeed
#      (`steps.render.outcome != 'success'`), covering eval failure AND a flaked
#      comment pipeline.
#   4. Post sticky comment keeps the default `success()` gate (no explicit `if:`)
#      so a genuinely broken job never publishes a partial body.
# `select(...) |` filters before the field access so only the matching step
# emits (otherwise every non-matching step emits a blank line via `// ""`).
# Bracket-quote the key so a hyphenated field (continue-on-error) is read as a
# key, not parsed as subtraction by yq.
field() { yq ".jobs.comment.steps[] | select(.name == \"$1\") | .[\"$2\"] // \"\"" "$workflow"; }
for step in "Validate report schema" "Render comment"; do
  if [ "$(field "$step" "continue-on-error")" = "true" ]; then
    note "orchestration [$step]: continue-on-error ok"
  else
    note "orchestration [$step]: FAIL (not continue-on-error; a failure would kill the job, no fallback)"; fail=1
  fi
done
if field "Render comment" "if" | grep -q "steps.validate.outcome == 'success'"; then
  note "orchestration [Render]: gated on validate outcome (fail-closed) ok"
else
  note "orchestration [Render]: FAIL (renders without a successful validate; hostile report could be posted)"; fail=1
fi
if field "Compose fallback comment (no usable report)" "if" | grep -q "steps.render.outcome != 'success'"; then
  note "orchestration [fallback]: fires on render failure (covers #1429) ok"
else
  note "orchestration [fallback]: FAIL (does not cover a flaked comment pipeline)"; fail=1
fi
post_if="$(field "Post sticky comment" "if")"
if [ -z "$post_if" ]; then
  note "orchestration [Post]: default success() gate (fail-closed) ok"
else
  note "orchestration [Post]: FAIL (explicit if '$post_if' can post a partial/unvalidated body)"; fail=1
fi

# Concurrency safety: the workflow now triggers on every `labeled` event, but
# the canceling per-ref concurrency group must NOT let an unrelated-label run
# (which the job `if:` skips) cancel a real in-flight eval -- that would leave
# the PR with no report and no fallback comment. The group expression routes
# unrelated-label events into a per-run throwaway group; assert that guard is
# present so the empty-comment path can't silently return.
group_expr="$(yq '.concurrency.group' "$workflow")"
if printf '%s' "$group_expr" | grep -q "github.event.label.name != 'blast-radius'" \
   && printf '%s' "$group_expr" | grep -q 'noop-'; then
  note "orchestration [concurrency]: unrelated-label runs can't cancel a real run ok"
else
  note "orchestration [concurrency]: FAIL (unrelated label event can cancel the real blast-radius run -> empty comment)"; fail=1
fi

if [ "$fail" -ne 0 ]; then echo "blast-radius-test: FAILED"; exit 1; fi
echo "blast-radius-test: all passed"
