#!/usr/bin/env bash
# Differentially run tests/functional/lang's eval corpus under two evaluator
# arms of ONE nix build and diff the outcomes. The corpus .exp files are not
# consulted: arm A is the oracle, so the comparison is live A-vs-B on the same
# binary and can never suffer version skew or stale expectations.
#
#   lang-diff.sh NIXBINDIR ARM_A ARM_B [--only GLOB]
#   lang-diff.sh NIXBINDIR --self-diff [--only GLOB]
#
# NIXBINDIR: directory containing the nix-instantiate and nix binaries under
#            test. Passed explicitly, never taken from PATH, because a
#            differ that silently picks up an ambient nix measures the wrong
#            thing; the binary path, its sha256 and its version are printed
#            on the RESULT line.
# --only:    run only the pairs whose test name matches this shell glob
#            (e.g. --only 'eval-okay-attrs*', or --only '@(*sort*|*map*)' for
#            a set of related cases), for a seconds-long loop while
#            working one case. The RESULT line always reports corpus= (every
#            pair discovered) beside pairs= (the ones run), so a filtered run
#            can never be read as a full one, and a glob matching nothing
#            trips the same zero-pairs refusal an empty corpus does.
# ARM:       SETTING=VALUE injected via NIX_CONFIG, or "none" for the plain
#            default path. eval-backend=rust also enables the rust-eval
#            experimental feature, which that setting needs and nothing else
#            does (same shape as eval-identity-harness.sh's eval-cores rule).
#
# --self-diff runs arm "none" against arm "none": every pair must match, and
# the arms-really-differ gate is waived because identical arms are the point.
# This mode is the harness's own permanent smoke test: a runner change that
# breaks comparison shows up here as a mismatch with no evaluator involved.
#
# --max-unimplemented, --min-match:
#            bounds on the two counts that a differ can otherwise satisfy by
#            measuring nothing. `unimplemented` neither passes nor fails, so
#            a construct that stops being implemented used to move a case out
#            of the comparison silently (ENG-12438); `match` is the only
#            outcome that proves the arms agree about a VALUE rather than
#            about a failure class. Both default to the numbers checked into
#            gate-ratchets.sh and are enforced by the exit status.
#
# Arms-really-differ gate (non-self-diff runs): when both arms set the same
# SETTING, `nix config show SETTING` must return different values under the
# two arms, else the run refuses. Exists because an option that never reaches
# nix leaves two arms byte-identical and reports a pass that means nothing
# (the eval-identity-harness.sh lesson).
#
# Capability gate, which is the stronger one: each arm evaluates a trivial
# expression under NIX_SHOW_STATS and its `evaluator` field must name the
# backend the arm asked for. That field is derived from a per-backend count
# of evaluations served (ENG-12542), so it reports what ran; it used to echo
# `eval-backend` back, which is the same inert-flag failure this whole header
# is about. An arm pointed at a binary compiled without the Rust evaluator
# reports `cpp` here and the run refuses.
#
# Per-case outcome lattice, ordered by precedence:
#   crash          either arm died by signal (exit >= 128)
#   corpus-fail    arm A (the oracle) did not behave as the corpus name says
#   unimplemented  arm B stderr says "rust-eval unimplemented:"
#   allowlisted    outcomes differ but the case is in eval-allowlist.toml
#   mismatch       eval-okay: stdout bytes or exit differ;
#                  eval-fail: arm B succeeded, or error CLASS differs
#   match / fail-as-fail
#
# Error text is never byte-compared for eval-fail pairs; only the class from
# a fixed enum {parse, throw, assert, type, missing-attr, infinite-recursion,
# stack-overflow, abort, context, unimplemented, unknown}. unknown-vs-unknown
# counts as a class match only when the two arms' TERMINAL error lines are
# identical, so a genuinely novel divergence cannot hide inside "unknown".
#
# The class token is the INTENDED bar here, not a temporary weakening on the
# way to byte parity (CLAUDE.md, "Parity bar"). Error wording is tier 2:
# functional equivalence suffices, and chasing byte parity of prose is not
# worth the effort. Do not escalate this comparison. The byte fallback inside
# `unknown` is not an escalation either -- it is what is left when the
# classifier has no name for the failure, and the right response to landing
# there often is to give the classifier a name, as the `context` token was.
#
# Terminal line, not the whole stream: a cppnix failure prints `error:`, then
# trace notes with file/line/source excerpts, then the real message; the Rust
# arm carries no source positions (ENG-12137) and prints the message alone.
# Comparing whole streams therefore made the position block, not the error,
# decide every unknown pair. The terminal line is still compared byte for byte
# after unindenting, so two different errors still differ.
#
# `assert` also accepts cppnix's assertEqValues family ("... is not equal to
# ...", "attribute names of attribute set ... differs from ..."), which is the
# detailed diagnostic cppnix produces for a false `assert a == b`. Both arms
# raise cppnix's AssertionError; only the message differs, because the Rust
# arm reports the generic "assertion failed" (ENG-12138).
#
# Exit 0 iff pairs > 0, mismatch = crash = corpus-fail = 0, unimplemented is
# at or under --max-unimplemented and match is at or over --min-match. An
# empty corpus is a failure (exit 2), never a pass: a glob that matched
# nothing must not read as "nothing diverged".
set -u
# nullglob is load-bearing: without it an empty corpus leaves the literal
# pattern in the loop, which evaluates as a file, fails, and counts as one
# fail-as-fail pair instead of tripping the zero-pairs refusal. Found by
# breaking the guard on purpose; keep the break-it test when changing this.
shopt -s nullglob
# extglob so --only can name a set: '@(*sort*|*map*)' is the shape a chunk of
# related cases wants, and a plain glob cannot express alternation. Adding it
# cannot change how an existing pattern matches; extglob only recognises the
# ?( *( +( @( !( forms, which are syntax errors without it.
shopt -s extglob

usage() { grep '^#' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }

# The checked-in expectations. Sourced before the flag loop so a flag can
# override one, and so a missing file is a refusal rather than an unbound
# variable that `set -u` reports three hundred lines later.
# Absolute, because this script cds into tests/functional and every helper
# beside it would otherwise be looked up relative to there. That is not
# hypothetical: the validator below was resolved against tests/functional and
# reported "No such file", which the caller read as a failed validation.
here=$(cd "$(dirname "$0")" && pwd)
ratchets=$here/gate-ratchets.sh
[ -f "$ratchets" ] || { echo "lang-diff: no gate-ratchets.sh beside this script" >&2; exit 2; }
# shellcheck source=./gate-ratchets.sh
. "$ratchets" || exit 2
# shellcheck source=./arm-config.sh
. "$here/arm-config.sh" || exit 2
# One owner of the gates' nix configuration, before anything reads the
# environment: an ambient `lint-url-literals = fatal` otherwise makes every
# rust arm refuse and every row score `unimplemented` (ENG-12996).
arm_pin_environment

ONLY=
MAX_UNIMPL=$LANG_DIFF_MAX_UNIMPLEMENTED
MIN_MATCH=$LANG_DIFF_MIN_MATCH
rest=()
while [ $# -gt 0 ]; do
  case $1 in
    --only)
      [ $# -ge 2 ] || { echo "lang-diff: --only needs a glob" >&2; exit 2; }
      ONLY=$2; shift 2 ;;
    --max-unimplemented)
      [ $# -ge 2 ] || { echo "lang-diff: --max-unimplemented needs a number" >&2; exit 2; }
      MAX_UNIMPL=$2; shift 2 ;;
    --min-match)
      [ $# -ge 2 ] || { echo "lang-diff: --min-match needs a number" >&2; exit 2; }
      MIN_MATCH=$2; shift 2 ;;
    *) rest+=("$1"); shift ;;
  esac
done
for n in "$MAX_UNIMPL" "$MIN_MATCH"; do
  case $n in
    ''|*[!0-9]*) echo "lang-diff: bounds must be non-negative integers, got '$n'" >&2; exit 2 ;;
  esac
done
# The +expansion guard is load-bearing under `set -u`: bash 3.2, which is what
# /bin/bash is on darwin, errors on "${rest[@]}" when the array is empty, and
# an empty array is exactly the no-arguments case that must reach usage().
set -- ${rest[@]+"${rest[@]}"}

[ $# -ge 2 ] || usage
NIXBINDIR=$1; shift
SELF_DIFF=0
if [ "$1" = --self-diff ]; then
  SELF_DIFF=1; ARM_A=none; ARM_B=none
else
  [ $# -eq 2 ] || usage
  ARM_A=$1; ARM_B=$2
fi

# Absolutised before the cd into tests/functional, for the same reason the
# helper paths are: a relative NIXBINDIR resolves against the corpus
# directory instead, and the arms-differ gate then reports "cannot read
# effective 'eval-backend'" -- a missing binary wearing the costume of a
# real finding. Caught by making exactly that mistake.
[ -d "$NIXBINDIR" ] || { echo "lang-diff: no such directory: $NIXBINDIR" >&2; exit 2; }
NIXBINDIR=$(cd "$NIXBINDIR" && pwd)
NIX_INSTANTIATE=$NIXBINDIR/nix-instantiate
NIX=$NIXBINDIR/nix
for b in "$NIX_INSTANTIATE" "$NIX"; do
  [ -x "$b" ] || { echo "lang-diff: not executable: $b" >&2; exit 2; }
done

repo_root=$(cd "$here/../.." && pwd)
cd "$repo_root/tests/functional" || exit 2
allowlist=$repo_root/maintainers/ix/eval-allowlist.toml

# sha256 of the binary so a report can never be attributed to the wrong build
# (shasum is darwin, sha256sum is coreutils; fail loudly rather than skip).
bin_sha() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}'
  else echo "lang-diff: no sha256 tool on PATH" >&2; exit 2; fi
}

# The three parser lints, pinned into every arm rather than inherited, the way
# shadow-corpus.sh does it. `NIX_CONFIG` is applied on top of whatever conf
# files are in scope, so a machine with `lint-url-literals = fatal` in
# ~/.config/nix/nix.conf used to make the Rust arm refuse every evaluation by
# name (`command-parser-lint`, retired when the compiler grew the lints).
# Both arms honour a fatal lint now, but an inherited one is still a fact
# about the machine, and pinning here keeps it out of the comparison.
#
# `warn` and not `ignore`, so a corpus case that would trip a lint still says
# so, and both arms say it identically.
#
# Pinned for `none` too. "none" names the arm's evaluator setting, not an
# absence of configuration, and an arm that inherits the machine's lints is
# not comparable with one that does not.
# The parser lints, and everything else the ambient configuration would
# otherwise leak in, now come from `arm-config.sh` -- one owner for every gate
# here (ENG-12996). They are `ignore` there rather than the `warn` this line
# used: the reasoning for `warn` was that a corpus case tripping a lint should
# still say so, and that only holds if both arms can say it. The rust backend
# has no parser lint, so at `warn` the cpp arm prints and the rust arm cannot,
# which is a guaranteed difference on every row containing an absolute path.
lang_diff_lints=$(arm_base_config)

arm_config() { # ARM -> NIX_CONFIG contents on stdout
  case $1 in
    none) ;;
    eval-backend=rust) printf 'extra-experimental-features = rust-eval\neval-backend = rust\n' ;;
    *=*) printf '%s = %s\n' "${1%%=*}" "${1#*=}" ;;
    *) echo "lang-diff: bad arm spec '$1' (want SETTING=VALUE or none)" >&2; exit 2 ;;
  esac
  printf '%s\n' "$lang_diff_lints"
}

# Hashed before the first pair rather than after the last, and checked again
# at the end: a rebuild landing mid-run silently swaps the binary under the
# loop, and a hash taken afterwards then attributes every earlier pair to a
# build that never ran them. Found by doing exactly that.
BIN_SHA=$(bin_sha "$NIX_INSTANTIATE")

CONFIG_A=$(arm_config "$ARM_A")
CONFIG_B=$(arm_config "$ARM_B")

# The runner isolates nix config and state the way common.sh does for
# lang.sh: without this, the machine's nix.conf and existing channel
# profiles leak two extra entries into builtins.nixPath and
# eval-okay-search-path fails on a count assertion. NIX_STATE_DIR points
# at a path that does not exist, on purpose. Created here, above the
# capability probe, because the probe writes its stats file into it.
tmp=$(mktemp -d /tmp/lang-diff.XXXXXX)
mkdir -p "$tmp/conf"
# Same grant as common/init.sh's test nix.conf: two lang tests parse flake
# refs and need the flakes feature. Arms still layer on top via NIX_CONFIG,
# which nix applies after conf files.
printf "experimental-features = nix-command flakes\n" > "$tmp/conf/nix.conf"
trap 'rm -rf "$tmp"' EXIT

# The isolation every invocation of the binary under test runs under, the
# capability probe and the arms-differ gate included. Those two used to run
# bare, so they read the machine's nix.conf while the scored pairs did not:
# the probe certified a configuration nobody scored, and the gate could refuse
# or pass for reasons no pair would ever see. ENG-12871.
iso_env=(
  NIX_CONF_DIR="$tmp/conf"
  NIX_USER_CONF_FILES=''
  NIX_STATE_DIR="$tmp/state-nonexistent"
)

# Arms-really-differ gate. Waived for self-diff (identical arms are the point)
# and for arms naming different settings (nothing comparable to probe).
if [ "$SELF_DIFF" = 0 ] && [ "${ARM_A%%=*}" = "${ARM_B%%=*}" ] && [ "$ARM_A" != none ]; then
  setting=${ARM_A%%=*}
  eff_a=$(env "${iso_env[@]}" NIX_CONFIG="$CONFIG_A" "$NIX" config show "$setting" 2>&1) || {
    echo "lang-diff: cannot read effective '$setting' under arm A; refusing a run whose arms cannot be told apart:" >&2
    echo "$eff_a" >&2; exit 2; }
  eff_b=$(env "${iso_env[@]}" NIX_CONFIG="$CONFIG_B" "$NIX" config show "$setting" 2>&1) || {
    echo "lang-diff: cannot read effective '$setting' under arm B:" >&2
    echo "$eff_b" >&2; exit 2; }
  if [ "$eff_a" = "$eff_b" ]; then
    echo "lang-diff: arms are indistinguishable (effective $setting='$eff_a' in both); a pass would mean nothing" >&2
    exit 2
  fi
fi

selected() { # test name -> 0 when --only admits it (everything, when unset)
  [ -n "$ONLY" ] || return 0
  # shellcheck disable=SC2254 # $ONLY is a glob on purpose, not a literal
  case $1 in $ONLY) return 0 ;; *) return 1 ;; esac
}

arm_evaluator() { # ARM -> the backend NIX_SHOW_STATS must report for it
  # Anything that is not `eval-backend=rust` runs the C++ evaluator, which is
  # the default and what every other SETTING=VALUE arm leaves in place.
  case $1 in
    eval-backend=rust) echo rust ;;
    *) echo cpp ;;
  esac
}

# Capability probe: the arms-differ gate reads `nix config show`, which
# reports the SETTING and so passes even on a binary compiled without the
# rust backend (-Dnix:rust-eval=disabled is the default); such a run scored
# mismatch=249 while measuring a stub. Each arm must actually evaluate a
# trivial expression, AND say which backend evaluated it, before the corpus
# counts anything.
#
# The second half is the one that cannot be faked. NIX_SHOW_STATS's
# `evaluator` field is derived from a count of evaluations each backend
# served (ENG-12542); before that it echoed `eval-backend` straight back, so
# it agreed with the setting no matter what ran. Stats go to a file, never to
# the default stderr, because the eval-fail arm compares stderr.
probe_arm() { # LABEL ARM CONFIG
  local label=$1 arm=$2 config=$3 want got stats evaluator
  got=$(env "${iso_env[@]}" NIX_CONFIG="$config" "$NIX_INSTANTIATE" --eval --strict -E 1 2>&1)
  if [ "$got" != 1 ]; then
    echo "lang-diff: arm $label ($arm) cannot evaluate the probe expression '1'; refusing to score a corpus against it:" >&2
    echo "$got" >&2
    exit 2
  fi
  want=$(arm_evaluator "$arm")
  stats=$tmp/probe-$label.json
  env "${iso_env[@]}" NIX_CONFIG="$config" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$stats" \
    "$NIX_INSTANTIATE" --eval --strict -E 1 > /dev/null 2>"$tmp/probe-$label.err"
  if [ ! -s "$stats" ]; then
    echo "lang-diff: arm $label ($arm) wrote no NIX_SHOW_STATS file; this binary predates the counted 'evaluator' field (ENG-12542) and cannot prove which backend ran" >&2
    sed -n '1,5p' "$tmp/probe-$label.err" >&2
    exit 2
  fi
  evaluator=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("evaluator", "<absent>"))' "$stats") || exit 2
  if [ "$evaluator" != "$want" ]; then
    echo "lang-diff: arm $label ($arm) asked for the '$want' evaluator and NIX_SHOW_STATS reports '$evaluator' ran; a comparison between arms that are both the same backend proves nothing" >&2
    python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print("  evaluatorCalls:", d.get("evaluatorCalls", "<absent>"), file=sys.stderr)' "$stats"
    exit 2
  fi
  echo "lang-diff: arm $label ($arm) evaluates, and NIX_SHOW_STATS confirms the '$evaluator' backend ran" >&2
}
probe_arm A "$ARM_A" "$CONFIG_A"
probe_arm B "$ARM_B" "$CONFIG_B"

# The allowlist is parsed, not grepped. `grep -q '^id = "<name>"'` reads one
# field of four, so tier, reason and approved were never checked by anything
# and an entry could waive a divergence with no reason and a machine's
# approval. The validator refuses the file rather than the case, because a
# malformed waiver is a problem with the waiver list and not with the corpus.
validator=$here/validate-eval-allowlist.py
approvers=$here/eval-allowlist-approvers.txt
if ! allowlist_ids=$(python3 "$validator" "$allowlist" "$approvers" --ids); then
  echo "lang-diff: eval-allowlist.toml did not validate (above); refusing to run against a waiver list nobody has checked" >&2
  exit 2
fi

allowlisted() { # test name -> 0 if listed
  # A whole-line match against the validated id list; a substring match would
  # let `eval-fail-eol` waive `eval-fail-eol-2`.
  printf '%s\n' "$allowlist_ids" | grep -qxF "$1"
}

# Error classification, shared with drv-parity.sh so the two gates cannot
# drift into disagreeing about what counts as the same failure.
# shellcheck source=./error-class.sh
. "$here/error-class.sh" || { echo "lang-diff: cannot source error-class.sh; the eval-fail arm would classify nothing" >&2; exit 2; }

run_arm() { # config file flags... ; stdout->$out stderr->$err, returns exit code
  local config=$1 outf=$2 errf=$3; shift 3
  env "${iso_env[@]}" \
  NIX_CONFIG="$config" \
  NIX_PATH=lang/dir3:lang/dir4 \
  HOME=/fake-home \
  TEST_VAR=foo \
  NIX_REMOTE=dummy:// \
  NIX_STORE_DIR=/nix/store \
  timeout 60 "$NIX_INSTANTIATE" "$@" 1>"$outf" 2>"$errf"
}

pairs=0 corpus=0 match=0 failfail=0 mismatch=0 crash=0 unimpl=0 allow=0 corpusfail=0 skipped=0

report() { echo "$1: $2 $3"; }

for nixf in lang/eval-okay-*.nix; do
  name=$(basename "$nixf" .nix)
  corpus=$((corpus + 1))
  selected "$name" || continue
  pairs=$((pairs + 1))
  if [ -e "lang/$name.exp-disabled" ]; then skipped=$((skipped + 1)); continue; fi

  declare -a flags=()
  if [ -e "lang/$name.flags" ]; then read -r -a flags < "lang/$name.flags"; fi
  if [ -e "lang/$name.exp.xml" ]; then
    flags+=(--eval --xml --no-location --strict)
  else
    flags+=(--eval --strict)
  fi

  run_arm "$CONFIG_A" "$tmp/a.out" "$tmp/a.err" "${flags[@]}" "lang/$name.nix"; ec_a=$?
  run_arm "$CONFIG_B" "$tmp/b.out" "$tmp/b.err" "${flags[@]}" "lang/$name.nix"; ec_b=$?

  if [ "$ec_a" -ge 128 ] || [ "$ec_b" -ge 128 ]; then
    crash=$((crash + 1)); report CRASH "$name" "exit a=$ec_a b=$ec_b"; continue
  fi
  if [ "$ec_a" -ne 0 ]; then
    corpusfail=$((corpusfail + 1)); report CORPUS-FAIL "$name" "oracle arm failed (exit $ec_a)"; continue
  fi
  if grep -q 'rust-eval unimplemented' "$tmp/b.err"; then
    unimpl=$((unimpl + 1)); continue
  fi
  if [ "$ec_b" -eq 0 ] && cmp -s "$tmp/a.out" "$tmp/b.out"; then
    match=$((match + 1))
  elif allowlisted "$name"; then
    allow=$((allow + 1))
  else
    mismatch=$((mismatch + 1))
    report MISMATCH "$name" "exit a=$ec_a b=$ec_b; stdout $(cmp -s "$tmp/a.out" "$tmp/b.out" && echo equal || echo differs)"
  fi
done

for nixf in lang/eval-fail-*.nix; do
  name=$(basename "$nixf" .nix)
  corpus=$((corpus + 1))
  selected "$name" || continue
  pairs=$((pairs + 1))

  # The .flags file ADDS to the defaults; it used to replace them. Four corpus
  # files carry one, and eval-fail-infinite-recursion-lambda's holds only
  # `--max-call-depth 100`, so the invocation carried no `--eval`, the Rust
  # arm refused the instantiate path before evaluating anything, and the one
  # case that reproduces ENG-12432 scored `unimplemented` -- a bucket that
  # neither passes nor fails, so it read as handled (ENG-12438). The
  # eval-okay loop twenty lines up has always appended; this is the same rule.
  flags_str="--eval --strict --show-trace"
  if [ -e "lang/$name.flags" ]; then
    flags_str="$(sed -e 's/#.*//' < "lang/$name.flags") $flags_str"
  fi
  # shellcheck disable=SC2086 # word splitting of flags is intended, as in lang.sh
  run_arm "$CONFIG_A" "$tmp/a.out" "$tmp/a.err" $flags_str "lang/$name.nix"; ec_a=$?
  # shellcheck disable=SC2086
  run_arm "$CONFIG_B" "$tmp/b.out" "$tmp/b.err" $flags_str "lang/$name.nix"; ec_b=$?

  if [ "$ec_a" -ge 128 ] || [ "$ec_b" -ge 128 ]; then
    crash=$((crash + 1)); report CRASH "$name" "exit a=$ec_a b=$ec_b"; continue
  fi
  if [ "$ec_a" -eq 0 ]; then
    corpusfail=$((corpusfail + 1)); report CORPUS-FAIL "$name" "oracle arm succeeded on an eval-fail case"; continue
  fi
  if grep -q 'rust-eval unimplemented' "$tmp/b.err"; then
    unimpl=$((unimpl + 1)); continue
  fi
  if [ "$ec_b" -eq 0 ]; then
    if allowlisted "$name"; then allow=$((allow + 1)); else
      mismatch=$((mismatch + 1)); report MISMATCH "$name" "arm B evaluated a must-fail case"
    fi
    continue
  fi
  class_a=$(error_class "$tmp/a.err"); class_b=$(error_class "$tmp/b.err")
  ok=0
  if [ "$class_a" = "$class_b" ]; then
    if [ "$class_a" = unknown ]; then
      ea=$(last_error "$tmp/a.err"); eb=$(last_error "$tmp/b.err")
      if [ -n "$ea" ] && [ "$ea" = "$eb" ]; then
        ok=1
      elif cmp -s "$tmp/a.err" "$tmp/b.err"; then
        # No terminal "error:" line to compare -- a message carrying invalid
        # UTF-8 has none, because stripping colour mangles it. Byte equality
        # is the only remaining evidence, and it is what --self-diff needs:
        # identical arms must match, and this pair is why. Found by the
        # self-diff smoke test, which is the whole reason it exists.
        ok=1
      fi
    else
      ok=1
    fi
  fi
  if [ "$ok" = 1 ]; then
    failfail=$((failfail + 1))
  elif allowlisted "$name"; then
    allow=$((allow + 1))
  else
    mismatch=$((mismatch + 1)); report MISMATCH "$name" "error class a=$class_a b=$class_b"
  fi
done

ver=$("$NIX_INSTANTIATE" --version | head -1)
sha_after=$(bin_sha "$NIX_INSTANTIATE")
if [ "$sha_after" != "$BIN_SHA" ]; then
  echo "lang-diff: the binary changed during the run ($BIN_SHA -> $sha_after); these counts mix two builds and mean nothing" >&2
  exit 2
fi
echo "RESULT lang-diff bin=$NIX_INSTANTIATE sha256=$BIN_SHA version='$ver' armA=$ARM_A armB=$ARM_B \
only='$ONLY' pairs=$pairs corpus=$corpus match=$match fail-as-fail=$failfail mismatch=$mismatch crash=$crash unimplemented=$unimpl allowlisted=$allow corpus-fail=$corpusfail skipped=$skipped \
max-unimplemented=$MAX_UNIMPL min-match=$MIN_MATCH ratchets-from=$GATE_RATCHETS_MEASURED_AT@$GATE_RATCHETS_MEASURED_ON"

if [ "$pairs" -eq 0 ]; then
  if [ -n "$ONLY" ]; then
    echo "lang-diff: --only '$ONLY' selected none of the $corpus pairs; a filter that matched nothing is a failure, not a pass" >&2
  else
    echo "lang-diff: zero pairs discovered; an empty corpus is a failure, not a pass" >&2
  fi
  exit 2
fi

# The two bounds. Without them the exit asked only that nothing was WRONG,
# which a run measuring nothing satisfies perfectly: every pair landing in
# `unimplemented` gives mismatch=0 and exit 0. They are skipped for a
# filtered run, where the counts are a slice of the corpus and comparing
# them against whole-corpus numbers would fail every --only invocation.
bounds_ok=1
if [ -n "$ONLY" ]; then
  echo "lang-diff: --only was given, so the unimplemented/match bounds are not applied to this slice" >&2
else
  if [ "$unimpl" -gt "$MAX_UNIMPL" ]; then
    echo "lang-diff: unimplemented=$unimpl is over the checked-in bound of $MAX_UNIMPL. A pair that lands here is not compared at all, so this is coverage leaving the gate, not a neutral outcome. Fix the construct, or raise the bound in gate-ratchets.sh in the same commit that explains why." >&2
    bounds_ok=0
  fi
  if [ "$match" -lt "$MIN_MATCH" ]; then
    echo "lang-diff: match=$match is under the checked-in floor of $MIN_MATCH. match is the only outcome that proves the arms agree about a VALUE; a backend that refused everything would otherwise report mismatch=0 and pass." >&2
    bounds_ok=0
  fi
fi

[ "$mismatch" -eq 0 ] && [ "$crash" -eq 0 ] && [ "$corpusfail" -eq 0 ] && [ "$bounds_ok" -eq 1 ]
