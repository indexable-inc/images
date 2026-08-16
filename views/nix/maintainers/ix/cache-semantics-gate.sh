#!/usr/bin/env bash
#
# `eval-cache-dir` changes speed and nothing else.
#
# Three proven violations of that (ENG-12540) were each a wrong answer visible
# only to somebody who had turned the option on, and none of them was visible
# to any gate that existed. lang-diff.sh compares cppnix against the Rust VM
# with both arms configured the same way, so a setting that changes meaning
# changes both arms identically and the comparison stays green.
#
# So this differs along the setting instead of along the evaluator: the same
# corpus, the same binary, once with a cache directory and once without, under
# several configurations. Any difference in an outcome class, a value or a
# warning is a failure.
#
#   cache-semantics-gate.sh [corpus-dir]
#
# ## The cross-configuration arm is the one that catches ENG-12541
#
# The paired arms above cannot see a memo key missing a setting: both runs in
# a pair are configured identically, so the key being short of `store_dir`
# never matters. The cross arm fills a cache under one configuration and then
# evaluates under another against that same cache. If a setting is missing
# from the key, the second run is served the first one's answers -- which for
# `store_dir` means an `outPath` computed against the wrong store, wrong in
# all 32 characters and indistinguishable from a right one.
#
# ## What this gate cannot see
#
# It compares answers, so it is blind to a cache that is correct and useless.
# The `CopyToStore` decoder bug was exactly that shape: every witness naming a
# path interpolation failed to parse, so those expressions re-evaluated for
# ever and answered correctly every time. Breaking that decoder on purpose
# leaves this script green. What catches it is
# `readset::tests::every_question_variant_round_trips_through_the_witness_codec`
# and `tests/cache_semantics.rs`'s
# `an_expression_that_interpolates_a_path_can_cache_hit`, which assert a hit
# rather than an answer. Run `cargo test -p nix-eval-rs` as well as this; the
# two cover different halves and neither subsumes the other.
#
# Each arm below has been watched failing, by reverting the fix it guards:
# arm 1 catches an unapplied call-depth ceiling and a dropped warning, arm 3
# catches a setting missing from the memo key.
#
# ## Denominators are printed, and a degenerate corpus is a failure
#
# A run over zero files, or one where every file lands in one outcome class,
# would compare equal and mean nothing. Both refuse.
#
# Exit 0 iff every comparison matched and every denominator was non-empty.
set -u

repo=$(cd "$(dirname "$0")/../.." && pwd)
corpus=${1:-"$repo/tests/functional/lang"}
# One expression per setting the lang corpus does not exercise. Without it two
# of the configurations below pass while witnessing nothing, which arm 2
# reports rather than hides. See that directory's README.
extra=$repo/maintainers/ix/cache-semantics-corpus
rust="$repo/rust"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Rebuilt every run, on purpose: editing the library and rerunning without a
# build measures the previous binary, which is how a break test once left a
# fake soundness failure behind for an hour.
echo "=== building the differential harness (release) ==="
( cd "$rust" && cargo build --release -p nix-eval-rs --example cache-differential ) || exit 2
harness=$rust/target/release/examples/cache-differential
[ -x "$harness" ] || { echo "REFUSING: no harness at $harness"; exit 2; }

# One configuration per line: a label, then the flags. `store_dir` and
# `nix_version` are OnceLocks, so each of these has to be its own process --
# which is what the whole file being a shell script buys.
configs=(
  "default|"
  "store-elsewhere|--store-dir /tmp/some-other/store"
  "shallow|--max-call-depth 100"
  "old-version|--nix-version 2.20.0"
  "other-platform|--current-system aarch64-darwin"
  "pure|--pure-eval"
  "restrict|--restrict-eval"
)

# Prefix every line of a block, for readable nested output. A function rather
# than `sed 's/^/      /'` because shellcheck flags that (SC2001) and the gate
# has to pass the repo's own pre-commit run.
indent() { while IFS= read -r line; do printf "      %s\n" "$line"; done <<< "$1"; }

run() { # run LABEL OUTFILE FLAGS...
  local label=$1 out=$2; shift 2
  # shellcheck disable=SC2086
  # --verify-rate 1 makes every cached arm check every hit against a fresh
  # evaluation and look every record up again. The corpus then does double
  # duty: it compares cached against uncached, and each cached run also
  # compares itself against evaluating. A disagreement fails the run through
  # the harness's own complaint channel, which the refusal below catches.
  "$harness" --corpus "$corpus" --corpus "$extra" --verify-rate 1 "$@" \
    > "$out" 2> "$out.err"
  local rc=$?
  [ "$rc" -eq 0 ] || { echo "  FAILED: $label exited $rc"; sed -n '1,5p' "$out.err"; return 1; }
  return 0
}

fail=0

echo "=== 0a. is the self-checking cache actually wired? ==="
# Every arm below passes --verify-rate 1, and a setting that never reached the
# code satisfies every assertion built on it. `nix config show` reports
# eval-backend=rust on a binary compiled without the Rust evaluator, and one
# lang-diff run scored mismatch=249 against exactly such a stub. So probe the
# effect: plant a wrong answer and require the verifier to catch it.
if ! "$harness" --verify-selftest; then
  echo "  REFUSING: the verifier is not wired in this binary, so every"
  echo "            --verify-rate arm below would measure nothing"
  exit 2
fi

echo "=== 0. the corpus says something ==="
run probe "$work/probe" || exit 2
files=$(wc -l < "$work/probe" | tr -d ' ')
classes=$(cut -f2 "$work/probe" | sort -u | wc -l | tr -d ' ')
echo "  corpus=$files files, distinct outcome classes=$classes"
[ "$files" -ge 265 ] || { echo "  REFUSING: only $files files; a shrunken corpus compares equal for the wrong reason"; exit 2; }
[ "$classes" -ge 4 ] || { echo "  REFUSING: only $classes outcome classes; the harness is not distinguishing outcomes"; exit 2; }

echo "=== 1. cached equals uncached, per configuration ==="
for entry in "${configs[@]}"; do
  label=${entry%%|*}; flags=${entry#*|}
  # shellcheck disable=SC2086
  run "$label uncached" "$work/$label.none" $flags || { fail=1; continue; }
  # shellcheck disable=SC2086
  run "$label cold" "$work/$label.cold" --cache "$work/store-$label" $flags || { fail=1; continue; }
  # shellcheck disable=SC2086
  run "$label warm" "$work/$label.warm" --cache "$work/store-$label" $flags || { fail=1; continue; }
  cold_diff=$(diff "$work/$label.none" "$work/$label.cold" | head -20)
  warm_diff=$(diff "$work/$label.none" "$work/$label.warm" | head -20)
  if [ -n "$cold_diff" ] || [ -n "$warm_diff" ]; then
    echo "  FAILED [$label]: eval-cache-dir changed an answer"
    [ -n "$cold_diff" ] && { echo "    cold:"; indent "$cold_diff"; }
    [ -n "$warm_diff" ] && { echo "    warm:"; indent "$warm_diff"; }
    fail=1
  else
    echo "  [$label] $files files identical, cached and uncached, cold and warm"
  fi
done

echo "=== 2. the configurations really do differ from each other ==="
# Without this the arms above are five copies of one comparison, and a harness
# whose flags never reached the evaluator would score a clean sweep. Every
# configuration must differ from the default in at least one outcome.
moved=0
for entry in "${configs[@]}"; do
  label=${entry%%|*}
  [ "$label" = default ] && continue
  if diff -q "$work/default.none" "$work/$label.none" > /dev/null; then
    echo "  [$label] changes no outcome; it cannot witness anything"
  else
    n=$(diff "$work/default.none" "$work/$label.none" | grep -c '^<')
    echo "  [$label] moves $n of $files outcomes"
    moved=$((moved + 1))
  fi
done
[ "$moved" -ge 2 ] || { echo "  REFUSING: only $moved configurations change any outcome, so arm 3 has almost nothing to detect"; exit 2; }

echo "=== 3. a swept store still serves the next process ==="
# The arm whose absence let ENG-12601 ship. Everything above uses an uncapped
# store, so the sweep never runs, so a sweep that deleted every witness in the
# store passed all of it -- while a capped store served nothing and reported
# itself healthy.
#
# Deliberately not a copy of rust-incremental-gate's arm E. That one is about
# eviction *pressure*: does the working set survive a cap that bites. This one
# sets a cap far above the store's size, so nothing is under pressure and
# nothing should be evicted at all, which isolates "the sweep ran" from "the
# sweep had to choose". ENG-12601 fails this one instantly, because the
# witness pass is not gated on pressure.
( cd "$rust" && cargo build --release -p nix-eval-rs --example eval-server ) || exit 2
server=$rust/target/release/examples/eval-server
[ -x "$server" ] || { echo "  REFUSING: no eval-server at $server"; exit 2; }

sweep=$work/sweep
mkdir -p "$sweep/src"
for i in 1 2 3 4 5; do
  printf 'let f = x: x * %d; in f %d\n' "$i" "$i" > "$sweep/src/m$i.nix"
done
ls "$sweep"/src/*.nix > "$sweep/files.txt"
want=$(wc -l < "$sweep/files.txt" | tr -d ' ')

# Fill, uncapped, so the first run has nothing to sweep and nothing to hit.
"$server" --memo --store "$sweep/store" < "$sweep/files.txt" > "$sweep/fill.jsonl" 2> "$sweep/fill.err"
filled=$(grep -c '"memo":true' "$sweep/fill.jsonl" || true)
size=$(find "$sweep/store" -type f -exec cat {} + 2>/dev/null | wc -c | tr -d ' ')
echo "  fill: $filled of $want served (expect 0), store holds ${size}B"
[ "$filled" -eq 0 ] || { echo "  REFUSING: the fill run served $filled; the store was not cold"; exit 2; }

# Two runs, not one, and that is load-bearing. Each process looks up BEFORE it
# sweeps, so the first capped round still hits off the witnesses the fill run
# left and passes even with ENG-12601 present; it is the second that reads a
# directory the first one emptied. Measured with the bug restored: round 1
# served 5 of 5 with "0 witnesses left", round 2 served 0 of 5.
roomy=$(( size * 100 + 1000000 ))
for round in 1 2; do
  "$server" --memo --store "$sweep/store" --store-max-bytes "$roomy" \
    < "$sweep/files.txt" > "$sweep/swept-$round.jsonl" 2> "$sweep/swept-$round.err"
  served=$(grep -c '"memo":true' "$sweep/swept-$round.jsonl" || true)
  witnesses=$(find "$sweep/store/witness" -type f 2>/dev/null | grep -vc '/[.]tmp-' || true)
  echo "  round $round under a ${roomy}B cap: $served of $want served, $witnesses witnesses left"
  if [ "$served" -ne "$want" ]; then
    echo "  FAILED: a swept store served $served of $want; the sweep is destroying"
    echo "          what the next process needs, so the cache is write-only"
    sed -n '1,5p' "$sweep/swept-$round.err"
    fail=1
    break
  fi
done

echo "=== 4. a cache filled under one configuration does not answer another ==="
# ENG-12541. Fill a cache under each configuration, then evaluate every other
# configuration against that same cache, and require the answers to be the
# ones that configuration gives with no cache at all.
for filler in "${configs[@]}"; do
  fill_label=${filler%%|*}; fill_flags=${filler#*|}
  shared=$work/shared-$fill_label
  # shellcheck disable=SC2086
  run "fill $fill_label" "$work/fill-$fill_label" --cache "$shared" $fill_flags || { fail=1; continue; }
  for user in "${configs[@]}"; do
    use_label=${user%%|*}; use_flags=${user#*|}
    [ "$use_label" = "$fill_label" ] && continue
    # shellcheck disable=SC2086
    run "$use_label on $fill_label's cache" "$work/x-$fill_label-$use_label" --cache "$shared" $use_flags || { fail=1; continue; }
    d=$(diff "$work/$use_label.none" "$work/x-$fill_label-$use_label" | head -10)
    if [ -n "$d" ]; then
      echo "  FAILED: $use_label served $fill_label's answers from a shared cache"
      indent "$d"
      fail=1
    fi
  done
  echo "  [$fill_label's cache] every other configuration still got its own answers"
done

if [ "$fail" -eq 0 ]; then
  echo "ALL CACHE-SEMANTICS CHECKS PASSED ($files files, ${#configs[@]} configurations)"
else
  echo "CACHE-SEMANTICS GATE FAILED"
fi
exit "$fail"
