#!/usr/bin/env bash
#
# What the Rust VM's incrementality is allowed to claim.
#
# Three gates over rust/nix-eval-rs, each printed with the denominator it was
# measured against, because a bare count here has repeatedly been the thing
# that misled somebody:
#
#   A. serialization  a Module round-tripped through the content-addressed
#                     store evaluates to the same bytes as a fresh compile
#   B. persistence    a long-lived VM agrees with fresh processes, and an edit
#                     between two requests to one process is not answered from
#                     cache
#   C. memoisation    the same, with evaluated results served from the memo
#                     table, plus a count of how many answers actually came
#                     from it
#   D. persistence    a process started fresh against an on-disk store serves
#                     the same answers without evaluating, an edit between two
#                     processes invalidates as it does within one, and a
#                     damaged store is a miss with a reason rather than a wrong
#                     answer or a crash
#   E. eviction       a capped store stays under its cap through an edit loop,
#                     keeps serving hits while it does, and still answers
#                     byte-identically to a fresh process afterwards
#   F. scrub          the offline scan finds a mis-filed row that lookups, being
#                     by key, would never consult
#
# ## Gate B and C's edit case is the whole point
#
# The corpus arms cannot detect the failure that matters. Deliberately keying
# the read set on the questions and not the answers, which serves a pre-edit
# result forever, still passes the 300-file corpus comparison with
# `agree=300 differ=0`, because no corpus file changes during a run. Only the
# edit case catches it. If you are tempted to drop the edit case as fiddly,
# that is the one to keep and the corpus arm is the one to drop.
#
# ## Why `served_from_memo` is printed
#
# A memo cache that never hits is correct and useless, and a gate that only
# compares answers scores it as a pass. That happened during development: the
# lookup and record paths built the key differently, so every lookup missed
# while every store succeeded, and the corpus arm was perfectly green. The
# count is the only thing that noticed.
#
# ## Rebuilding is not optional
#
# The examples are rebuilt every run, on purpose. Editing the library and
# rerunning without `cargo build --examples` measures the previous binary; that
# cost an hour once, chasing a "flaky" soundness failure that was a stale
# binary left behind by a break test.
#
# ## Every arm counts its rows, not just its disagreements
#
# Arms A and C to E used to iterate the RESULT list and compare what was in
# it. A `timeout 600` that truncated the server's output therefore produced a
# short list in which everything present agreed, which is a pass -- the
# assertion whose passing state is an absence. Each arm now requires the exact
# row count its input implies (`2 * total` for C, which asks twice), so a
# truncated run fails on the count before anything is compared. "Nothing
# differed" and "all N were checked and none differed" are different claims,
# and only the second one is worth making.
#
#   ./rust-incremental-gate.sh [corpus-dir]
#
# Exit 0 iff every arm passes and every arm compared its full denominator.

set -u

repo=$(cd "$(dirname "$0")/../.." && pwd)
corpus=${1:-"$repo/tests/functional/lang"}
rust="$repo/rust"
work=$(mktemp -d "${TMPDIR:-/tmp}/rust-incr-gate.XXXXXX")
trap 'rm -rf "$work"' EXIT

# shellcheck source=./gate-ratchets.sh
. "$(cd "$(dirname "$0")" && pwd)/gate-ratchets.sh" || exit 2

cargo=${CARGO:-cargo}
command -v "$cargo" >/dev/null || {
    echo "gate: no cargo on PATH; try: nix shell nixpkgs#cargo nixpkgs#rustc nixpkgs#gcc" >&2
    exit 2
}

echo "== rebuilding examples =="
( cd "$rust" && "$cargo" build --release --examples -p nix-eval-rs ) || exit 2
server="$rust/target/release/examples/eval-server"
roundtrip="$rust/target/release/examples/module-roundtrip"
for bin in "$server" "$roundtrip"; do
    [ -x "$bin" ] || { echo "gate: missing $bin after a successful build" >&2; exit 2; }
done

ls "$corpus"/eval-okay-*.nix > "$work/files.txt" 2>/dev/null
total=$(wc -l < "$work/files.txt")
[ "$total" -gt 0 ] || { echo "gate: no eval-okay-*.nix under $corpus; refusing to score an empty corpus" >&2; exit 2; }
echo "corpus=$total files"

fail=0

# -- A. serialization ------------------------------------------------------
# One process per file: the corpus contains expressions this VM does not
# terminate on, and an in-process loop has no way to bound one.
echo
echo "== A. module round trip =="
: > "$work/rt.tsv"
while IFS= read -r f; do
    line=$( ulimit -v 4194304; timeout 20 "$roundtrip" "$corpus" "$f" 2>&1 )
    [ -n "$line" ] || line=$(printf '%s\tKILLED\t-\t-' "$f")
    printf '%s\n' "$line" >> "$work/rt.tsv"
done < "$work/files.txt"
awk -F'\t' '{c[$2]++} END{for (k in c) printf "  %-8s %d\n", k, c[k]}' "$work/rt.tsv"
bad=$(awk -F'\t' '$2=="FAIL"||$2=="KILLED"' "$work/rt.tsv" | wc -l)
[ "$bad" -eq 0 ] || { echo "  A FAILED: $bad files"; fail=1; }
# Counting the bad rows is not the same as requiring the good ones. `bad=0`
# is satisfied by an arm that round tripped nothing at all, by a row that
# never reached the file, and by any status this loop has not heard of.
# Three things are required instead: every file produced a row, none of them
# failed, and `skip` -- which module-roundtrip prints for a source its
# compiler could not take -- stays inside a checked-in budget. skip is the
# bucket that neither passes nor fails here, so it is the one that can grow
# without anybody noticing.
rows_rt=$(wc -l < "$work/rt.tsv")
if [ "$rows_rt" -ne "$total" ]; then
    echo "  A FAILED: $rows_rt rows for $total corpus files; some file produced no verdict at all"
    fail=1
fi
skip_rt=$(awk -F'\t' '$2=="skip"' "$work/rt.tsv" | wc -l)
match_rt=$(awk -F'\t' '$2=="match"' "$work/rt.tsv" | wc -l)
echo "  round trip: match=$match_rt skip=$skip_rt of $total (skip budget $RUST_INCR_MAX_SKIP)"
if [ "$skip_rt" -gt "$RUST_INCR_MAX_SKIP" ]; then
    echo "  A FAILED: $skip_rt files skipped, budget is $RUST_INCR_MAX_SKIP. A skip is a source this VM's compiler could not take, so it is coverage leaving the arm rather than a neutral outcome."
    awk -F'\t' '$2=="skip" {print "    skip\t" $1 "\t" $4}' "$work/rt.tsv" | head -10
    fail=1
fi
# And no status outside the three this arm knows about.
unknown_rt=$(awk -F'\t' '$2!="match" && $2!="skip" && $2!="FAIL" && $2!="KILLED"' "$work/rt.tsv" | wc -l)
if [ "$unknown_rt" -ne 0 ]; then
    echo "  A FAILED: $unknown_rt rows carry a status this gate does not know how to score"
    awk -F'\t' '$2!="match" && $2!="skip" && $2!="FAIL" && $2!="KILLED" {print "    " $2 "\t" $1}' "$work/rt.tsv" | head -5
    fail=1
fi

# -- B. persistence --------------------------------------------------------
echo
echo "== B. persistent VM vs fresh processes =="
# Each corpus arm appends its verdict here; the count at the end is what
# catches an arm that silently did not run.
: > "$work/arms"
( ulimit -v 8388608; timeout 600 "$server" < "$work/files.txt" > "$work/persistent.jsonl" 2>/dev/null )
: > "$work/fresh.jsonl"
while IFS= read -r f; do
    ( ulimit -v 4194304; timeout 30 "$server" <<< "$f" 2>/dev/null ) >> "$work/fresh.jsonl"
done < "$work/files.txt"
if ! python3 - "$work/persistent.jsonl" "$work/fresh.jsonl" "$work/arms" "$total" <<'PY'
import json, sys
def load(p):
    out = {}
    for line in open(p):
        if line.strip():
            r = json.loads(line)
            out[r["file"]] = (r["status"], r["value"])
    return out
a, b = load(sys.argv[1]), load(sys.argv[2])
total = int(sys.argv[4])
keys = sorted(set(a) | set(b))
differ = [k for k in keys if a.get(k) != b.get(k)]
verdict = f"  compared={len(keys)} agree={len(keys)-len(differ)} differ={len(differ)} want={total}"
print(verdict)
open(sys.argv[3], "a").write(verdict + "\n")
for k in differ[:5]:
    print(f"  DIFFER {k}: persistent={a.get(k)} fresh={b.get(k)}")
# The union of the two sides already turns a truncated persistent run into
# differences, but only while the fresh side is complete. Requiring the count
# outright covers the case where both were cut short by the same timeout.
if len(keys) != total:
    print(f"  B FAILED: {len(keys)} files compared, {total} in the corpus; a run cut short by its timeout compares a subset and every row in it agrees")
    raise SystemExit(1)
if differ:
    raise SystemExit(1)
PY
then
    fail=1
fi

# -- the edit case, for both B and C --------------------------------------
# Requests go through a FIFO so one process spans the edit.
edit_case() {
    local mode=$1 dir="$work/edit-$1" want=$2
    rm -rf "$dir"; mkdir -p "$dir"
    printf '{ n = 1; }\n' > "$dir/lib.nix"
    printf '(import ./lib.nix).n\n' > "$dir/main.nix"
    mkfifo "$dir/req"
    # shellcheck disable=SC2086 # $mode is "" or --memo; quoting it would pass
    # an empty argument instead of none.
    ( ulimit -v 4194304; timeout 60 "$server" $mode < "$dir/req" > "$dir/out" 2>/dev/null ) &
    local srv=$!
    exec 9>"$dir/req"
    local sent=0
    send() {
        echo "$dir/main.nix" >&9
        sent=$((sent + 1))
        local waited=0
        while [ "$(wc -l < "$dir/out")" -lt "$sent" ]; do
            sleep 0.1
            waited=$((waited + 1))
            [ "$waited" -lt 600 ] || { echo "  edit case timed out" >&2; return 1; }
        done
    }
    send && send || return 1
    # Edit the IMPORTED file: main.nix's own text is unchanged, so a cache
    # keyed on the request alone serves the stale answer here.
    printf '{ n = 2; }\n' > "$dir/lib.nix"
    send && send || return 1
    exec 9>&-
    wait $srv 2>/dev/null
    python3 - "$dir/out" "$want" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
got = ",".join(r["value"] for r in rows)
print(f"  values={got} want={sys.argv[2]}")
raise SystemExit(0 if got == sys.argv[2] else 1)
PY
}

echo "  edit between requests, no memoisation:"
edit_case "" "1,1,2,2" || { echo "  B FAILED: stale answer after an edit"; fail=1; }

# -- C. result memoisation -------------------------------------------------
echo
echo "== C. memoised results vs fresh processes =="
cat "$work/files.txt" "$work/files.txt" > "$work/two.txt"
( ulimit -v 8388608; timeout 600 "$server" --memo < "$work/two.txt" > "$work/memo.jsonl" 2>/dev/null )
if ! python3 - "$work/memo.jsonl" "$work/fresh.jsonl" "$work/arms" "$total" <<'PY'
import json, sys
memo = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
# Asked twice (files.txt concatenated with itself), so the row count is 2n.
want = 2 * int(sys.argv[4])
fresh = {}
for line in open(sys.argv[2]):
    if line.strip():
        r = json.loads(line)
        fresh[r["file"]] = (r["status"], r["value"])
differ = [r["file"] for r in memo if fresh.get(r["file"]) != (r["status"], r["value"])]
served = sum(1 for r in memo if r["memo"])
verdict = (f"  compared={len(memo)} agree={len(memo)-len(differ)} differ={len(differ)}"
           f" served_from_memo={served} want={want}")
print(verdict)
open(sys.argv[3], "a").write(verdict + "\n")
for f in differ[:5]:
    print(f"  DIFFER {f}")
# This arm iterates the RESULT list, so a `timeout 600` that cut the server
# off mid-corpus left a short list whose every row agreed -- a pass produced
# by measuring less. The count is checked before the contents.
if len(memo) != want:
    print(f"  C FAILED: {len(memo)} answers, wanted {want} (every corpus file, asked twice); the run was truncated")
    raise SystemExit(1)
# A memo cache that never serves is correct and useless; the corpus arm alone
# cannot tell that apart from a working one.
if differ or served == 0:
    raise SystemExit(1)
PY
then
    fail=1
fi

echo "  edit between requests, with memoisation:"
edit_case "--memo" "1,1,2,2" || { echo "  C FAILED: memo served a stale result"; fail=1; }

# -- D. persistence across processes ---------------------------------------
echo
echo "== D. a cold process on a warm store =="
store="$work/store"

# Pass 1 fills the store. Pass 2 is a genuinely separate process that must
# serve the same answers from it without evaluating.
( ulimit -v 8388608; timeout 600 "$server" --memo --store "$store" < "$work/files.txt" > "$work/d-fill.jsonl" 2>"$work/d-fill.err" )
( ulimit -v 8388608; timeout 600 "$server" --memo --store "$store" < "$work/files.txt" > "$work/d-warm.jsonl" 2>"$work/d-warm.err" )
if ! python3 - "$work/d-warm.jsonl" "$work/fresh.jsonl" "$work/arms" "$total" <<'PY'
import json, sys
warm = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
want = int(sys.argv[4])
fresh = {}
for line in open(sys.argv[2]):
    if line.strip():
        r = json.loads(line)
        fresh[r["file"]] = (r["status"], r["value"])
differ = [r["file"] for r in warm if fresh.get(r["file"]) != (r["status"], r["value"])]
served = sum(1 for r in warm if r["memo"])
# Counted separately because they are two different caches over one store, and
# `memo` alone stays green when compile-cache persistence breaks: the result
# cache answers first and the compile cache is never asked to prove itself.
compiled = sum(1 for r in warm if r["hit"])
verdict = (f"  compared={len(warm)} agree={len(warm)-len(differ)} differ={len(differ)}"
           f" served_from_memo={served} compile_hits={compiled} want={want}")
print(verdict)
open(sys.argv[3], "a").write(verdict + "\n")
for f in differ[:5]:
    print(f"  DIFFER {f}")
# Same truncation hole as arm C: the comparison walks the warm list, so a
# short list is a short comparison and not a failure until the count is
# required.
if len(warm) != want:
    print(f"  D FAILED: {len(warm)} answers off the warm store, wanted {want}; the run was truncated")
    raise SystemExit(1)
# Either count at zero means the store was written and never read, which is
# what a cold cache looks like and is the failure this arm exists to catch.
if differ or served == 0 or compiled == 0:
    raise SystemExit(1)
PY
then
    fail=1
fi

# The store must actually be on disk, not merely reported as used.
objects=$(find "$store/objects" -type f 2>/dev/null | wc -l)
rowfiles=$(find "$store/index" -type f 2>/dev/null | wc -l)
witnesses=$(find "$store/witness" -type f 2>/dev/null | wc -l)
echo "  on disk: objects=$objects rows=$rowfiles witnesses=$witnesses"
if [ "$objects" -eq 0 ] || [ "$rowfiles" -eq 0 ] || [ "$witnesses" -eq 0 ]; then
    echo "  D FAILED: the store is empty, so the arm above proved nothing"
    fail=1
fi

# -- D2. an edit between two processes -------------------------------------
# Same 1,1,2,2 shape as B and C, except every request is its own process, so
# nothing is carried in memory and the store is the only thing connecting them.
echo "  edit between four separate processes:"
edit_dir="$work/edit-cross"
rm -rf "$edit_dir"; mkdir -p "$edit_dir"
printf '{ n = 1; }\n' > "$edit_dir/lib.nix"
printf '(import ./lib.nix).n\n' > "$edit_dir/main.nix"
ask() {
    ( ulimit -v 4194304; timeout 60 "$server" --memo --store "$edit_dir/store" <<< "$edit_dir/main.nix" 2>>"$edit_dir/err" )
}
{ ask; ask; } > "$edit_dir/before.jsonl"
printf '{ n = 2; }\n' > "$edit_dir/lib.nix"
{ ask; ask; } > "$edit_dir/after.jsonl"
cat "$edit_dir/before.jsonl" "$edit_dir/after.jsonl" > "$edit_dir/all.jsonl"
if ! python3 - "$edit_dir/all.jsonl" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
values = ",".join(r["value"] for r in rows)
memo = ",".join("memo" if r["memo"] else "eval" for r in rows)
print(f"  values={values} want=1,1,2,2")
print(f"  served={memo} want=eval,memo,eval,memo")
# Both halves matter. The values catch a stale answer; the memo flags catch a
# store that is being written and never read, which would pass on values alone
# because every process would simply re-evaluate.
raise SystemExit(0 if values == "1,1,2,2" and memo == "eval,memo,eval,memo" else 1)
PY
then
    echo "  D FAILED: cross-process edit did not invalidate as it does in one process"
    fail=1
fi

# -- D3. a damaged store is a miss with a reason ---------------------------
echo "  damaged store entries:"
damage_case() {
    local label=$1 dir="$work/damage-$1"
    rm -rf "$dir"; mkdir -p "$dir"
    printf '1 + 41\n' > "$dir/plain.nix"
    # A second, differently valued file, so the swapped case has something to
    # swap with and a wrong answer would be visibly wrong.
    printf '1 + 1\n' > "$dir/other.nix"
    ( ulimit -v 4194304; timeout 60 "$server" --memo --store "$dir/store" <<< "$dir/plain.nix" >/dev/null 2>&1 )
    ( ulimit -v 4194304; timeout 60 "$server" --memo --store "$dir/store" <<< "$dir/other.nix" >/dev/null 2>&1 )
    case "$label" in
        truncated) for f in "$dir"/store/objects/*; do head -c 3 "$f" > "$f.t" && mv "$f.t" "$f"; done ;;
        garbage)   for f in "$dir"/store/objects/*; do printf 'not canonical at all' > "$f"; done ;;
        swept)     rm -f "$dir"/store/objects/* ;;
        swapped)   # Two objects exchanged. Both decode perfectly, so nothing
                   # downstream can notice; only re-hashing against the address
                   # the row asked for can. This is the case that separates a
                   # real integrity check from a decode that happens to fail.
                   mapfile -t objs < <(find "$dir"/store/objects -type f | sort)
                   if [ "${#objs[@]}" -ge 2 ]; then
                       cp "${objs[0]}" "$dir/a"; cp "${objs[1]}" "$dir/b"
                       cp "$dir/b" "${objs[0]}"; cp "$dir/a" "${objs[1]}"
                   fi ;;
        misfiled)  # Two rows exchanged, rather than one renamed to a key
                   # nobody asks for. Lookups are by key, so a row filed under
                   # an unused name is never consulted and there is nothing to
                   # report; swapping makes both rows answer a question they
                   # were not computed for, which is the case that must be
                   # refused.
                   for d in "$dir"/store/index/*/; do
                       mapfile -t rowfiles < <(find "$d" -type f | sort)
                       if [ "${#rowfiles[@]}" -ge 2 ]; then
                           cp "${rowfiles[0]}" "$dir/r0"; cp "${rowfiles[1]}" "$dir/r1"
                           cp "$dir/r1" "${rowfiles[0]}"; cp "$dir/r0" "${rowfiles[1]}"
                       fi
                   done ;;
    esac
    ( ulimit -v 4194304; timeout 60 "$server" --memo --store "$dir/store" <<< "$dir/plain.nix" > "$dir/out.jsonl" 2>"$dir/err" )
    local rc=$?
    python3 - "$dir/out.jsonl" "$dir/err" "$label" "$rc" <<'PY'
import json, sys
out, err, label, rc = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
rows = [json.loads(l) for l in open(out) if l.strip()]
reasons = [l for l in open(err) if "warning:" in l or "dropped a" in l]
value = rows[0]["value"] if rows else None
ok = rc == 0 and len(rows) == 1 and value == "42" and reasons
print(f"    {label:10s} exit={rc} answers={len(rows)} value={value} reasons={len(reasons)} "
      + ("ok" if ok else "FAILED"))
if not ok:
    for line in reasons[:3]:
        print("      " + line.rstrip())
raise SystemExit(0 if ok else 1)
PY
}
for case in truncated garbage swept misfiled swapped; do
    damage_case "$case" || { echo "  D FAILED: damaged store ($case)"; fail=1; }
done

# -- E. eviction ------------------------------------------------------------
# An edit loop under a byte cap. The store must stay under the cap, keep
# serving the unchanged files while it does, and still answer correctly.
echo
echo "== E. a capped store under an edit loop =="
evict="$work/evict"
rm -rf "$evict"; mkdir -p "$evict/src"
for i in $(seq 1 10); do
    printf 'let f = x: x * %d; in f %d\n' "$i" "$i" > "$evict/src/stable$i.nix"
done
printf '{ n = 0; }\n' > "$evict/src/churn.nix"
ls "$evict"/src/*.nix > "$evict/files.txt"

# Uncapped first, to learn what this loop would grow to.
for r in $(seq 0 20); do
    [ "$r" -gt 0 ] && printf '{ n = %d; }\n' "$r" > "$evict/src/churn.nix"
    ( ulimit -v 8388608; timeout 120 "$server" --memo --store "$evict/free" \
        < "$evict/files.txt" >/dev/null 2>&1 )
done
uncapped=$(du -sb "$evict/free" | cut -f1)

# Then the same loop under a cap of half that.
cap=$((uncapped / 2))
printf '{ n = 0; }\n' > "$evict/src/churn.nix"
over=0
for r in $(seq 0 20); do
    [ "$r" -gt 0 ] && printf '{ n = %d; }\n' "$r" > "$evict/src/churn.nix"
    ( ulimit -v 8388608; timeout 120 "$server" --memo --store "$evict/capped" \
        --store-max-bytes "$cap" < "$evict/files.txt" > "$evict/out.jsonl" 2>"$evict/err" )
    size=$(du -sb "$evict/capped" | cut -f1)
    [ "$size" -gt "$cap" ] && over=$((over + 1))
done
capped=$(du -sb "$evict/capped" | cut -f1)
served=$(grep -c '"memo":true' "$evict/out.jsonl" || true)
echo "  uncapped=${uncapped}B capped=${capped}B cap=${cap}B over_cap_rounds=$over"
echo "  last round served $served of $(wc -l < "$evict/files.txt") from cache (want $(( $(wc -l < "$evict/files.txt") - 1 )))"
if [ "$over" -ne 0 ]; then
    echo "  E FAILED: the store exceeded its cap on $over of 21 rounds"
    fail=1
fi
# A cap that evicts everything is under its cap and useless, so the working
# set has to survive, and the bar is every unchanged file rather than "most of
# them". That precision is the point: with a loose threshold, evicting by write
# time instead of by use still passed, losing one stable file per round while
# reading as healthy. The working set here is the 10 stable files; only the
# edited one should miss.
want_served=$(( $(wc -l < "$evict/files.txt") - 1 ))
if [ "$served" -lt "$want_served" ]; then
    echo "  E FAILED: $served hits in the last round, wanted $want_served;"
    echo "            eviction is discarding entries that are still in use"
    fail=1
fi

# And the capped store's answers must still match fresh processes.
: > "$evict/fresh.jsonl"
while IFS= read -r f; do
    ( ulimit -v 4194304; timeout 30 "$server" <<< "$f" 2>/dev/null ) >> "$evict/fresh.jsonl"
done < "$evict/files.txt"
if ! python3 - "$evict/out.jsonl" "$evict/fresh.jsonl" "$work/arms" "$(wc -l < "$evict/files.txt")" <<'PY'
import json, sys
capped = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
want = int(sys.argv[4])
fresh = {}
for line in open(sys.argv[2]):
    if line.strip():
        r = json.loads(line)
        fresh[r["file"]] = (r["status"], r["value"])
differ = [r["file"] for r in capped if fresh.get(r["file"]) != (r["status"], r["value"])]
verdict = f"  compared={len(capped)} agree={len(capped)-len(differ)} differ={len(differ)} want={want}"
print(verdict)
open(sys.argv[3], "a").write(verdict + "\n")
for f in differ[:5]:
    print(f"  DIFFER {f}")
# The last round's output is the input here, so a round that died partway
# through leaves fewer rows and every one of them agrees.
if len(capped) != want:
    print(f"  E FAILED: {len(capped)} answers from the capped store, wanted {want}; the last round did not finish")
    raise SystemExit(1)
# Eviction may only ever cause a miss, so a difference here is a wrong answer.
if differ:
    raise SystemExit(1)
PY
then
    echo "  E FAILED: a swept store gave a different answer"
    fail=1
fi

# -- F. scrub ---------------------------------------------------------------
# Lookups are by key, so a row filed under a key nobody asks for is never
# consulted and never reported. The scan that finds it must not be on the
# request path, so it lives in a subcommand; this checks the subcommand works.
echo
echo "== F. offline scrub =="
scrubdir="$work/scrub"
rm -rf "$scrubdir"; mkdir -p "$scrubdir/src"
printf '1 + 41\n' > "$scrubdir/src/a.nix"
printf '2 + 40\n' > "$scrubdir/src/b.nix"
( ulimit -v 4194304; timeout 60 "$server" --memo --store "$scrubdir/store" \
    < <(ls "$scrubdir"/src/*.nix) >/dev/null 2>&1 )

( timeout 60 "$server" --store "$scrubdir/store" --scrub > "$scrubdir/clean.out" 2>&1 )
clean_rc=$?
echo "  clean store: exit=$clean_rc $(grep -c 'refused' "$scrubdir/clean.out" >/dev/null && tail -1 "$scrubdir/clean.out")"
if [ "$clean_rc" -ne 0 ]; then
    echo "  F FAILED: scrub reported a problem with a healthy store"
    fail=1
fi

# Mis-file one row: rename it to a key nothing will ever look up. A point
# lookup cannot see this; only the scan can.
for d in "$scrubdir"/store/index/*/; do
    for f in "$d"*; do
        mv "$f" "${d}$(printf '%064d' 7)"
        break
    done
done
( timeout 60 "$server" --store "$scrubdir/store" --scrub > "$scrubdir/dirty.out" 2>&1 )
dirty_rc=$?
found=$(grep -c 'wrong key' "$scrubdir/dirty.out" || true)
echo "  mis-filed row: exit=$dirty_rc reported=$found"
if [ "$dirty_rc" -eq 0 ] || [ "$found" -eq 0 ]; then
    echo "  F FAILED: scrub did not report a mis-filed row"
    sed -n '1,6p' "$scrubdir/dirty.out"
    fail=1
fi

# An arm that produces no verdict is not an arm that passed. This gate shipped
# once with two of its four checks silently skipped: a heredoc attached to the
# wrong side of an `||` fed python an empty script, which exited 0 and printed
# nothing, and the run read as clean. Counting the verdict lines is what turns
# that into a failure.
echo
arms=$(wc -l < "$work/arms")
if [ "$arms" -ne 4 ]; then
    echo "gate: expected 4 corpus comparisons, saw $arms; an arm did not run"
    fail=1
fi
echo "RESULT ratchets-from=$GATE_RATCHETS_MEASURED_AT@$GATE_RATCHETS_MEASURED_ON"
[ "$fail" -eq 0 ] && echo "RESULT: pass" || echo "RESULT: FAIL"
exit "$fail"
