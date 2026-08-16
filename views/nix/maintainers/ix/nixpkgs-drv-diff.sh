#!/usr/bin/env bash
# Diff drvPaths over a nixpkgs top-level sweep between two evaluator arms of
# ONE nix build. Zero builds happen: drvPath equality transitively certifies
# the whole input-derivation closure, so this is the strongest cheap
# real-world equivalence check (the tvix technique).
#
#   nixpkgs-drv-diff.sh NIXBINDIR ARM_A ARM_B [--sample N]
#   nixpkgs-drv-diff.sh NIXBINDIR --self-diff [--sample N]
#
# NIXPKGS must point at a nixpkgs checkout; its rev is printed beside every
# result (a stale input produces a confident wrong measurement).
#
# IFD is forced off in both arms (allow-import-from-derivation = false) so
# any IFD aborts loudly instead of silently realizing store paths.
#
# Both arms always evaluate the FULL top level (that is what nix-env does);
# --sample N only bounds the comparison report to a deterministic every-k-th
# subset of the sorted attr intersection. Coverage is reported two ways, per
# the ASOF-join lesson: attrs each arm evaluated, then the share compared and
# matched.
#
# Exit 0 iff compared > 0, mismatch = 0, arm B produced no more nulls than
# arm A, and the two arms' attr sets still overlap by at least 99%. The last
# two are the ones that stop this from passing on an arm that broke: a null
# drvPath is an attr that failed to instantiate, and both-null compares equal,
# so failure moving into arm B reads as agreement. null_a and null_b were
# computed and printed and nothing was asserted about either.
set -u
# sort and join must agree on collation or join silently drops rows; the
# full-sweep self-diff produced 'is not sorted' warnings under the default
# locale at haskellPackages.berkeleydb. Bytewise order everywhere.
export LC_ALL=C

usage() { grep '^#' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
[ $# -ge 2 ] || usage
NIXBINDIR=$1; shift
SAMPLE=0
if [ "$1" = --self-diff ]; then ARM_A=none ARM_B=none; shift
else ARM_A=$1 ARM_B=$2; shift 2; fi
if [ $# -ge 1 ]; then
  [ "$1" = --sample ] && [ $# -eq 2 ] || usage
  SAMPLE=$2
fi

NIXPKGS=${NIXPKGS:?set NIXPKGS to a nixpkgs checkout}
NIX_ENV=$NIXBINDIR/nix-env
[ -x "$NIX_ENV" ] || { echo "nixpkgs-drv-diff: not executable: $NIX_ENV" >&2; exit 2; }
nixpkgs_rev=${NIXPKGS_REV:-$(git -C "$NIXPKGS" rev-parse HEAD 2>/dev/null || echo unknown)}

arm_config() {
  case $1 in
    none) ;;
    eval-backend=rust) printf 'extra-experimental-features = rust-eval\neval-backend = rust\n' ;;
    *=*) printf '%s = %s\n' "${1%%=*}" "${1#*=}" ;;
    *) echo "nixpkgs-drv-diff: bad arm spec '$1'" >&2; exit 2 ;;
  esac
}

tmp=$(mktemp -d /tmp/nixpkgs-drv-diff.XXXXXX)
trap 'rm -rf "$tmp"' EXIT

# Config and state isolation, for the same reason lang-diff.sh has it: the
# machine nix.conf leaks settings that change evaluation (abort-on-warn turned
# a deprecation warning into a hard failure here).
# The system pin lives in this conf file, not a command-line --option: a
# command-line option outranks NIX_CONFIG, which would make it impossible
# for an arm to vary `system`, and the mismatch guard-fire test does
# exactly that on a fixture tree.
conf=$tmp/conf; mkdir -p "$conf"
printf 'system = x86_64-linux\n' > "$conf/nix.conf"
sweep() { # arm-config outfile
  NIX_CONFIG=$1 NIX_CONF_DIR=$conf NIX_USER_CONF_FILES='' NIX_STATE_DIR=$tmp/state-nonexistent \
  "$NIX_ENV" -f "$NIXPKGS" -qaP --drv-path --json \
    --option allow-import-from-derivation false \
    > "$2" 2> "$2.err"
}

sweep "$(arm_config "$ARM_A")" "$tmp/a.json"; ec_a=$?
sweep "$(arm_config "$ARM_B")" "$tmp/b.json"; ec_b=$?
if [ "$ec_a" -ne 0 ] || [ "$ec_b" -ne 0 ]; then
  echo "nixpkgs-drv-diff: sweep failed (a=$ec_a b=$ec_b); stderr tails:" >&2
  tail -n 5 "$tmp/a.json.err" "$tmp/b.json.err" >&2
  exit 2
fi

for arm in a b; do
  jq -r 'to_entries[] | "\(.value.attrPath // .key)\t\(.value.drvPath)"' \
    "$tmp/$arm.json" | sort > "$tmp/$arm.tsv"
done
n_a=$(wc -l < "$tmp/a.tsv"); n_b=$(wc -l < "$tmp/b.tsv")
# Attrs whose drvPath is null failed to instantiate in that arm. Both-null
# compares equal and would read as a match, so the count is reported
# beside the verdict; growth here is failure moving where the differ
# cannot see it.
null_a=$(awk -F'\t' '$2 == "null"' "$tmp/a.tsv" | wc -l | tr -d ' ')
null_b=$(awk -F'\t' '$2 == "null"' "$tmp/b.tsv" | wc -l | tr -d ' ')

comm -12 <(cut -f1 "$tmp/a.tsv") <(cut -f1 "$tmp/b.tsv") > "$tmp/common"
n_common=$(wc -l < "$tmp/common")
if [ "$SAMPLE" -gt 0 ] && [ "$n_common" -gt "$SAMPLE" ]; then
  k=$((n_common / SAMPLE))
  awk -v k="$k" -v n="$SAMPLE" 'NR % k == 0 && c < n { print; c++ }' "$tmp/common" > "$tmp/picked"
else
  cp "$tmp/common" "$tmp/picked"
fi
n_picked=$(wc -l < "$tmp/picked")

join -t$'\t' "$tmp/picked" "$tmp/a.tsv" > "$tmp/a.picked"
join -t$'\t' "$tmp/picked" "$tmp/b.tsv" > "$tmp/b.picked"
# comm -3 indents lines unique to file 2 with one leading tab; strip it or
# cut -f1 reads an empty attr name there and every divergence counts twice.
mismatch=$(comm -3 "$tmp/a.picked" "$tmp/b.picked" | sed 's/^\t//' | cut -f1 | sort -u | tee "$tmp/mismatched" | wc -l | tr -d ' ')
matched=$((n_picked - mismatch))

[ "$mismatch" -gt 0 ] && sed 's/^/MISMATCH /' "$tmp/mismatched" | head -50

# The headline number is the one that carries information: a pair where both
# arms said null matched, but it matched about a failure. Reporting `matched`
# alone let an arm degrade into nulls and still read as parity.
nonnull_picked=$(join -t$'\t' "$tmp/picked" "$tmp/a.tsv" | awk -F'\t' '$2 != "null"' | wc -l | tr -d ' ')
echo "RESULT nixpkgs-drv-diff bin=$NIX_ENV nixpkgs=$nixpkgs_rev armA=$ARM_A armB=$ARM_B \
attrs-a=$n_a attrs-b=$n_b null-a=$null_a null-b=$null_b common=$n_common compared=$n_picked \
compared-non-null=$nonnull_picked matched=$matched mismatch=$mismatch"

if [ "$n_picked" -eq 0 ]; then
  echo "nixpkgs-drv-diff: compared zero attrs; an empty comparison is a failure, not a pass" >&2; exit 2
fi

ok=1
[ "$mismatch" -eq 0 ] || ok=0

# A null drvPath is an attr that did not instantiate. Two nulls compare equal,
# so every attr arm B newly fails on is a match here -- failure moving to
# where the differ cannot see it, scored as agreement. Arm A is the oracle, so
# arm B is allowed to match its nulls and never to add any.
if [ "$null_b" -gt "$null_a" ]; then
  echo "nixpkgs-drv-diff: arm B produced $null_b null drvPaths against arm A's $null_a. Those $((null_b - null_a)) attrs failed to instantiate under B and still compared equal wherever A failed too; a null is not a match." >&2
  comm -13 <(awk -F'\t' '$2 == "null" {print $1}' "$tmp/a.tsv") \
           <(awk -F'\t' '$2 == "null" {print $1}' "$tmp/b.tsv") | head -20 | sed 's/^/  ONLY-B-NULL /' >&2
  ok=0
fi

# And the arms must still be talking about the same nixpkgs. An arm that
# aborts partway through the sweep emits fewer attrs, and the intersection
# shrinks to whatever it managed -- inside which everything agrees.
min_common=$(( n_a * 99 / 100 ))
if [ "$n_common" -lt "$min_common" ]; then
  echo "nixpkgs-drv-diff: the arms share only $n_common attrs of arm A's $n_a (need $min_common, 99%). One arm did not finish the sweep, so the comparison covers whatever overlap survived rather than the top level." >&2
  ok=0
fi

[ "$ok" -eq 1 ]
