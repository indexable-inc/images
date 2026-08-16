#!/usr/bin/env bash
# Run tests/functional/lang's eval corpus under `eval-backend = shadow` and
# report the divergence histogram.
#
#   shadow-corpus.sh NIXBINDIR [--only GLOB] [--max-divergences N]
#
# Shadow serves the C++ answer and runs the Rust evaluator beside it, so this
# is not a pass/fail differ like lang-diff.sh -- the corpus outcomes are the
# C++ ones either way. What it measures is what shadow *saw*: how many
# evaluations it compared, how many agreed, how many the Rust arm refused, and
# every divergence with its stable id.
#
# Why this exists next to lang-diff.sh rather than inside it: lang-diff runs
# two processes and compares their streams, which is the right shape for a
# gate and the wrong shape for measuring shadow, whose whole claim is that one
# process can do the comparison in-band. A shadow bug that made the in-process
# comparison disagree with the two-process one would be invisible to
# lang-diff and is exactly what this can see.
#
# Two capability gates, both of the shape CLAUDE.md warns about ("a setting is
# not a capability"):
#
#   1. The binary really has the Rust evaluator. `eval-backend = rust`
#      reports fine on a build compiled without it, so the probe evaluates
#      `1` and requires the answer.
#   2. Shadow really shadowed. A build where the comparison never runs
#      reports zero divergences, which reads exactly like a clean run. The
#      probe requires `shadow.attempts >= 1` on a trivial expression before
#      any corpus number is believed.
#
# Exit 0 iff both probes pass, at least one case ran, no attempt went
# unaccounted for, and the divergence count is at or under --max-divergences
# (default: unbounded, because the first honest number is the point and a
# ratchet can be set once there is one).
set -u
# shellcheck source=./arm-config.sh
. "$(cd "$(dirname "$0")" && pwd)/arm-config.sh" || exit 2
# One owner of the gates' nix configuration, before anything reads the
# environment: an ambient `lint-url-literals = fatal` otherwise makes every
# rust arm refuse and every row score `unimplemented` (ENG-12996).
arm_pin_environment

shopt -s nullglob
shopt -s extglob

usage() { grep '^#' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }

bindir=${1:-}
[ -n "$bindir" ] || usage
shift
only='*'
max_divergences=-1
while [ $# -gt 0 ]; do
  case $1 in
    --only) only=${2:-}; shift 2 ;;
    --max-divergences) max_divergences=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done

# Absolute before anything else: the corpus loop runs from inside the lang
# directory, and a relative binary path stops resolving there. The failure is
# not loud -- every case exits non-zero, no stats file is written, and the run
# reports zero divergences over zero attempts, which is precisely the vacuous
# pass the probes below exist to refuse. (They did refuse it, which is how
# this line came to be written.)
bindir=$(cd "$bindir" && pwd) || exit 2
instantiate=$bindir/nix-instantiate
[ -x "$instantiate" ] || { echo "shadow-corpus: no nix-instantiate at $instantiate" >&2; exit 2; }

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
lang=$root/tests/functional/lang
[ -d "$lang" ] || { echo "shadow-corpus: no corpus at $lang" >&2; exit 2; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Identify the binary by path and hash, never by --version: a checkout build's
# version string carries no revision, so two different binaries print the same
# thing.
echo "shadow-corpus: binary $instantiate sha256=$(shasum -a 256 "$instantiate" | cut -d' ' -f1)"


run_under() { # NIX_CONFIG-lines statsfile args...
  local config=$1 stats=$2; shift 2
  NIX_CONFIG="$config" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$stats" \
    "$instantiate" "$@" > "$work/out" 2> "$work/err"
  echo $?
}

# The three parser lints are pinned rather than inherited. `NIX_CONFIG` is
# applied on top of the user's nix.conf, not instead of it, and a machine with
# `lint-url-literals = fatal` in ~/.config/nix/nix.conf used to make the
# bridge refuse every single evaluation by name (`command-parser-lint`, a
# token retired when the compiler grew the lints) before it reached the
# corpus -- a histogram of one row, about the machine rather than the
# evaluator. This one is not hypothetical: it is what the first run of this
# script on the author's Mac produced. The arms agree about fatal lints now,
# but an inherited lint still makes the histogram about the machine.
#
# From `arm-config.sh`, the one owner of the gates' nix configuration
# (ENG-12996), which also drops the developer's conf file rather than only
# layering three settings on top of it. `ignore` and not the `warn` this line
# used; see that file for why a `warn` the rust arm cannot emit is a
# guaranteed difference rather than useful signal.
lints=$(arm_base_config)

rust_config="extra-experimental-features = rust-eval
eval-backend = rust
$lints"
shadow_config="extra-experimental-features = rust-eval
eval-backend = shadow
$lints"

# Every setting that changes what the Rust arm is allowed to do, printed
# rather than assumed. `pure-eval` in particular makes the bridge refuse every
# question that reaches the filesystem, so a run under it produces a histogram
# of `access-control` and says nothing about the evaluator.
echo "shadow-corpus: ambient settings that decide what is measured:"
NIX_CONFIG="$shadow_config" "$instantiate" --version > /dev/null 2>&1
for setting in pure-eval restrict-eval lint-url-literals lint-short-path-literals lint-absolute-path-literals; do
  value=$(NIX_CONFIG="$shadow_config" "$bindir/nix" config show "$setting" 2>/dev/null || echo '?')
  echo "  $setting = $value"
done

# Probe 1: the Rust evaluator is linked in and answers.
rc=$(run_under "$rust_config" "$work/probe-rust.json" --eval --strict -E 1)
probe=$(cat "$work/out")
if [ "$rc" != 0 ] || [ "$probe" != 1 ]; then
  echo "shadow-corpus: this binary does not evaluate through the Rust backend" >&2
  echo "  eval-backend=rust, --eval --strict -E 1 -> rc=$rc, stdout='$probe'" >&2
  sed -e 's/^/  /' "$work/err" >&2
  exit 2
fi
echo "shadow-corpus: probe 1 ok, the Rust backend evaluates"

# Probe 2: shadow really compares. Zero attempts and zero divergences look
# identical from the histogram, and only one of them is a clean run.
rc=$(run_under "$shadow_config" "$work/probe-shadow.json" --eval --strict -E 1)
probe=$(cat "$work/out")
attempts=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("shadow",{}).get("attempts",0))' "$work/probe-shadow.json" 2>/dev/null || echo 0)
if [ "$rc" != 0 ] || [ "$probe" != 1 ] || [ "$attempts" -lt 1 ]; then
  echo "shadow-corpus: shadow did not shadow" >&2
  echo "  eval-backend=shadow, --eval --strict -E 1 -> rc=$rc, stdout='$probe', attempts=$attempts" >&2
  sed -e 's/^/  /' "$work/err" >&2
  exit 2
fi
echo "shadow-corpus: probe 2 ok, shadow compared $attempts evaluation(s) of a trivial expression"

# The corpus. Both families: an eval-fail case exercises the both-arms-failed
# branch of the comparator, which is half of it and the half a
# success-only run would never touch.
# From inside the corpus directory, and named relatively, which is what
# lang-diff.sh does and is not cosmetic. cppnix answers `__curPos` differently
# for the same file named absolutely under a symlinked prefix: on macOS, where
# /tmp is a symlink to /private/tmp, `nix-instantiate --eval --strict
# /tmp/.../eval-okay-curpos.nix` says `[ 1 17 1 35 ]` and the relative spelling
# says `[ 3 7 4 9 ]`, the second being the corpus's own .exp. That is cppnix's
# bug and not this backend's -- both arms of `eval-backend` reproduce it -- but
# a harness that walked into it would report a value divergence about the
# oracle. Filed separately; see the shadow report.
cases=0
mkdir -p "$work/stats"
cd "$lang" || exit 2
for nix_file in eval-okay-$only.nix eval-fail-$only.nix; do
  name=${nix_file%.nix}
  extra=()
  # The corpus's own per-case flags, ADDED to --eval rather than replacing it
  # (ENG-12438), which is what lang-diff.sh learned to do.
  [ -f "$name.flags" ] && read -r -a extra < "$name.flags"
  cases=$((cases + 1))
  run_under "$shadow_config" "$work/stats/$name.json" --eval --strict "${extra[@]}" "$nix_file" > /dev/null
done
cd - > /dev/null || exit 2

if [ "$cases" -eq 0 ]; then
  echo "shadow-corpus: no cases matched --only '$only'; refusing to report zero divergences over an empty corpus" >&2
  exit 2
fi

python3 - "$work/stats" "$cases" "$max_divergences" <<'PY'
import json, pathlib, sys
from collections import Counter

stats_dir, cases, max_divergences = pathlib.Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])

attempts = 0
unaccounted = 0
micros = 0
verdicts, tokens, skips, kinds = Counter(), Counter(), Counter(), Counter()
divergences = {}
read = 0

for path in sorted(stats_dir.glob("*.json")):
    try:
        blob = json.loads(path.read_text())
    except (OSError, ValueError):
        # A case whose process died before writing stats. Counted, because
        # "the file is missing" and "the file says zero" are different facts
        # and only one of them is a clean run.
        continue
    read += 1
    shadow = blob.get("shadow", {})
    attempts += shadow.get("attempts", 0)
    unaccounted += shadow.get("unaccounted", 0)
    micros += shadow.get("rustMicros", 0)
    verdicts.update(shadow.get("verdicts", {}))
    tokens.update(shadow.get("refusalTokens", {}))
    skips.update(shadow.get("skipped", {}))
    kinds.update(shadow.get("divergenceKinds", {}))
    for d in shadow.get("divergences", []):
        # Add to an existing row, or start one. Written as an explicit
        # membership test because the obvious `setdefault` version double
        # counts the first sighting -- which is how the first corpus run
        # reported every divergence as x2.
        if d["id"] in divergences:
            divergences[d["id"]]["count"] += d["count"]
        else:
            divergences[d["id"]] = dict(d)

print()
print(f"cases run:            {cases}")
print(f"stats files read:     {read}   (a case whose process died writes none)")
print(f"shadow attempts:      {attempts}")
print(f"rust arm time:        {micros / 1e6:.2f}s")
print(f"unaccounted:          {unaccounted}   (attempts that reached no verdict)")
print()
print("verdicts")
for name, n in sorted(verdicts.items()):
    print(f"  {name:<32} {n}")
print()
print("refusal tokens (rust arm, under shadow)")
if tokens:
    for name, n in sorted(tokens.items(), key=lambda kv: (-kv[1], kv[0])):
        print(f"  {name:<32} {n}")
else:
    print("  none")
print()
print("skipped")
for name, n in sorted(skips.items()):
    print(f"  {name:<32} {n}")
print()
print("divergence kinds")
for name, n in sorted(kinds.items()):
    print(f"  {name:<32} {n}")
print()
print(f"distinct divergences: {len(divergences)}")
for d in sorted(divergences.values(), key=lambda d: (-d["count"], d["id"])):
    print(f"  {d['id']}  x{d['count']}  {d['kind']}  {d['origin']}")
    print(f"      {d['detail']}")

total_divergences = sum(kinds.values())
failures = []
if attempts == 0:
    failures.append("no evaluation was shadowed, so every zero below is vacuous")
if unaccounted:
    failures.append(f"{unaccounted} attempt(s) reached no verdict")
if max_divergences >= 0 and total_divergences > max_divergences:
    failures.append(f"{total_divergences} divergences exceeds --max-divergences {max_divergences}")

print()
print(f"RESULT cases={cases} attempts={attempts} divergences={total_divergences} unaccounted={unaccounted}")
for f in failures:
    print(f"shadow-corpus: {f}", file=sys.stderr)
sys.exit(1 if failures else 0)
PY
