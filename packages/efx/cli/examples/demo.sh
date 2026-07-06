#!/usr/bin/env bash
# End-to-end efx demo: apply (everything executes), apply again (all cache
# hits), change one input and re-plan (only the changed effect and its
# dependents invalidate), then render the HTML run report.
#
# Usage: demo.sh [workdir] [report-path]
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workdir="${1:-$(mktemp -d)}"
report="${2:-$workdir/efx-report.html}"
efx=(cargo run --quiet --manifest-path "$here/../../../../Cargo.toml" -p efx --)

cp "$here/site.efx" "$workdir/site.efx"
cd "$workdir"

echo "== run 1: first apply, everything executes"
"${efx[@]}" apply site.efx

echo
echo "== run 2: nothing changed, everything is a cache hit"
"${efx[@]}" apply site.efx

echo
echo "== edit one input: retitle the page"
sed -i.bak 's/hello from efx/hello again from efx/' site.efx && rm site.efx.bak
"${efx[@]}" plan site.efx

echo
echo "== run 3: only the changed effect and its dependents re-execute"
"${efx[@]}" apply site.efx

echo
"${efx[@]}" report --html "$report"
echo "demo site: $workdir/out/index.html"
