#!/usr/bin/env bash
# Run `eval-backend = shadow` over a few hundred nixpkgs attributes and report
# what shadow saw: how many attributes the Rust arm could evaluate at all, and
# the divergence histogram over that evaluated set.
#
#   shadow-nixpkgs-sweep.sh BINDIR ATTRFILE [options]
#
#     --nixpkgs PATH   the tree to sweep (default: the flake registry's, which
#                      FLOATS -- pass a store path for a reproducible run)
#     --jobs N         parallel nix-instantiate processes (default 16)
#     --out DIR        keep the per-attribute stats there (default: a tempdir)
#     --label TEXT     printed beside the numbers, e.g. "pinned 25.11"
#     --root-file F    sweep the attributes of F instead of a nixpkgs package
#                      set. F must already be an attribute set (not a
#                      function), for the auto-call reason below. Used for the
#                      ix fleet inventory arm.
#     --expect-refusal-token TOK
#                      require that TOK is the majority refusal token. For the
#                      seeded-divergence and known-wall runs, where a clean
#                      sheet would mean the run measured the wrong thing.
#
# Why this exists beside shadow-corpus.sh: that script runs 262 small
# expressions written to exercise language corners, and its own report says so
# ("One workload, and it is the language corpus ... A nixpkgs or ix-fleet
# evaluation would weight the refusal histogram completely differently and is
# the run that should decide the default flip. This is not that run."). This is
# an attempt at that run.
#
# THE ROOT IS PRE-APPLIED, AND THAT IS NOT COSMETIC. `nix-instantiate -A x
# <nixpkgs>` makes cppnix auto-call the function at the root using --arg and
# the formals' defaults; `evalAndSelect` in the bridge does not, and refuses
# with `command-unsupported` ("auto-calling the function reached at ..."). A
# sweep against the bare tree therefore reports one refusal per attribute and
# measures the command layer rather than the evaluator. So the sweep writes a
# one-line wrapper that applies the function itself, and the attribute paths
# are relative to that.
#
# COVERAGE IS REPORTED TWICE, ALWAYS. "How many attributes diverged" is
# meaningless without "how many the Rust arm evaluated at all", because a
# backend that refuses everything reports zero divergences and reads exactly
# like a backend that agrees with everything. Refusals are not divergences;
# they get their own histogram keyed on the refusal token.
set -u

# shellcheck source=./arm-config.sh
. "$(cd "$(dirname "$0")" && pwd)/arm-config.sh" || exit 2
# One owner of the gates' nix configuration, before anything reads the
# environment: an ambient `lint-url-literals = fatal` otherwise makes every
# rust arm refuse and every row score `unimplemented` (ENG-12996).
arm_pin_environment
set -o pipefail

usage() { grep '^#' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }

bindir=${1:-}; attrfile=${2:-}
[ -n "$bindir" ] && [ -n "$attrfile" ] || usage
shift 2
nixpkgs=''
jobs=16
outdir=''
label=''
expect_token=''
root_file=''
while [ $# -gt 0 ]; do
  case $1 in
    --nixpkgs) nixpkgs=${2:-}; shift 2 ;;
    --jobs) jobs=${2:-}; shift 2 ;;
    --out) outdir=${2:-}; shift 2 ;;
    --label) label=${2:-}; shift 2 ;;
    --root-file) root_file=${2:-}; shift 2 ;;
    --expect-refusal-token) expect_token=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done

# Absolute before the loop cds anywhere, the lesson shadow-corpus.sh records:
# a relative binary path stops resolving and every case fails identically,
# which reports zero divergences over zero attempts.
bindir=$(cd "$bindir" && pwd) || exit 2
instantiate=$bindir/nix-instantiate
[ -x "$instantiate" ] || { echo "shadow-nixpkgs-sweep: no nix-instantiate at $instantiate" >&2; exit 2; }
[ -r "$attrfile" ] || { echo "shadow-nixpkgs-sweep: no attribute list at $attrfile" >&2; exit 2; }

work=$(mktemp -d)
cleanup() { rm -rf "$work"; }
trap cleanup EXIT
[ -n "$outdir" ] || outdir=$work/stats
mkdir -p "$outdir"

sha=$(sha256sum "$instantiate" | cut -d' ' -f1)
echo "shadow-nixpkgs-sweep: binary $instantiate sha256=$sha"
[ -n "$label" ] && echo "shadow-nixpkgs-sweep: label $label"

# The three parser lints are pinned rather than inherited, for the reason
# shadow-corpus.sh records: NIX_CONFIG applies on top of the user's nix.conf,
# so a machine with `lint-url-literals = fatal` makes both arms reject what
# the lint hits (it used to make the bridge refuse everything by name, via
# the since-retired `command-parser-lint` token) and the histogram becomes a
# fact about the machine.
# From `arm-config.sh`, the one owner of the gates' nix configuration
# (ENG-12996). `ignore` and not `warn`; see that file for why.
lints=$(arm_base_config)

# eval-shadow-budget = 0 (no limit). The default 120s of Rust-arm time per
# process is the right production default and the wrong measurement setting:
# it would silently convert the tail of a long sweep into skipped[budget].
# Each attribute here is its own process anyway, so the budget would rarely
# bind; setting it to 0 means the report cannot be quietly truncated.
base="extra-experimental-features = rust-eval
eval-shadow-budget = 0
$lints"
rust_config="$base
eval-backend = rust"
shadow_config="$base
eval-backend = shadow"

if [ -z "$nixpkgs" ]; then
  nixpkgs=$(nix eval --raw --impure --expr '(builtins.getFlake "nixpkgs").outPath' 2>"$work/resolve.err") || {
    echo "shadow-nixpkgs-sweep: could not resolve the nixpkgs flake, and --nixpkgs is unset:" >&2
    sed -e 's/^/  /' "$work/resolve.err" >&2
    exit 2
  }
  echo "shadow-nixpkgs-sweep: WARNING --nixpkgs was not given, so this used the flake registry," >&2
  echo "  which floats. The run is not reproducible. Resolved to $nixpkgs" >&2
fi
[ -d "$nixpkgs" ] || { echo "shadow-nixpkgs-sweep: no nixpkgs at '$nixpkgs'" >&2; exit 2; }
echo "shadow-nixpkgs-sweep: nixpkgs $nixpkgs"

echo "shadow-nixpkgs-sweep: ambient settings that decide what is measured:"
for setting in pure-eval restrict-eval eval-shadow-budget lint-url-literals lint-short-path-literals lint-absolute-path-literals; do
  value=$(NIX_CONFIG="$shadow_config" "$bindir/nix" config show "$setting" 2>/dev/null || echo '?')
  echo "  $setting = $value"
done

run_under() { # config statsfile args...
  local config=$1 stats=$2; shift 2
  NIX_CONFIG="$config" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$stats" \
    "$instantiate" "$@" > "$work/out" 2> "$work/err"
  echo $?
}

# Probe 1: the Rust evaluator is linked in and answers. `nix config show`
# reports eval-backend on a binary compiled without it (CLAUDE.md), so the
# only check worth anything is an evaluation.
rc=$(run_under "$rust_config" "$work/p1.json" --eval --strict -E 1)
probe=$(cat "$work/out")
if [ "$rc" != 0 ] || [ "$probe" != 1 ]; then
  echo "shadow-nixpkgs-sweep: this binary does not evaluate through the Rust backend" >&2
  echo "  rc=$rc stdout='$probe'" >&2; sed -e 's/^/  /' "$work/err" >&2; exit 2
fi
echo "shadow-nixpkgs-sweep: probe 1 ok, the Rust backend evaluates"

# Probe 2: shadow really compared something. Zero attempts and zero
# divergences are the same histogram and only one of them is a clean run.
rc=$(run_under "$shadow_config" "$work/p2.json" --eval --strict -E 1)
attempts=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("shadow",{}).get("attempts",0))' "$work/p2.json" 2>/dev/null || echo 0)
if [ "$rc" != 0 ] || [ "$attempts" -lt 1 ]; then
  echo "shadow-nixpkgs-sweep: shadow did not shadow (rc=$rc attempts=$attempts)" >&2
  sed -e 's/^/  /' "$work/err" >&2; exit 2
fi
echo "shadow-nixpkgs-sweep: probe 2 ok, shadow compared $attempts evaluation(s) of a trivial expression"

# The pre-applied root. See the header: without this the sweep measures the
# command layer's missing auto-call and nothing else.
if [ -n "$root_file" ]; then
  [ -r "$root_file" ] || { echo "shadow-nixpkgs-sweep: no root file at $root_file" >&2; exit 2; }
  wrapper=$(cd "$(dirname "$root_file")" && pwd)/$(basename "$root_file")
  echo "shadow-nixpkgs-sweep: root file $wrapper"
else
  wrapper=$work/pkgs.nix
  printf 'import <nixpkgs> { system = "x86_64-linux"; config = { }; overlays = [ ]; }\n' > "$wrapper"
fi

# Probe 3: the wrapper itself is servable by BOTH arms before any attribute is
# scored. If the root refuses, every row below refuses for that one reason and
# the per-attribute histogram says nothing about the attributes. This is the
# probe that the pinned-nixpkgs run fails, which is the point of having it.
root_rust=$(NIX_CONFIG="$rust_config" "$instantiate" --eval --strict -I "nixpkgs=$nixpkgs" \
  -E "builtins.typeOf (import $wrapper)" 2>&1)
if ! printf '%s' "$root_rust" | grep -q '^"set"$'; then
  echo "shadow-nixpkgs-sweep: NOTE the Rust arm cannot evaluate the package set root."
  echo "  Every attribute below will refuse for that one reason. Reported, not hidden:"
  printf '%s\n' "$root_rust" | grep -v 'search path entry' | sed -e 's/^/    /' | head -4
  root_servable=no
else
  echo "shadow-nixpkgs-sweep: probe 3 ok, both arms evaluate the package set root"
  root_servable=yes
fi

mapfile -t attrs < <(grep -vE '^\s*(#|$)' "$attrfile")
n_attrs=${#attrs[@]}
[ "$n_attrs" -gt 0 ] || { echo "shadow-nixpkgs-sweep: the attribute list is empty; refusing to report zero over nothing" >&2; exit 2; }
echo "shadow-nixpkgs-sweep: sweeping $n_attrs attributes, $jobs at a time"

export NIX_CONFIG="$shadow_config"
export SWEEP_INSTANTIATE=$instantiate SWEEP_NIXPKGS=$nixpkgs SWEEP_WRAPPER=$wrapper SWEEP_OUT=$outdir
one() {
  attr=$1
  safe=$(printf '%s' "$attr" | tr -c 'A-Za-z0-9._-' '_')
  NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$SWEEP_OUT/$safe.json" \
    "$SWEEP_INSTANTIATE" --eval --strict -I "nixpkgs=$SWEEP_NIXPKGS" \
    -A "$attr" "$SWEEP_WRAPPER" > "$SWEEP_OUT/$safe.out" 2> "$SWEEP_OUT/$safe.err"
  printf '%s\t%s\n' "$?" "$attr" >> "$SWEEP_OUT/../rc.tsv"
}
export -f one
: > "$outdir/../rc.tsv"
started=$(date +%s)
printf '%s\n' "${attrs[@]}" | xargs -P "$jobs" -I{} bash -c 'one "$@"' _ {}
elapsed=$(( $(date +%s) - started ))
echo "shadow-nixpkgs-sweep: sweep wall clock ${elapsed}s at -P $jobs"

python3 - "$outdir" "$n_attrs" "$sha" "$nixpkgs" "$label" "$elapsed" "$root_servable" "$expect_token" <<'PY'
import json, pathlib, sys
from collections import Counter

outdir = pathlib.Path(sys.argv[1])
n_attrs, sha, nixpkgs, label, elapsed = int(sys.argv[2]), sys.argv[3], sys.argv[4], sys.argv[5], int(sys.argv[6])
root_servable, expect_token = sys.argv[7], sys.argv[8]

attempts = unaccounted = micros = 0
stats_read = 0
verdicts, tokens, skips, kinds = Counter(), Counter(), Counter(), Counter()
divergences = {}

for path in sorted(outdir.glob("*.json")):
    try:
        blob = json.loads(path.read_text())
    except (OSError, ValueError):
        continue
    stats_read += 1
    s = blob.get("shadow", {})
    attempts += s.get("attempts", 0)
    unaccounted += s.get("unaccounted", 0)
    micros += s.get("rustMicros", 0)
    verdicts.update(s.get("verdicts", {}))
    tokens.update(s.get("refusalTokens", {}))
    skips.update(s.get("skipped", {}))
    kinds.update(s.get("divergenceKinds", {}))
    for d in s.get("divergences", []):
        if d["id"] in divergences:
            divergences[d["id"]]["count"] += d["count"]
        else:
            divergences[d["id"]] = dict(d)

refused = verdicts.get("refused", 0)
crashed = verdicts.get("crashed", 0)
evaluated = attempts - refused - crashed
total_div = sum(kinds.values())

print()
print("=" * 68)
print(f"shadow nixpkgs sweep{(': ' + label) if label else ''}")
print(f"  binary sha256   {sha}")
print(f"  nixpkgs         {nixpkgs}")
print(f"  wall clock      {elapsed}s")
print("=" * 68)
print()
print("coverage, reported two ways (neither number means anything alone)")
print(f"  attributes attempted            {n_attrs}")
print(f"  stats files read                {stats_read}   (a process that died writes none)")
print(f"  shadow attempts                 {attempts}")
print(f"  of those, the rust arm REFUSED  {refused}")
print(f"  of those, the rust arm CRASHED  {crashed}")
print(f"  so the rust arm EVALUATED       {evaluated}"
      + (f"   ({100.0*evaluated/attempts:.1f}% of attempts)" if attempts else ""))
print(f"  unaccounted                     {unaccounted}   (attempts that reached no verdict)")
print(f"  rust arm time                   {micros/1e6:.2f}s   (COLD every call: ENG-12830)")
print()
print("verdicts")
for k, v in sorted(verdicts.items()):
    print(f"  {k:<32} {v}")
print()
print("refusal tokens (a refusal is NOT a divergence)")
if tokens:
    for k, v in sorted(tokens.items(), key=lambda kv: (-kv[1], kv[0])):
        print(f"  {k:<32} {v}")
else:
    print("  none")
print()
print("skipped")
for k, v in sorted(skips.items()):
    print(f"  {k:<32} {v}")
print()
print(f"divergence kinds, over the {evaluated} evaluated")
for k, v in sorted(kinds.items()):
    print(f"  {k:<32} {v}")
print()
print(f"distinct divergences: {len(divergences)}")
for d in sorted(divergences.values(), key=lambda d: (-d["count"], d["id"])):
    print(f"  {d['id']}  x{d['count']}  {d['kind']}  {d['origin']}")
    print(f"      {d['detail'][:300]}")

failures = []
if attempts == 0:
    failures.append("no evaluation was shadowed, so every zero above is vacuous")
if stats_read == 0:
    failures.append("no stats file was written at all")
if unaccounted:
    failures.append(f"{unaccounted} attempt(s) reached no verdict")
# A sweep that compared far fewer things than it was asked to is not a clean
# sheet, it is a broken run. Without this, a wrapper that failed to resolve
# reports attempts=0 divergences=0 and reads as a pass.
if attempts < n_attrs:
    failures.append(f"only {attempts} of {n_attrs} attributes were shadowed at all")
if expect_token:
    top = tokens.most_common(1)
    if not top or top[0][0] != expect_token:
        failures.append(f"expected '{expect_token}' to be the majority refusal token, got {top or 'none'}")

print()
print(f"RESULT attrs={n_attrs} attempts={attempts} refused={refused} evaluated={evaluated} "
      f"divergences={total_div} distinct={len(divergences)} unaccounted={unaccounted} "
      f"root_servable={root_servable} sha={sha[:12]}")
for f in failures:
    print(f"shadow-nixpkgs-sweep: {f}", file=sys.stderr)
sys.exit(1 if failures else 0)
PY
