#!/usr/bin/env bash
#
# Does `nix eval` give the same bytes under both backends?
#
# This is a differential gate, not a golden-output one: every case runs twice
# through the same binary, once with `eval-backend = cpp` and once with
# `rust`, and the two are compared byte for byte on stdout and by exit code.
# A golden file would have to be updated whenever cppnix changed its printer,
# and would then be asserting what somebody wrote down rather than what the
# other evaluator does.
#
# Separate from rust-eval-cache-cli.sh, which is about the on-disk cache: that
# one wants a cache directory set and times a second process, this one wants
# neither and would be slower and less readable for the mixture.
#
# Needs a built nix with the Rust evaluator linked in:
#   meson setup build -Dnix:rust-eval=enabled && ninja -C build
# Point it at one with NIX_BUILD_DIR; the default matches the other gates.
set -u

BUILD=${NIX_BUILD_DIR:-$HOME/incr-vm/nix/build}
NIX=$BUILD/src/nix/nix
NIXI=$BUILD/src/nix/nix-instantiate
[ -x "$NIX" ] || { echo "no nix at $NIX (set NIX_BUILD_DIR)"; exit 2; }
[ -x "$NIXI" ] || NIXI=$NIX

# shellcheck source=./gate-ratchets.sh
. "$(cd "$(dirname "$0")" && pwd)/gate-ratchets.sh" || exit 2

W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
# pure-eval is deliberately NOT set here. `nix` assigns it in main.cc, so the
# environment cannot move it -- but naming it in a config file marks the
# setting `overridden`, and `--file` refuses to run against an overridden
# pure-eval in both arms. Pinning it for determinism made every --file case
# below fail identically on both sides, which `same()` scored as a match:
# an assertion whose passing state is an absence. Section 0 prints what the
# settings actually are instead.
# The three parser lints, pinned rather than inherited, for the reason
# lang-diff.sh pins them: `NIX_CONFIG` layers on top of the machine's nix.conf,
# and a `lint-url-literals = fatal` there used to make the rust arm refuse
# every evaluation by name (`command-parser-lint`, a token retired when the
# compiler grew the lints). This gate's own capability probe hit that and
# exited 2 having measured nothing (ENG-12871). Both arms honour a fatal lint
# now, but an inherited one still makes the score a fact about the machine,
# which is what pinning prevents. `warn`, not `ignore`, so a case that would
# trip a lint still says so.
# shellcheck source=./arm-config.sh
. "$(cd "$(dirname "$0")" && pwd)/arm-config.sh" || exit 2
# One owner of the gates' nix configuration (ENG-12996). This gate had the
# seventh copy of the lint list; the copies are gone and the developer's conf
# file is dropped outright, which is what the copies were reaching for.
arm_pin_environment
LINTS=$(arm_base_config)
BASE="extra-experimental-features = nix-command rust-eval
$LINTS"
CPP="$BASE
eval-backend = cpp"
RUST="$BASE
eval-backend = rust"

pairs=0; match=0; mismatch=0; served=0; refused=0; produced=0; empty_agreement=0
# Refusals on a case that was supposed to produce a value. Counted separately
# from `refused` because the two mean opposite things: a refusal on a `same`
# case is this rung's scope, and a refusal on a `serves` case is the gate
# failing to check what it said it would.
serves_refused=0
failures=()
# What same() decided, for serves() to read. A refusal used to leave match
# unchanged, and serves() read "match did not move" as "nothing to do here"
# and returned 0 -- so a pair that refused was silently not counted at all.
last_verdict=none

# Run one command under both backends and require identical stdout and exit
# code. stderr is not compared: the two evaluators word their errors
# differently and always have (eval-allowlist.toml's `error-text` tier), so
# comparing it would fail on cases that agree about everything that matters.
same() {
  local label=$1; shift
  pairs=$((pairs + 1))
  NIX_CONFIG="$CPP" "$@" > "$W/cpp.out" 2> "$W/cpp.err"; local rc_cpp=$?
  NIX_CONFIG="$RUST" "$@" > "$W/rust.out" 2> "$W/rust.err"; local rc_rust=$?

  # A refusal is not a mismatch, it is the scope of this rung. It has to be
  # loud, though: an unnamed refusal is indistinguishable from a crash.
  if [ $rc_rust -ne 0 ] && grep -q "rust-eval unimplemented" "$W/rust.err"; then
    refused=$((refused + 1)); last_verdict=refused
    echo "  REFUSED  $label -- $(grep -o 'rust-eval unimplemented: .*' "$W/rust.err" | head -1)"
    return 0
  fi

  if [ $rc_cpp -eq $rc_rust ] && cmp -s "$W/cpp.out" "$W/rust.out"; then
    match=$((match + 1)); last_verdict=match
    [ $rc_cpp -eq 0 ] && served=$((served + 1))
    return 0
  fi
  mismatch=$((mismatch + 1)); last_verdict=mismatch
  failures+=("$label")
  echo "  MISMATCH $label"
  echo "    rc   cpp=$rc_cpp rust=$rc_rust"
  echo "    cpp  $(head -c 200 "$W/cpp.out" | tr '\n' '~')"
  echo "    rust $(head -c 200 "$W/rust.out" | tr '\n' '~')"
  echo "    rust stderr: $(head -c 200 "$W/rust.err" | tr '\n' '~')"
}

echo "=== 0. capability probe: does this binary really evaluate with the rust backend? ==="
# A setting is not a capability. `nix config show` reports eval-backend = rust
# on a binary compiled without the evaluator, and a gate that reads the
# setting scores a stub; one lang-diff run reached mismatch=249 that way.
probe=$(NIX_CONFIG="$RUST" "$NIX" eval --expr 1 2>&1)
echo "  nix eval --expr 1 => $probe"
[ "$probe" = "1" ] || { echo "  REFUSING: the rust arm cannot evaluate '1'; nothing below would mean anything"; exit 2; }
# These decide which branch several cases below take, so a run on another box
# says what it measured rather than leaving it to be guessed.
NIX_CONFIG="$BASE" "$NIX" config show 2>/dev/null | grep -E "^(pure-eval|restrict-eval|eval-backend) " | sed 's/^/  effective: /'

# Stronger than the probe above, and the check rung G names as the flip test:
# NIX_SHOW_STATS's `evaluator` field says which backend served the
# evaluation. It is derived from a per-backend count (ENG-12542); until then
# it echoed `eval-backend` back, so it agreed with the request whatever ran,
# and an arm pointed at a binary built without the Rust evaluator looked
# identical to one pointed at a working build. Stats go to a file: the
# default sink is stderr, which several cases below read.
for arm in cpp rust; do
  case $arm in cpp) cfg=$CPP ;; *) cfg=$RUST ;; esac
  NIX_CONFIG="$cfg" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/stats-$arm.json" \
    "$NIX" eval --expr '1 + 41' > /dev/null 2>&1
  ev=$(python3 -c 'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("evaluator", "<absent>"))
except OSError:
    print("<no stats file>")' "$W/stats-$arm.json")
  if [ "$ev" != "$arm" ]; then
    echo "  REFUSING: the $arm arm reports evaluator='$ev' under NIX_SHOW_STATS; both arms would be the same backend and every comparison below would be vacuous"
    exit 2
  fi
  echo "  arm $arm: NIX_SHOW_STATS confirms the '$ev' backend ran"
done


# Like same(), and additionally requires that both arms produced a value. Use
# it wherever "they agree" would otherwise be satisfied by both arms failing.
serves() {
  local label=$1; shift
  last_verdict=none
  same "$label" "$@"
  # A refusal here is a failure, not a silence. `serves` is used exactly where
  # "the two arms agree" would otherwise be satisfied by neither producing
  # anything, and a refusal is the strongest form of not producing anything --
  # yet it used to leave `match` unmoved, which this function read as "no
  # value to check" and returned 0. An all-refusing binary passed the gate.
  if [ "$last_verdict" = refused ]; then
    serves_refused=$((serves_refused + 1))
    echo "  NOT SERVED $label -- this case must produce a value; a refusal here is a gap in what the gate covers, not an accepted scope limit"
    return 0
  fi
  if [ "$last_verdict" != match ]; then return 0; fi
  if [ -s "$W/rust.out" ]; then
    produced=$((produced + 1))
  else
    empty_agreement=$((empty_agreement + 1))
    echo "  EMPTY    $label -- both arms agreed and neither printed anything"
    echo "    cpp stderr: $(head -c 200 "$W/cpp.err" | tr '\n' '~')"
  fi
}

echo "=== 1. every type that crosses the handle boundary ==="
for e in '1' '-3' '9223372036854775807' '1.5' '3.0e10' 'true' 'false' 'null' \
         '"hi"' '"a\nb\t\"c\""' '"non-ascii: é"' \
         '[ ]' '[ 1 2 3 ]' '[ [ 1 ] [ ] [ "x" ] ]' \
         '{ }' '{ a = 1; b = "x"; }' '{ "a b" = 1; if = 2; }' \
         '{ a = { b = { c = 42; }; }; }' \
         'x: x' 'builtins.add' 'builtins.add 1' \
         '/tmp/some/path' \
         '{ type = "car"; }' '{ type = 1; }' '{ typeface = "derivation"; }'; do
  same "plain  $e" "$NIX" eval --expr "$e"
done

echo "=== 2. attribute path selection ==="
serves "attrpath a.b.c"      "$NIX" eval --expr '{ a = { b = { c = 42; }; }; }' a.b.c
serves "attrpath one level"  "$NIX" eval --expr '{ a = "top"; }' a
serves "attrpath quoted"     "$NIX" eval --expr '{ "a.b" = 7; }' '"a.b"'
serves "attrpath list index" "$NIX" eval --expr '{ xs = [ 10 20 30 ]; }' xs.1
serves "attrpath into list"  "$NIX" eval --expr '{ xs = [ { y = 1; } ]; }' xs.0.y
same "attrpath missing"    "$NIX" eval --expr '{ a = 1; }' nope
same "attrpath into a non-set" "$NIX" eval --expr '{ a = 1; }' a.b

echo "=== 3. --json ==="
for e in '1' '1.5' 'true' 'null' '"x"' '[ 1 "two" null ]' \
         '{ a = 1; b = [ { c = "d"; } ]; }' '{ }' \
         '{ __toString = self: "coerced"; x = 1; }' \
         '{ outPath = "/nix/store/fake"; other = 1; }'; do
  serves "json   $e" "$NIX" eval --json --expr "$e"
done
serves "json with a selection" "$NIX" eval --json --expr '{ a = { b = [ 1 2 ]; }; }' a.b
same "json of a function"    "$NIX" eval --json --expr 'x: x'

echo "=== 4. --raw ==="
serves "raw    string"   "$NIX" eval --raw --expr '"no quotes here"'
serves "raw    newline"  "$NIX" eval --raw --expr '"a\nb"'
serves "raw    selected" "$NIX" eval --raw --expr '{ a = "picked"; }' a
# coerceMore = false, so these fail in cppnix too. Only the exit code is
# compared; the wording differs and is not part of the contract.
same "raw    integer"  "$NIX" eval --raw --expr '1'
same "raw    list"     "$NIX" eval --raw --expr '[ 1 ]'

echo "=== 5. --file ==="
mkdir -p "$W/f/sub"
printf '{ a = { b = 1; }; imported = import ./sub/x.nix; }\n' > "$W/f/default.nix"
printf '"from a relative import"\n' > "$W/f/sub/x.nix"
serves "file   whole"      "$NIX" eval -f "$W/f/default.nix"
serves "file   selection"  "$NIX" eval -f "$W/f/default.nix" a.b
serves "file   relative import resolves against the file" \
     "$NIX" eval --raw -f "$W/f/default.nix" imported
serves "file   a directory means its default.nix" "$NIX" eval -f "$W/f" a.b
# A path interpolated into a string is the store path cppnix copies it to
# (ENG-12447). The handle path installs the same store-copy hook the one-call
# path does; when it did not, this printed the source path here and the store
# path through nix-instantiate.
# shellcheck disable=SC2016  # ${...} here is Nix interpolation, not shell
printf '"${./sub/x.nix}"\n' > "$W/f/interp.nix"
serves "file   an interpolated path is its store path" "$NIX" eval --raw -f "$W/f/interp.nix"
serves "ni     the same through nix-instantiate" "$NIXI" --eval --strict "$W/f/interp.nix"

echo "=== 6. failures keep their class ==="
same "throw"        "$NIX" eval --expr 'throw "boom"'
same "assert"       "$NIX" eval --expr 'assert false; 1'
same "infinite rec" "$NIX" eval --expr 'let a = a; in a'
same "parse error"  "$NIX" eval --expr '1 +'
same "undefined variable" "$NIX" eval --expr 'nosuchthing'

echo "=== 7. laziness: selecting must not force a sibling ==="
# The property the handle table exists for, and the one the lang corpus
# cannot see, because no corpus case selects an attribute. If selection
# forced siblings the throw would fire and the exit code would differ.
lazy_ok=1
for case in \
  '{ ok = 1; boom = throw "a sibling was forced"; }|ok|1' \
  '{ a = { keep = "yes"; boom = throw "forced"; }; b = throw "outer forced"; }|a.keep|"yes"' \
  '{ xs = [ 1 (throw "list sibling forced") ]; }|xs.0|1'; do
  IFS='|' read -r e path want <<< "$case"
  got=$(NIX_CONFIG="$RUST" "$NIX" eval --expr "$e" "$path" 2>"$W/lazy.err")
  rc=$?
  if [ "$got" != "$want" ] || [ $rc -ne 0 ]; then
    echo "  FAILED laziness: $path of $e gave rc=$rc '$got', wanted '$want'"
    echo "    stderr: $(head -c 300 "$W/lazy.err")"
    lazy_ok=0
  else
    echo "  lazy ok: $path => $got, siblings untouched"
  fi
  # And the same selection under cpp, so this is a property of the language
  # rather than of one backend.
  same "laziness under both: $path" "$NIX" eval --expr "$e" "$path"
done

echo "=== 8. what is still refused, by name ==="
# Each of these must fail with a message naming the feature. A refusal that
# says nothing is worse than a mismatch: the user learns only that it broke.
refusals_ok=1
check_refusal() {
  local want=$1; shift
  local err
  err=$(NIX_CONFIG="$RUST" "$@" 2>&1 >/dev/null)
  if grep -q "rust-eval unimplemented: .*$want" <<< "$err"; then
    echo "  refuses by name: $want"
  else
    echo "  FAILED: expected a refusal naming '$want', got: $(head -c 300 <<< "$err")"
    refusals_ok=0
  fi
}
# -- one "error: " per error, from a hook -------------------------------------
#
# Every embedder hook in `rust-eval-session.cc` reports a caught `nix::Error`
# with `e.message()`, deliberately: `e.what()` renders the whole `ErrorInfo`
# including the "error: " prefix, and the evaluator adds its own, so a `what()`
# here reads `error: error: ...`.
#
# That is a convention with nothing holding it, which is how one hook came to
# be written against it (ENG-13022). The convention is invisible to a grep --
# every hook's `catch (std::exception &)` clause sits directly under its
# `catch (Error &)` clause and correctly uses `what()`, so a search for
# `e.what()` returns a hit per hook whether or not any of them is wrong. This
# is the check that is not fooled by that.
#
# `MissingExperimentalFeature` out of the flake-locking hook, because it is the
# one exception reaching these hooks whose rendering carries a prefix; an
# ordinary `Error` would pass this check even through `what()` and prove
# nothing.
prefix_probe=$(mktemp -d)
cat > "$prefix_probe/flake.nix" <<'PROBEEOF'
{ outputs = { self }: { marker = "probe"; }; }
PROBEEOF
# `$RUST` and not a hand-rolled config: the first version of this probe built
# its own and so inherited the ambient `lint-url-literals = fatal`, which made
# the rust arm refuse at the PARSER LINT before reaching any hook. That refusal
# carries exactly one "error: ", so the check counted 1 and reported ok --
# vacuously passing on the strength of the very bug ENG-12996 fixes. The
# absolute `experimental-features` line comes last so it wins over the
# `extra-` line in `$RUST`, which is what takes `flakes` away.
prefix_out=$(NIX_CONFIG="$RUST
experimental-features = rust-eval nix-command" "$NIX" eval --raw --impure \
  --expr "(builtins.getFlake \"path:$prefix_probe\").marker" 2>&1 |
  LC_ALL=C sed -e 's/\x1b\[[0-9;]*m//g')
rm -rf "$prefix_probe"
prefix_count=$(printf '%s' "$prefix_out" | LC_ALL=C grep -o 'error: ' | wc -l | tr -d ' ')
if [ "$prefix_count" = 1 ]; then
  echo "  ok       a hook error carries exactly one 'error: ' prefix"
elif [ "$prefix_count" = 0 ]; then
  # Zero is not a pass. It means the probe did not fail at all -- most likely
  # `flakes` is enabled somewhere this run cannot see -- so the check measured
  # nothing rather than measuring a clean message.
  echo "  FAILED: the prefix probe produced no error, so it checked nothing. Got: $(printf '%s' "$prefix_out" | head -c 200)"
  refusals_ok=0
else
  echo "  FAILED: a hook error carries $prefix_count 'error: ' prefixes, want 1."
  echo "          A hook is reporting with e.what() rather than e.message() (ENG-13022):"
  printf '          %s\n' "$(printf '%s' "$prefix_out" | head -c 300)"
  refusals_ok=0
fi

check_refusal "nix eval --apply"     "$NIX" eval --expr '1' --apply 'x: x'
check_refusal "nix eval --write-to"  "$NIX" eval --expr '"x"' --write-to "$W/out"
# A flake reference is served now, so what is left of this row is the other
# half of what it used to cover: a store path names something already built
# and there is nothing to evaluate. The served half moved to
# `drv-parity.sh`'s build arm, which has a flake fixture with no inputs and
# compares the `.drv` bytes, the drvPath and a real outPath -- a stronger
# assertion than "it did not refuse", and one that needs no network.
# `flake-inputs-parity.sh` is the same assertion for a flake that HAS inputs,
# where the interesting halves of `call-flake.nix` live.
check_refusal "store-path installable" "$NIX" eval /nix/store/00000000000000000000000000000000-nope
check_refusal "stdin"                "$NIX" eval -f - a
check_refusal "arg"                  "$NIX" eval --expr '{ a ? 1 }: a' --arg a 2
check_refusal "output selection"     "$NIX" eval --expr '{ a = 1; }' 'a^out'
check_refusal "only a plain path"    "$NIX" eval -f '<nixpkgs>' lib.version
check_refusal "auto-calling"         "$NIX" eval --expr '{ f = { a ? 1 }: { b = a; }; }' f.b
# Only the with-locations spelling refuses now: plain `--xml` implies source
# locations and the rust document has none (ENG-12137), but
# `--xml --no-location` is served and gated by `lang-diff.sh`'s `.exp.xml`
# cases. No `--no-location` here, so this asserts the refusing half.
check_refusal "xml"                  "$NIXI" --eval --strict --xml -E '1'
# `nix eval` prints «error: ...» for a value that fails inside a structure and
# carries on; this printer stops. Named rather than allowed to look like an
# ordinary evaluation failure, which is what it is not: cppnix succeeds here.
check_refusal "below the top level"  "$NIX" eval --expr '{ a = throw "x"; }'
check_refusal "below the top level"  "$NIX" eval --expr '[ (throw "y") ]'
# cppnix prints «derivation <store path>» for these, which needs a store.
check_refusal "derivation-shaped"    "$NIX" eval --expr '{ type = "derivation"; }'
check_refusal "derivation-shaped"    "$NIX" eval --expr '{ outer = { type = "derivation"; }; }'
# cppnix copies a bare path to the store for --raw. This refuses instead, and
# the thing that must never happen is printing the source path, which a caller
# cannot tell from a real store path.
check_refusal "path"                 "$NIX" eval --raw --expr '/tmp/some/source/path'
if NIX_CONFIG="$RUST" "$NIX" eval --raw --expr '/tmp/some/source/path' 2>/dev/null \
     | grep -q '^/tmp/some/source/path$'; then
  echo "  FAILED: --raw printed the source path where cppnix prints a store path"
  refusals_ok=0
fi

# A flake's OUTPUTS evaluate on the VM, and the locking exemption does not
# leak into them.
#
# `EvalState::LockingFlake` lets the C++ evaluator read `flake.nix`'s `inputs`
# to produce a lock file, which is bridge machinery like the fetchers. What
# must never follow is the C++ evaluator answering for the flake's outputs:
# that would be the silent fallback the whole choke point exists to prevent,
# and it would be invisible, because a cpp-evaluated output looks exactly like
# a served one.
#
# So: a flake whose outputs use a construct this backend does not implement
# must REFUSE BY NAME. If the exemption leaked, cpp would evaluate the output
# and the command would answer instead. The counter cannot see this -- it
# reports proportions, not which values came from where -- so this is the
# assertion for the boundary's shape.
#
# `builtins.fetchMercurial` is the construct: implemented in cppnix, refused
# here, and it names no path -- the first attempt used `filterSource ./.`,
# whose relative path inside a flake made the rust arm fail with "path '/nix'
# does not exist", which is a refusal about the wrong thing. If it ever lands,
# this case starts answering and the gate says so, which is the moment to pick
# another one.
#
# That "path '/nix' does not exist" was not a quirk of this fixture. It was
# ENG-13123, and it cost 62.5% of ix's flake eval surface for as long as
# nobody wrote it down as a case. Section 8c below is that case.
flakeout=$(mkdir -p "$W/flake-out" && cd "$W/flake-out" && pwd -P)
flakesys=$(NIX_CONFIG="$CPP" "$NIX" eval --raw --impure --expr builtins.currentSystem 2>&1) || flakesys=unknown
cat > "$flakeout/flake.nix" <<FLAKEEOF
{
  outputs = { self }: {
    packages."$flakesys".probe = builtins.fetchMercurial "https://example.invalid/repo";
  };
}
FLAKEEOF
check_refusal "fetchMercurial" "$NIX" eval --raw "path:$flakeout#probe"
# And the same flake's LOCK still works, so the case above is a refusal about
# the output rather than a flake that never resolved. Without this the check
# passes for a flake that failed to lock, which is the vacuous shape.
if ! NIX_CONFIG="$RUST" "$NIX" eval --raw "path:$flakeout#probe" 2>&1 \
     | grep -q 'rust-eval unimplemented'; then
  echo "  FAILED: the flake-output refusal did not name a rust-eval gap"
  refusals_ok=0
fi
# The control: the cpp arm must reach the *output* too, so the refusal above
# is about evaluating an output and not about a flake that never resolved.
# "Reaches it" and not "produces a value": the fixture fetches an invalid URL,
# so cpp fails as well -- it just fails inside the fetcher rather than by
# refusing the construct. A run where cpp reported a rust-eval refusal, or a
# flake-level error, would mean the two arms never got as far as the output.
cppout=$(NIX_CONFIG="$CPP" "$NIX" eval --raw "path:$flakeout#probe" 2>&1)
if grep -q 'rust-eval unimplemented' <<< "$cppout"; then
  echo "  FAILED: the cpp arm refused too, so the fixture never reached an output: $(head -c 200 <<< "$cppout")"
  refusals_ok=0
elif ! grep -qi 'mercurial\|hg\|fetch' <<< "$cppout"; then
  echo "  FAILED: the cpp arm did not reach the output's fetch, so the refusal above says nothing about outputs: $(head -c 200 <<< "$cppout")"
  refusals_ok=0
else
  echo "  outputs evaluate on the VM: cpp reaches the output's fetcher, rust refuses the construct by name"
fi

echo "=== 8b. the purity settings are honoured per question, and named ==="
# Two claims, and both need checking because they fail in opposite directions.
#
# The forbidden half: a read outside the allow list must be stopped, and must
# be stopped the way cppnix stops it. Since ENG-12792 the read goes through
# `host.state.rootPath(...)`, i.e. this process's `rootFS` with its
# `AllowListSourceAccessor` (eval.cc:306), so the check is no longer "did the
# backend refuse by name" but "did the two arms produce the same refusal" --
# which is a stronger claim, and the point of the change. It is compared
# against the cpp arm rather than against a string written here, for the
# reason at the top of this file: a golden string asserts what somebody wrote
# down, and cppnix's mode wording ("in pure evaluation mode" against "in
# restricted mode") is exactly the kind of thing that moves upstream.
#
# Before ENG-12792 this arm expected the crate's own refusal, because those
# five questions landed on a std::fs Host that consults no allow list and had
# to refuse rather than answer outside it. That is still what a standalone
# embedding does; it is not what the nix binary does.
#
# The service half: pure-eval does NOT forbid the question channel, and
# treating it as if it did is the bug this section now also guards. cppnix
# answers getEnv with "" under either setting (primops.cc:1261) and nixPath
# with the lookup path it built under them, and refusing those made no flake
# evaluable on this backend. `rust/nix-eval-rs/src/purity.rs` is the table.
#
# The third half is new with ENG-12792 and is the one that cannot be satisfied
# by an absence: a read the allow list PERMITS has to come back with the
# bytes. "The read was stopped" is true of a backend that stopped everything,
# including a broken one, so it is not on its own evidence of anything.
#
# nix-instantiate rather than nix, because `nix` turns pure-eval on itself in
# main.cc, so `restrict-eval = true` alone cannot be observed through it: the
# detail correctly names both and the per-setting wording goes unchecked.
purity_probe() { # purity_probe CONFIG EXPR [BACKEND]
  NIX_CONFIG="extra-experimental-features = rust-eval
$(arm_base_config)
eval-backend = ${3:-rust}
$1" "$NIXI" --eval --strict -E "$2" 2>&1 | grep -v '^<4>'
}

# The one line of a refusal that says what happened, with cppnix's trace block
# and the search-path warning dropped. Both arms go through this, so a
# difference in framing is not scored as a difference in behaviour -- the cpp
# arm prints a "while calling the builtin" trace and the rust arm does not
# (ENG-12137), which is tier 2 under CLAUDE.md's parity bar.
refusal_line() {
  grep -oE "access to absolute path '[^']*' is forbidden[^\"]*" | head -1
}
# One record per line, so the both-settings case joins its two config lines
# with a semicolon and `tr` splits them again. A literal newline inside the
# record is what `read` treats as the end of it, which silently turned three
# cases into four -- two of them nonsense that then failed and were read as
# the refusal being broken.
while IFS='|' read -r joined named; do
  setting=$(tr ';' '\n' <<< "$joined")
  rust_raw=$(purity_probe "$setting" "builtins.readFile /etc/hostname" rust)
  cpp_raw=$(purity_probe "$setting" "builtins.readFile /etc/hostname" cpp)
  rust_err=$(refusal_line <<< "$rust_raw")
  cpp_err=$(refusal_line <<< "$cpp_raw")
  # An arm that failed some OTHER way is not an arm that allowed the read, and
  # saying so is the difference between one look and an hour: running this
  # against a pre-ENG-12792 binary reports "the read was ALLOWED" if the
  # fallback is a fixed string, when what happened is that the backend refused
  # in its own words. So the fallback is whatever it actually said.
  said() { # said LINE RAW
    if [ -n "$1" ]; then printf '%s\n' "$1"; else
      printf 'no cppnix refusal; it said: %s\n' "$(grep -vE '^$|does not exist, ignoring' <<< "$2" | head -1 | head -c 160)"
    fi
  }
  if [ -z "$cpp_err" ]; then
    # cppnix answering here would mean the expression is not forbidden at all,
    # so the whole comparison is vacuous rather than passing.
    echo "  REFUSING: [$joined] does not stop a read on the cpp arm either, so this case tests nothing"
    refusals_ok=0
  elif [ "$rust_err" = "$cpp_err" ]; then
    echo "  both arms refuse a direct read under [$joined]: $cpp_err"
  else
    echo "  FAILED: [$joined] the two arms disagree about a forbidden read"
    echo "      cpp:  $(said "$cpp_err" "$cpp_raw")"
    echo "      rust: $(said "$rust_err" "$rust_raw")"
    refusals_ok=0
  fi
  # The mode wording is cppnix's and it is per setting, so a run that refused
  # for the wrong reason is still a failure.
  case "$named" in
    *pure*) want="in pure evaluation mode" ;;
    *) want="in restricted mode" ;;
  esac
  grep -q "$want" <<< "$rust_err" || {
    echo "  FAILED: [$joined] refused, but not as $named ($want): $(said "$rust_err" "$rust_raw")"
    refusals_ok=0
  }
done <<'PURITY'
pure-eval = true|pure-eval
restrict-eval = true|restrict-eval
pure-eval = true;restrict-eval = true|pure-eval and restrict-eval
PURITY

# The service half. Each of these was a refusal before the split and is an
# answer now, and each is what cppnix answers -- measured on
# nix 2.34.7+ix.h24085346 under --pure-eval.
while IFS='|' read -r expr want; do
  got=$(purity_probe "pure-eval = true" "$expr" | head -1)
  if [ "$got" = "$want" ]; then
    echo "  pure-eval serves \`$expr\`"
  else
    echo "  FAILED: pure-eval should serve \`$expr\` as $want, got: $(head -c 200 <<< "$got")"
    refusals_ok=0
  fi
done <<'SERVED'
1 + 41|42
builtins.getEnv "HOME"|""
builtins.nixPath|[ ]
SERVED

# ENG-12792's own claim, and the arm that an all-refusing backend cannot pass:
# a file the allow list PERMITS is read, imported and listed under pure-eval.
# This is the shape a flake entry needs -- fetch something pinned, then read
# files out of the store path it produced -- because `fetch` calls `allowPath`
# on what it fetched.
#
# Compared against the cpp arm, so it asserts agreement rather than a string.
mkdir -p "$W/pure/tree/sub"
printf '{ greeting = "served"; sub = import ./sub/x.nix; }\n' > "$W/pure/tree/default.nix"
printf '7\n' > "$W/pure/tree/sub/x.nix"
tar -C "$W/pure" -czf "$W/pure/tree.tar.gz" --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner tree
# fetchTarball pins the NAR hash of the unpacked tree, not the tarball, and it
# is computed here rather than written down so the fixture can change.
tree_sha=$("$BUILD/src/nix/nix" hash path --type sha256 --sri "$W/pure/tree" 2>/dev/null)
if [ -z "$tree_sha" ]; then
  echo "  REFUSING: could not hash the pure-eval fixture, so the served arm would test nothing"
  refusals_ok=0
else
  fetched="builtins.fetchTarball { url = \"file://$W/pure/tree.tar.gz\"; sha256 = \"$tree_sha\"; }"
  while IFS='|' read -r what expr; do
    rust_got=$(purity_probe "pure-eval = true" "$expr" rust | tail -1)
    cpp_got=$(purity_probe "pure-eval = true" "$expr" cpp | tail -1)
    if [ "$rust_got" = "$cpp_got" ] && ! grep -q 'error\|unimplemented' <<< "$rust_got"; then
      echo "  pure-eval serves $what out of a fetched store path: $rust_got"
    else
      echo "  FAILED: pure-eval does not serve $what out of a fetched store path"
      echo "      cpp:  $(head -c 200 <<< "$cpp_got")"
      echo "      rust: $(head -c 200 <<< "$rust_got")"
      refusals_ok=0
    fi
  done <<SERVED_FROM_STORE
a readDir|builtins.readDir ($fetched)
an import|(import ($fetched)).greeting
a nested import|(import ($fetched)).sub
a readFile|builtins.readFile ($fetched + "/default.nix")
a pathExists|builtins.pathExists ($fetched + "/default.nix")
a readFileType|builtins.readFileType ($fetched + "/default.nix")
SERVED_FROM_STORE
fi

# An unpinned fetch under pure eval is cppnix's error, not a backend refusal,
# and the difference is what a refusal census counts. cppnix's own wording,
# from fetchTree.cc:537.
err=$(purity_probe "pure-eval = true" 'builtins.fetchurl "https://example.invalid/x"')
if grep -q "in pure evaluation mode, 'fetchurl' requires a 'sha256' argument" <<< "$err" \
   && ! grep -q "rust-eval unimplemented" <<< "$err"; then
  echo "  an unpinned fetch under pure-eval fails as cppnix does, not as a refusal"
else
  echo "  FAILED: unpinned fetch under pure-eval: $(head -c 300 <<< "$err")"
  refusals_ok=0
fi

echo "=== 8c. a filtered builtins.path in a flake output, which is pure eval ==="
# The regression case for ENG-13123, and the one shape no other gate here can
# express.
#
# `lib.fileset` -- which is how ix names a source tree, and therefore how most
# of its packages name theirs -- lowers to `builtins.path` with a `filter`.
# Inside a flake that runs under pure eval, where `EvalState::rootFS` is a
# mounted accessor holding `/` -> empty and `/nix/store` -> the store
# (`eval.cc:294`). The root is a store path, so its first ancestor is `/nix`,
# which matches no mount and reads as missing. The Rust arm's filter walk
# asked a question that threw on that and died with `path '/nix' does not
# exist` before the filter ran once: 90 of ix's 144 flake attributes, every
# one of them fine on cppnix, and no `rust-eval unimplemented:` marker
# anywhere, so `lang-diff.sh`'s bucket could not see it either.
#
# Why it has to be a flake. `lang-diff.sh`'s corpus is the natural home for a
# filtered `builtins.path` and it already has one (`eval-okay-path.nix`), but
# it cannot have this one: passing `--pure-eval` to a corpus file makes the
# ORACLE arm refuse -- "access to absolute path '.../eval-okay-path.nix' is
# forbidden in pure evaluation mode" -- so the case never reaches an
# evaluator. A flake is the only shape that gets pure eval and a readable
# source at once, which is exactly why the bug lived where no gate looked.
#
# Both halves are here on purpose. The unfiltered copy passed throughout the
# bug's life, so without it beside the filtered one a run cannot say whether a
# failure is about filtering or about `builtins.path` at all.
#
# `serves` and not `same`: two arms that both fail to produce a store path
# agree, and agreeing about nothing is the vacuous pass this fixture exists to
# rule out.
filterflake=$(mkdir -p "$W/filter-flake/sub" && cd "$W/filter-flake" && pwd -P)
printf 'eng13123\n' > "$filterflake/sub/a.txt"
cat > "$filterflake/flake.nix" <<'FILTEREOF'
{
  outputs = { self }: {
    noFilter        = builtins.toString (builtins.path { path = ./sub; name = "s"; });
    withFilter      = builtins.toString (builtins.path { path = ./sub; name = "s"; filter = p: t: true; });
    viaFilterSource = builtins.toString (builtins.filterSource (p: t: true) ./sub);
  };
}
FILTEREOF
serves "flake builtins.path, no filter" "$NIX" eval --raw "path:$filterflake#noFilter"
serves "flake builtins.path, with filter (ENG-13123)" "$NIX" eval --raw "path:$filterflake#withFilter"
serves "flake builtins.filterSource (ENG-13123)" "$NIX" eval --raw "path:$filterflake#viaFilterSource"
# The filter must not change the store path: it accepts everything, so the
# copy is the same tree the unfiltered call copies. Comparing the two arms
# only would pass on two evaluators that agree about a WRONG path, which is
# the Tier 1 outcome nothing downstream can detect.
nof=$(NIX_CONFIG="$RUST" "$NIX" eval --raw "path:$filterflake#noFilter" 2>/dev/null)
wf=$(NIX_CONFIG="$RUST" "$NIX" eval --raw "path:$filterflake#withFilter" 2>/dev/null)
if [ -n "$nof" ] && [ "$nof" = "$wf" ]; then
  echo "  an accept-everything filter copies the same tree: $wf"
else
  echo "  FAILED: filtered and unfiltered copies disagree: unfiltered='$nof' filtered='$wf'"
  refusals_ok=0
fi

echo "=== 9. nix-instantiate keeps what it had, and gains -A and --json ==="
serves "ni whole"  "$NIXI" --eval --strict -E '{ a = 1; }'
serves "ni -A"     "$NIXI" --eval --strict -E '{ a = { b = 2; }; }' -A a.b
serves "ni --json" "$NIXI" --eval --strict --json -E '{ a = 1; }'
serves "ni --json -A" "$NIXI" --eval --strict --json -E '{ a = [ 1 2 ]; }' -A a

echo "=== 10. rollback is one setting, and the default is untouched ==="
# Folded in from rust-backend-selection.sh, which was deleted: that script had
# been inverted since rung H (its section 2 failed when `nix eval` started
# working, which it now does), pointed at a hardcoded build path, and read its
# exit code through a `head` pipeline. It exited non-zero at section 2 every
# run, so the rollback check below -- the only part of it that was still
# testing something real, and the property rung G's flip depends on -- had not
# executed in weeks.
rollback_ok=1
# The default path, with no eval-backend named at all.
for label in "nix-instantiate" "nix eval"; do
  case $label in
    "nix-instantiate") got=$("$NIXI" --eval --strict -E '1 + 41' 2>&1) ;;
    *) got=$(NIX_CONFIG="extra-experimental-features = nix-command" "$NIX" eval --expr '1 + 41' 2>&1) ;;
  esac
  if [ "$got" = "42" ]; then
    echo "  default: $label => 42"
  else
    echo "  FAILED: the default backend changed behaviour: $label => $got"
    rollback_ok=0
  fi
done
# And the rollback proper: the experimental feature stays granted, only the
# setting goes away. That is the one edit an operator makes to back out a
# flip, so it is tested by doing it rather than by reading the code.
got=$(NIX_CONFIG="extra-experimental-features = nix-command rust-eval" "$NIX" eval --expr '1 + 41' 2>&1)
if [ "$got" = "42" ]; then
  echo "  rollback: dropping 'eval-backend = rust' restores cpp => 42"
else
  echo "  FAILED: rollback did not restore the C++ path: $got"
  rollback_ok=0
fi
# ... and it really is cpp that answered, not rust agreeing by coincidence.
NIX_CONFIG="extra-experimental-features = nix-command rust-eval" \
  NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/stats-rollback.json" \
  "$NIX" eval --expr '1 + 41' > /dev/null 2>&1
ev=$(python3 -c 'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("evaluator", "<absent>"))
except OSError:
    print("<no stats file>")' "$W/stats-rollback.json")
if [ "$ev" = "cpp" ]; then
  echo "  rollback: NIX_SHOW_STATS confirms cpp served it"
else
  echo "  FAILED: after rollback the evaluator is '$ev', wanted cpp"
  rollback_ok=0
fi

echo
echo "RESULT rust-nix-eval-gate pairs=$pairs match=$match mismatch=$mismatch served=$served refused=$refused produced=$produced empty=$empty_agreement lazy_ok=$lazy_ok refusals_ok=$refusals_ok rollback_ok=$rollback_ok serves-refused=$serves_refused ratchets-from=$GATE_RATCHETS_MEASURED_AT@$GATE_RATCHETS_MEASURED_ON"
if [ ${#failures[@]} -gt 0 ]; then
  echo "mismatched: ${failures[*]}"
fi
# `served` is the denominator that means something: pairs that agreed AND
# produced a value. A run where everything refused would otherwise report
# mismatch=0 and read as a pass.
# `empty` is the number of times the two arms agreed by both printing nothing.
# That is not agreement about a value, and the run that found this shape scored
# four --file cases as matches while both sides were refusing the invocation.
#
# Exact, from gate-ratchets.sh, not floors. These were `served > 30` and
# `produced > 20` against an observed 51 and 27, which is 40% of slack: the
# rung-H work could have stopped serving a third of the cases and the gate
# would still have said PASS. Every case here is a literal in this file, so
# the counts are deterministic and any movement belongs in a diff.
exact_ok=1
check_exact() { # NAME GOT WANT
  [ "$2" -eq "$3" ] && return 0
  echo "  RATCHET: $1=$2, gate-ratchets.sh says $3. If this is intended, change the number there in the same commit; do not widen the comparison."
  exact_ok=0
}
check_exact pairs    "$pairs"    "$RUST_NIX_EVAL_PAIRS"
check_exact served   "$served"   "$RUST_NIX_EVAL_SERVED"
check_exact produced "$produced" "$RUST_NIX_EVAL_PRODUCED"
check_exact refused  "$refused"  "$RUST_NIX_EVAL_REFUSED"

[ "$mismatch" -eq 0 ] && [ "$lazy_ok" -eq 1 ] && [ "$refusals_ok" -eq 1 ] \
  && [ "$rollback_ok" -eq 1 ] && [ "$serves_refused" -eq 0 ] \
  && [ "$empty_agreement" -eq 0 ] && [ "$exact_ok" -eq 1 ] \
  && { echo "PASS"; exit 0; }
echo "FAIL"; exit 1
