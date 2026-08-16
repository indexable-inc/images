#!/usr/bin/env bash
#
# Does the on-disk evaluation cache work through the real nix binary?
#
# rust-incremental-gate.sh exercises the caches through the library and the
# example server. This exercises them through `nix-instantiate` with
# `eval-backend = rust` and `eval-cache-dir` set, which is the only thing a
# user ever runs, and it is a separate script because it needs a built nix
# (meson setup build -Dnix:rust-eval=enabled && ninja -C build) rather than
# just cargo.
#
# Everything here checks an effect rather than a reported setting. `nix config
# show` reports eval-backend and eval-cache-dir on a binary compiled without
# the Rust evaluator at all, and a lang-diff run once scored mismatch=249
# against exactly such a stub. So: evaluate, and look at what appears on disk.
set -u
# The build directory is a parameter, not a fact about one person's home. It
# was hardcoded to ~/incr-vm, so running this from any other checkout measured
# whatever binary happened to be sitting in that one -- a stale-input failure
# that reports a confident pass. NIX_BUILD_DIR overrides; the old path is the
# default so existing invocations keep working.
BUILD=${NIX_BUILD_DIR:-$HOME/incr-vm/nix/build}
RUST_DIR=${RUST_DIR:-$(cd "$BUILD/.." && pwd)/rust}
NIXI=$BUILD/src/nix/nix-instantiate
[ -x "$NIXI" ] || NIXI=$BUILD/src/nix/nix
[ -x "$NIXI" ] || { echo "no nix-instantiate under $BUILD (set NIX_BUILD_DIR)"; exit 2; }
echo "measuring $NIXI"
here=$(cd "$(dirname "$0")" && pwd)
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
RUST="extra-experimental-features = rust-eval
eval-backend = rust"

echo "=== 0. capability probe: does this binary really evaluate with the rust backend? ==="
got=$(NIX_CONFIG="$RUST" "$NIXI" --eval --strict -E 1 2>&1)
echo "  probe result: $got"
[ "$got" = "1" ] || { echo "  REFUSING: the rust arm cannot evaluate '1'; nothing below would mean anything"; exit 2; }

echo "=== 1. does setting eval-cache-dir populate a store? ==="
NIX_CONFIG="$RUST
eval-cache-dir = $W/store" "$NIXI" --eval --strict -E '1 + 41' > "$W/a.out" 2>"$W/a.err"
echo "  value: $(cat "$W/a.out")"
objs=$(find "$W/store/objects" -type f 2>/dev/null | wc -l)
rows=$(find "$W/store/index" -type f 2>/dev/null | wc -l)
wits=$(find "$W/store/witness" -type f 2>/dev/null | wc -l)
echo "  store: objects=$objs rows=$rows witnesses=$wits"
[ "$objs" -gt 0 ] && [ "$rows" -gt 0 ] || { echo "  FAILED: the setting is inert, nothing was written"; exit 1; }

echo "=== 2. no cache dir means no store (the default really is off) ==="
# This used to look for directories named `store2*`, which nothing in this
# script ever creates, and then only printed the count without asserting on
# it. It therefore reported "0 store dirs created" on every possible run,
# including one where the default had been wired on.
#
# Snapshot the work directory instead. The bookkeeping lives in $SNAP, a
# separate directory, because a listing written into the directory being
# listed shows up in its own next listing.
SNAP=$(mktemp -d); trap 'rm -rf "$W" "$SNAP"' EXIT
find "$W" -mindepth 1 -maxdepth 1 | sort > "$SNAP/before"
NIX_CONFIG="$RUST" "$NIXI" --eval --strict -E '1 + 41' > "$W/b.out" 2>&1
find "$W" -mindepth 1 -maxdepth 1 | sort > "$SNAP/after"
echo "  value: $(cat "$W/b.out")"
# b.out is the redirection above, so it is the one expected new entry.
appeared=$(comm -13 "$SNAP/before" "$SNAP/after" | grep -vxF "$W/b.out" || true)
if [ -n "$appeared" ]; then
  echo "  FAILED: an evaluation with no eval-cache-dir set wrote to the filesystem:"
  # shellcheck disable=SC2086 # $appeared is a list of paths, one per line
  printf '    %s\n' $appeared
  exit 1
fi
echo "  nothing appeared beside the output file: the default really is off"

echo "=== 3. a second process serves the first one's work (timing on a heavy expression) ==="
heavy='builtins.foldl'"'"' (a: b: a + b) 0 (builtins.genList (i: i) 400000)'
t() { python3 -c "import time;print(f'{time.time():.4f}')"; }
# cold, no cache
s=$(t); NIX_CONFIG="$RUST" "$NIXI" --eval --strict -E "$heavy" > "$W/h0.out" 2>&1; e=$(t)
nocache=$(python3 -c "print(f\"{float('$e')-float('$s'):.3f}\")")
# cold, with cache (fills it)
s=$(t); NIX_CONFIG="$RUST
eval-cache-dir = $W/heavy" "$NIXI" --eval --strict -E "$heavy" > "$W/h1.out" 2>&1; e=$(t)
cold=$(python3 -c "print(f\"{float('$e')-float('$s'):.3f}\")")
# warm, separate process
s=$(t); NIX_CONFIG="$RUST
eval-cache-dir = $W/heavy" "$NIXI" --eval --strict -E "$heavy" > "$W/h2.out" 2>&1; e=$(t)
warm=$(python3 -c "print(f\"{float('$e')-float('$s'):.3f}\")")
echo "  value: $(cat "$W/h2.out")"
echo "  no-cache=${nocache}s  cold-with-cache=${cold}s  warm-second-process=${warm}s"
[ "$(cat "$W/h0.out")" = "$(cat "$W/h2.out")" ] || { echo "  FAILED: cached answer differs from uncached"; exit 1; }
python3 -c "
import sys
cold, warm = float('$cold'), float('$warm')
if warm >= cold * 0.5:
    print(f'  FAILED: warm {warm}s is not meaningfully faster than cold {cold}s')
    sys.exit(1)
print(f'  warm is {cold/warm:.1f}x faster than cold')
" || exit 1

echo "=== 4. two processes racing on one store ==="
#
# Both entry points, because both write now. Until ENG-12830 the handle path
# only ever put a compiled module into `eval-cache-dir`, so racing it exercised
# one of the two writers; it files result rows and witnesses as well now, and a
# concurrency check that covers the older writer alone would keep passing while
# the newer one corrupted the store.
race_arm() { # tag  cmd...
  local tag=$1
  shift
  local race=$W/race-$tag i bad=0
  for i in $(seq 1 8); do
    NIX_CONFIG="$RUST
eval-cache-dir = $race" "$@" > "$W/r-$tag-$i.out" 2>"$W/r-$tag-$i.err" &
  done
  wait
  for i in $(seq 1 8); do
    [ "$(cat "$W/r-$tag-$i.out")" = "$(cat "$W/h0.out")" ] || {
      bad=$((bad + 1))
      echo "  [$tag] process $i answered $(cat "$W/r-$tag-$i.out")"
    }
  done
  echo "  [$tag] 8 concurrent invocations, wrong answers: $bad"
  [ "$bad" -eq 0 ] || return 1
  local srv=$RUST_DIR/target/release/examples/eval-server
  "$srv" --store "$race" --scrub > "$W/scrub-$tag.out" 2>&1
  local srb=$?
  echo "  [$tag] scrub after the race: exit=$srb"
  tail -3 "$W/scrub-$tag.out"
  [ "$srb" -eq 0 ] || { echo "  FAILED: the $tag race left the store inconsistent"; return 1; }
  return 0
}
race_arm instantiate "$NIXI" --eval --strict -E "$heavy" || exit 1
# No `--raw`: this fixture evaluates to an integer and `--raw` is cppnix's
# `coerceToString` with `coerceMore = false`, which refuses one. The first
# version of this line used `--raw` and would have reported all 8 invocations
# wrong on every run, for a reason that has nothing to do with concurrency --
# and it could not be caught by running the script, because section 3 above
# exits first on any machine whose process startup is close to the fixture's
# cost. Plain `nix eval` prints the digits, which is what `h0.out` holds.
race_arm handle "$BUILD/src/nix/nix" eval --expr "$heavy" || exit 1
echo "=== 5. drv-parity gives the same verdict with and without the cache, stderr included ==="
# The one arm that compares *stderr*. Every other check here reads the value
# on stdout, and a whole class of divergence never touches stdout: cppnix
# warns about six derivation attributes `__structuredAttrs` disables, and a
# result served from the memo table used to reproduce the value while saying
# nothing (ENG-12540). A gate that diffs only stdout scores that as a pass.
#
# drv-parity is the right body for it because it is the only corpus here that
# builds derivations, which is what emits those warnings in the first place.
parity=$here/drv-parity.sh
[ -x "$parity" ] || { echo "  REFUSING: no drv-parity.sh beside this script"; exit 2; }

# The eval arm only. drv-parity's build arm names its fixtures after its own
# PID, deliberately, so that their `.drv` cannot already be in the store --
# which means two runs of it print different names and different store
# hashes, and the byte-for-byte diff below would fail on a difference that
# says nothing about caching. Normalising those hashes away instead would
# blind this gate to the store paths it exists to compare.
#
# Nothing is lost here: what this section is after is the *stderr* warnings a
# memoised result used to swallow (ENG-12540), and those come from the eval
# arm's `__structuredAttrs` cases. The build arm carries its own cache
# question -- whether a memo hit still writes the `.drv`, ENG-12801 -- and
# measures it itself, with its own fresh cache directory.
NIX_BUILD_DIR=$BUILD RUST_DIR=$RUST_DIR DRV_PARITY_ARMS=eval "$parity" \
  > "$W/parity-plain.out" 2> "$W/parity-plain.err"
plain=$?
NIX_BUILD_DIR=$BUILD RUST_DIR=$RUST_DIR DRV_PARITY_ARMS=eval \
  EXTRA_NIX_CONFIG="eval-cache-dir = $W/parity-store" "$parity" \
  > "$W/parity-cached.out" 2> "$W/parity-cached.err"
cachedrc=$?

# The cache has to have been used, or this compares two uncached runs and
# passes for the wrong reason -- the shape that has caught this repo out
# before (`nix config show` reporting a setting on a binary that ignores it).
objs=$(find "$W/parity-store/objects" -type f 2>/dev/null | wc -l | tr -d " ")
echo "  cache objects written by the cached arm: $objs"
[ "$objs" -gt 0 ] || { echo "  FAILED: the cached arm wrote nothing, so it was not cached"; exit 1; }

echo "  exit codes: plain=$plain cached=$cachedrc"
[ "$plain" -eq "$cachedrc" ] || { echo "  FAILED: eval-cache-dir changed drv-parity's verdict"; exit 1; }

# drv-parity makes its own scratch directory per run and that path appears in
# the expressions it echoes, so the two runs differ in a way that says nothing
# about caching. Normalised, not ignored: every OTHER byte still has to match,
# including the store paths those expressions produce, which is the whole
# point. (This gate caught the assumption that no normalisation was needed,
# which is the shape of thing it exists to catch.)
# Two forms, because `mktemp -d` does not agree across platforms: Linux hands
# back /tmp/tmp.XXXX, macOS hands back $TMPDIR/tmp.XXXX with TMPDIR under
# /var/folders/<a>/<b>/T/. Matching only the first left every scratch path
# intact on Darwin, so the byte-diff below failed on exactly the difference
# this function exists to erase -- a green Linux CI and a red Mac, for a
# reason that says nothing about caching.
scrub_scratch() {
    sed -E -e 's!/var/folders/[^/]+/[^/]+/T/tmp\.[A-Za-z0-9]+!/tmp/SCRATCH!g' \
           -e 's![/]tmp[/]tmp\.[A-Za-z0-9]+!/tmp/SCRATCH!g' "$1"
}
for stream in out err; do
  scrub_scratch "$W/parity-plain.$stream" > "$W/parity-plain.$stream.norm"
  scrub_scratch "$W/parity-cached.$stream" > "$W/parity-cached.$stream.norm"
  if ! diff -u "$W/parity-plain.$stream.norm" "$W/parity-cached.$stream.norm" > "$W/parity-$stream.diff"; then
    echo "  FAILED: drv-parity's std$stream differs with eval-cache-dir set"
    head -20 "$W/parity-$stream.diff"
    exit 1
  fi
done
# A comparison of two empty files would pass and mean nothing.
lines=$(grep -c . "$W/parity-plain.out")
[ "$lines" -ge 40 ] || { echo "  REFUSING: drv-parity printed only $lines lines; nothing was compared"; exit 2; }
echo "  stdout and stderr identical across $lines lines (scratch paths normalised)"

echo "ALL CLI CHECKS PASSED"
