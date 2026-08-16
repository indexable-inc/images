#!/usr/bin/env bash
# Generate an attribute list for maintainers/ix/shadow-nixpkgs-sweep.sh from a
# nixpkgs tree, so a sweep corpus can be rebuilt instead of only inherited.
#
#   shadow-nixpkgs-corpus-gen.sh --nixpkgs PATH [options] > attrs.txt
#
#     --nixpkgs PATH   the tree to enumerate. REQUIRED, and a store path
#                      rather than a channel, for the reason below.
#     --bindir DIR     the nix build whose nix-env enumerates (default: PATH).
#                      Enumeration runs on the cpp arm; see below.
#     --suffix S       appended to every attribute (default `.drvPath`).
#                      Pass '' for bare attribute names.
#     --limit N        stride-sample N attributes instead of emitting all.
#                      The stride is n/N over the sorted list, so the sample
#                      is a pure function of the tree and N -- no seed, and
#                      re-running it produces the same file.
#     --min N          fail if fewer than N attributes survive (default 1).
#     --include-nested also emit attribute paths inside package sets
#                      (`python3Packages.foo`); off by default, see below.
#
# WHY THIS EXISTS. The two checked-in lists (`shadow-nixpkgs-attrs.txt`,
# `shadow-nixpkgs-attrs-wide.txt`) record how they were sampled in prose and
# not as a command, so neither can be regenerated or widened, and neither is
# tied to a tree. That has a measurable cost and not just an aesthetic one:
# `shadow-fleet-run.md` records that the wide list was generated from unstable
# and that 217 of its entries do not exist in the pinned tree. By ENG-12913 an
# attribute-not-found costs the Rust arm ~59s against an attrset this size, so
# those 217 absent attributes burned 14,864s -- 79% of that run's entire
# Rust-arm time -- to measure nothing at all. A list generated FROM the tree
# under test cannot contain a missing attribute, which deletes that cost
# rather than paying it.
#
# THE TREE IS AN ARGUMENT, NOT A DEFAULT. `shadow-nixpkgs-sweep.sh` warns when
# it falls back to the flake registry because the registry floats and the run
# stops being reproducible. A corpus is worse: the file outlives the run, so a
# floating tree produces a list nobody can attribute to anything. There is no
# default here on purpose.
#
# ENUMERATION RUNS ON THE CPP ARM, DELIBERATELY. This decides which attributes
# EXIST and what cppnix computes for them; it is the reference side of the
# comparison and must not depend on the backend under test. If the Rust arm
# chose the corpus, a construct it refuses would quietly shrink the
# denominator and the sweep would report a clean sheet over the subset the
# backend already handles -- the exact failure `shadow-nixpkgs-sweep.sh`
# exists to prevent by reporting coverage twice.
#
# ATTRIBUTES THAT CPPNIX CANNOT EVALUATE ARE KEPT. `nix-env` reports a null
# drvPath for an unfree or broken package (1,690 of them in nixpkgs 25.11.6495).
# They are emitted anyway, because both arms then fail and the harness scores
# them in its `agreed-failure` branch, which is real signal about error
# behaviour. Dropping them would be selecting the corpus on an outcome.
#
# TOP LEVEL BY DEFAULT. `nix-env -qaP` recurses into package sets that set
# `recurseForDerivations`, which turns 22,732 top-level attributes into 97,024.
# Nested paths are not more coverage per unit of time -- a package set is
# largely one build recipe applied many times -- so they are opt-in.
#
# The suffix is `.drvPath` by default because that is the tier-1 question
# (CLAUDE.md: `.drv` bytes, outPaths and drvPaths are byte-exact or nothing),
# and because selecting a string off the derivation keeps `--strict` out of
# `meta`. A bare attribute would deep-force the whole derivation attrset and
# reach `meta.position`, which is null on the Rust backend by an approved
# divergence (ENG-12591), so every row would diverge for one known reason and
# the sweep would say nothing about drvPaths.
set -u

# shellcheck source=./arm-config.sh
. "$(cd "$(dirname "$0")" && pwd)/arm-config.sh" || exit 2
arm_pin_environment
set -o pipefail

usage() { grep '^#' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }

nixpkgs=''
bindir=''
suffix='.drvPath'
limit=0
min=1
nested=no
while [ $# -gt 0 ]; do
  case $1 in
    --nixpkgs) nixpkgs=${2:-}; shift 2 ;;
    --bindir) bindir=${2:-}; shift 2 ;;
    --suffix) suffix=${2:-}; shift 2 ;;
    --limit) limit=${2:-}; shift 2 ;;
    --min) min=${2:-}; shift 2 ;;
    --include-nested) nested=yes; shift ;;
    *) usage ;;
  esac
done

[ -n "$nixpkgs" ] || { echo "shadow-nixpkgs-corpus-gen: --nixpkgs is required" >&2; usage; }
[ -d "$nixpkgs" ] || { echo "shadow-nixpkgs-corpus-gen: no nixpkgs at '$nixpkgs'" >&2; exit 2; }

if [ -n "$bindir" ]; then
  bindir=$(cd "$bindir" && pwd) || exit 2
  nixenv=$bindir/nix-env
else
  nixenv=$(command -v nix-env) || { echo "shadow-nixpkgs-corpus-gen: no nix-env on PATH and no --bindir" >&2; exit 2; }
fi
[ -x "$nixenv" ] || { echo "shadow-nixpkgs-corpus-gen: no executable nix-env at $nixenv" >&2; exit 2; }

command -v jq > /dev/null || { echo "shadow-nixpkgs-corpus-gen: jq is required" >&2; exit 2; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The cpp arm, explicitly. Not inherited: a machine with `eval-backend = rust`
# in its nix.conf would otherwise pick the corpus with the backend under test.
NIX_CONFIG="$(arm_base_config)
eval-backend = cpp" \
  "$nixenv" -f "$nixpkgs" -qaP --drv-path --json \
    --option allow-import-from-derivation false \
    --arg config '{ }' --arg overlays '[ ]' --argstr system x86_64-linux \
    > "$work/enum.json" 2> "$work/enum.err"
rc=$?
if [ "$rc" != 0 ]; then
  echo "shadow-nixpkgs-corpus-gen: nix-env enumeration failed (rc=$rc)" >&2
  sed -e 's/^/  /' "$work/enum.err" >&2
  exit 2
fi

jq -r 'to_entries[] | "\(.value.attrPath // .key)\t\(.value.drvPath)"' "$work/enum.json" \
  | LC_ALL=C sort > "$work/all.tsv"
n_all=$(wc -l < "$work/all.tsv" | tr -d ' ')
[ "$n_all" -gt 0 ] || { echo "shadow-nixpkgs-corpus-gen: nix-env returned no attributes; refusing to write an empty corpus" >&2; exit 2; }
n_null=$(awk -F'\t' '$2 == "null"' "$work/all.tsv" | wc -l | tr -d ' ')

cut -f1 "$work/all.tsv" > "$work/paths.txt"
if [ "$nested" = yes ]; then
  cp "$work/paths.txt" "$work/scoped.txt"
else
  grep -v '\.' "$work/paths.txt" > "$work/scoped.txt"
fi
n_scoped=$(wc -l < "$work/scoped.txt" | tr -d ' ')

# `shadow-nixpkgs-sweep.sh` names each attribute's stats file by mapping every
# character outside [A-Za-z0-9._-] to '_'. Two attributes differing only in
# such a character would collide and silently overwrite one another's census,
# so they are dropped here and the count is reported rather than swallowed.
grep -v '[^A-Za-z0-9._-]' "$work/scoped.txt" > "$work/safe.txt"
n_safe=$(wc -l < "$work/safe.txt" | tr -d ' ')
n_dropped=$(( n_scoped - n_safe ))

if [ "$limit" -gt 0 ] && [ "$n_safe" -gt "$limit" ]; then
  k=$(( n_safe / limit ))
  awk -v k="$k" -v m="$limit" 'NR % k == 0 && c < m { print; c++ }' "$work/safe.txt" > "$work/picked.txt"
else
  cp "$work/safe.txt" "$work/picked.txt"
fi
n_picked=$(wc -l < "$work/picked.txt" | tr -d ' ')

if [ "$n_picked" -lt "$min" ]; then
  echo "shadow-nixpkgs-corpus-gen: only $n_picked attributes survived, below --min $min" >&2
  exit 1
fi

# The header carries the command, because a corpus whose provenance is prose
# is one nobody can regenerate -- which is the whole reason this script exists.
cat <<HEADER
# nixpkgs attribute corpus for maintainers/ix/shadow-nixpkgs-sweep.sh
#
# GENERATED. Do not hand-edit; regenerate with the command below, which is a
# pure function of the tree and the arguments (no seed, no sampling state).
#
#   maintainers/ix/shadow-nixpkgs-corpus-gen.sh \\
#     --nixpkgs $nixpkgs \\
#     --suffix '$suffix'$( [ "$limit" -gt 0 ] && printf ' \\\n#     --limit %s' "$limit" )$( [ "$nested" = yes ] && printf ' \\\n#     --include-nested' )
#
# tree                     $nixpkgs
# enumerated by            $nixenv  (eval-backend = cpp, the reference arm)
# attribute paths found    $n_all
# of those, null drvPath   $n_null   (unfree/broken; kept, both arms fail and
#                          the harness scores that in agreed-failure)
# scope                    $( [ "$nested" = yes ] && echo 'top level + nested package sets' || echo 'top level only (dotless attribute paths)' )
# in scope                 $n_scoped
# dropped, unsafe filename $n_dropped   (chars outside [A-Za-z0-9._-] would
#                          collide in the harness stats filenames)
# emitted                  $n_picked$( [ "$limit" -gt 0 ] && printf '   (stride sample, --limit %s)' "$limit" )
#
# Attribute paths are relative to a PRE-APPLIED package set, which is what the
# harness constructs; see its header for why the root is applied.
HEADER

if [ -n "$suffix" ]; then
  sed -e "s|\$|$suffix|" "$work/picked.txt"
else
  cat "$work/picked.txt"
fi
