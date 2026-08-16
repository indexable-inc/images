#!/usr/bin/env bash
# Cross-backend parity for builtins.derivationStrict: every expression below
# is evaluated twice through ONE binary, once with eval-backend=cpp and once
# with rust, and the two are compared byte for byte on stdout and exit code.
#
# Differential and not golden, for the reason rust-nix-eval-gate.sh gives: a
# golden file asserts what somebody wrote down, this asserts what the other
# evaluator does.
#
# This gate is tier 1 and stays there (CLAUDE.md, "Parity bar"). Every case
# below computes a `.drv`, an outPath or a drvPath, and for those byte
# identity IS functional identity: a different outPath is a different store
# path and nothing substitutes. So a pair that PRODUCES a value is compared
# byte for byte, there is no allowlist, and this script deliberately does not
# read eval-allowlist.toml -- which can only ever waive tier 2.
#
# The one place a class token appears is the both-arms-failed bucket, and it
# is not a relaxation: neither arm produced a value there, so there is no hash
# to compare and nothing tier 1 has an opinion about. What is being asked is
# only "did they fail the same way", which is a tier-2 question with a tier-2
# answer.
#
# Outcomes are classified, because "not equal" hides two very different
# things. A rust arm that refuses by name is `unimplemented` -- a named gap,
# never a wrong answer. A rust arm that succeeds with different bytes, or
# fails where cpp succeeded for any other reason, is `mismatch`, which is the
# only outcome that may not appear.
#
# There is a second arm at the bottom, and it is a different kind of check.
# Everything above it evaluates; `nix build` also *realises*, which needs the
# `.drv` to be a real store object rather than a path the evaluator computed.
# That distinction is not academic: on 2026-08-06, on dev-compute-6, the Rust
# arm printed the same drvPath as cpp for a two-line derivation and left
# nothing at it, while the cpp arm wrote 272 bytes there (ENG-12799). The
# eval arms above cannot see that: they only ever compare printed paths.
#
# So the build arm needs a WRITABLE STORE and a working builder, which the
# eval arms do not. That is a real added requirement on whoever runs this.
#
# Needs a built nix with the Rust evaluator linked in, which the default build
# is not (`rust-eval` defaults to `disabled`):
#
#   nix develop --command nix shell nixpkgs#cargo nixpkgs#rustc --command bash -c \
#     'meson setup build-rust --prefix="$out" -Dnix:rust-eval=enabled && ninja -C build-rust'
#
# Point it at one with NIX_BUILD_DIR; the default is ./build-rust.
#
# Exits non-zero unless all four of these hold:
#
#   mismatch == 0
#   cases    == DRV_PARITY_CASES     (gate-ratchets.sh)
#   match    >= DRV_PARITY_MIN_MATCH (gate-ratchets.sh)
#   every both-arms-failed pair failed in the SAME error class
#
# Only the first was checked once, and the other three are what stopped this
# gate from being satisfied by measuring nothing. A backend that refused every
# case scored `unimplemented=60 mismatch=0` and exited 0; so did a run whose
# case list had silently shrunk; and 17 of the 60 pairs were landing in
# `both-fail-differently`, which was printed and then ignored, so cppnix
# rejecting an attribute name and the Rust VM aborting for an unrelated reason
# counted the same as agreement.
set -u
BUILD=${NIX_BUILD_DIR:-$PWD/build-rust}

# Which arms to run: `eval`, `build`, or `both` (the default).
#
# Not a convenience. `rust-eval-cache-cli.sh` runs this whole script twice,
# once with `eval-cache-dir` set, and diffs the two outputs byte for byte
# after normalising only the scratch directory. The build arm's fixtures
# carry this process's PID in their derivation name, deliberately, so that
# their `.drv` cannot already be in the store -- which means the two runs
# print different names and different store hashes, and that gate would
# fail on a difference that says nothing about caching. Normalising the
# hashes away instead would blind it to the store paths it exists to
# compare. So the cache gate asks for the eval arm, which is the one that
# carries the stderr warnings it is really after.
DRV_PARITY_ARMS=${DRV_PARITY_ARMS:-both}
case $DRV_PARITY_ARMS in
  eval|build|both) ;;
  *) echo "drv-parity: DRV_PARITY_ARMS must be eval, build or both (got '$DRV_PARITY_ARMS')"; exit 2 ;;
esac

NIXI=$BUILD/src/nix/nix-instantiate
[ -x "$NIXI" ] || { echo "no nix-instantiate at $NIXI"; exit 2; }

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=./arm-config.sh
. "$here/arm-config.sh" || exit 2
# Before anything reads the environment: one owner of the gates' nix
# configuration, so an ambient `lint-url-literals = fatal` cannot make every
# rust arm refuse and every row score `unimplemented` (ENG-12996).
arm_pin_environment
# shellcheck source=./gate-ratchets.sh
. "$here/gate-ratchets.sh" || exit 2
# shellcheck source=./error-class.sh
. "$here/error-class.sh" || exit 2

# EXTRA_NIX_CONFIG is appended to both arms, so a caller can run this whole
# comparison under an additional setting -- `eval-cache-dir`, in
# rust-eval-cache-cli.sh's hands -- and require the verdict to come out the
# same. Empty by default, so an ordinary run is unchanged.
# `nix-command` and `flakes` are named rather than assumed off the machine's
# nix.conf: the build arm below runs `nix eval` and `nix build`, and one case
# resolves a flake reference. Both arms get the same set out of the same
# binary, so widening what is enabled cannot move the comparison -- it only
# stops the gate depending on how the host happens to be configured.
# `ca-derivations` is here for the four content-addressed cases below; it
# gates behavior only where a derivation asks for it, so the input-addressed
# cases cannot see it.
BASE="extra-experimental-features = rust-eval nix-command flakes ca-derivations
$(arm_base_config)${EXTRA_NIX_CONFIG:+
$EXTRA_NIX_CONFIG}"
CPP="$BASE
eval-backend = cpp"
RUST="$BASE
eval-backend = rust"

W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
mkdir -p "$W/src"; echo "hello from a source file" > "$W/src/f.txt"

# Capability probe. This gate was the only one without one, and it is the one
# that needs it most: `nix config show` reports `eval-backend = rust` on a
# binary compiled without the Rust evaluator (-Dnix:rust-eval=disabled is the
# default), and every case below would then refuse, land in `unimplemented`,
# and exit 0. A setting is not a capability; evaluate something.
for arm in CPP RUST; do
  case $arm in CPP) cfg=$CPP ;; *) cfg=$RUST ;; esac
  got=$(NIX_CONFIG="$cfg" "$NIXI" --eval --strict -E 1 2>&1)
  [ "$got" = 1 ] || {
    echo "drv-parity: the $arm arm cannot evaluate the probe expression '1'; nothing below would mean anything:"
    echo "$got"
    exit 2
  }
  # And the stronger form: which backend actually ran. Derived from a count of
  # evaluations served, not echoed back from the setting (ENG-12542).
  case $arm in CPP) want=cpp ;; *) want=rust ;; esac
  NIX_CONFIG="$cfg" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/stats-$arm.json" \
    "$NIXI" --eval --strict -E 1 > /dev/null 2>&1
  ev=$(python3 -c 'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("evaluator", "<absent>"))
except OSError:
    print("<no stats file>")' "$W/stats-$arm.json")
  [ "$ev" = "$want" ] || {
    echo "drv-parity: the $arm arm asked for the '$want' evaluator, NIX_SHOW_STATS reports '$ev'; the two arms would be the same backend and every comparison below would be vacuous"
    exit 2
  }
  echo "probe: $arm arm evaluates, NIX_SHOW_STATS confirms the '$ev' backend ran"
done

D="name = \"g\"; system = \"x86_64-linux\"; builder = \"/bin/sh\";"

declare -a CASES=(
  "builtins.derivationStrict { $D }"
  "builtins.derivationStrict { $D outputs = [ \"dev\" \"lib\" \"out\" ]; }"
  "builtins.derivationStrict { $D outputs = [ \"bin\" ]; }"
  "let a = builtins.derivationStrict { name = \"a\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; }; in builtins.derivationStrict { $D dep = a.out; }"
  "let a = builtins.derivationStrict { name = \"a\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; }; in let b = builtins.derivationStrict { name = \"b\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; dep = a.out; }; in builtins.derivationStrict { $D dep = b.out; args = [ \"-x\" a.out ]; }"
  "let a = builtins.derivationStrict { name = \"a\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; outputs = [ \"dev\" \"out\" ]; }; in builtins.derivationStrict { $D d1 = a.dev; d2 = a.out; }"
  "builtins.derivationStrict { $D args = [ \"-c\" \"echo hi\" ]; }"
  "builtins.derivationStrict { $D n = 42; f = 1.5; t = true; e = false; l = [ \"a\" \"b\" ]; }"
  # A list holding an empty list: cppnix drops the separator after it, so this
  # attribute's coerced bytes, and therefore the outPath, differ if the join
  # rule is wrong (ENG-12527).
  "builtins.derivationStrict { $D l = [ \"a\" [ ] \"b\" ]; }"
  "builtins.derivationStrict { $D l = [ [ ] \"b\" ]; }"
  "builtins.derivationStrict { $D __ignoreNulls = true; skipped = null; kept = \"x\"; }"
  "builtins.derivationStrict { $D kept = null; }"
  "builtins.derivationStrict { $D out = \"overwritten\"; }"
  "let a = builtins.derivationStrict { name = \"a\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; }; in builtins.derivationStrict { $D out = a.out; }"
  "builtins.derivationStrict { $D src = $W/src; }"
  "builtins.derivationStrict { $D src = $W/src/f.txt; }"
  "builtins.derivationStrict { $D src = $W/src/f.txt; args = [ $W/src/f.txt ]; }"
  "builtins.getContext (builtins.derivationStrict { $D }).out"
  "builtins.getContext (builtins.derivationStrict { $D }).drvPath"
  "builtins.attrNames (builtins.derivationStrict { $D outputs = [ \"a\" \"b\" \"c\" ]; })"
  "let a = builtins.derivationStrict { name = \"a\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; }; in builtins.getContext (builtins.derivationStrict { $D dep = a.out; }).out"
  "(builtins.derivationStrict { $D }) == (builtins.derivationStrict { $D })"
  "builtins.derivationStrict { $D name2 = \"\"; }"
  "builtins.derivationStrict { name = \"a-b.c_d+e?f=g\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; }"
  "builtins.derivationStrict { }"
  "builtins.derivationStrict { name = \"x\"; }"
  "builtins.derivationStrict { name = \"x\"; builder = \"/bin/sh\"; }"
  "builtins.derivationStrict { name = \"x.drv\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; }"
  "builtins.derivationStrict { name = \"a/b\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; }"
  # Content-addressed (ENG-13140): the .drv path and every output value
  # (downstream placeholders, not paths) are compared byte for byte. The
  # fourth case is a derivation DOWNSTREAM of a CA one, whose own outputs are
  # Deferred and whose output values are placeholders too.
  "builtins.derivationStrict { $D __contentAddressed = true; }"
  "builtins.derivationStrict { $D __contentAddressed = true; outputs = [ \"dev\" \"out\" ]; }"
  "builtins.derivationStrict { $D __contentAddressed = true; outputHashMode = \"flat\"; outputHashAlgo = \"sha1\"; }"
  "let a = builtins.derivationStrict { $D __contentAddressed = true; }; in builtins.derivationStrict { name = \"h\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; dep = a.out; }"
  "builtins.derivationStrict { $D outputs = [ \"drvPath\" ]; }"
  "builtins.derivationStrict { $D outputs = [ ]; }"
  "builtins.derivationStrict { $D outputs = [ \"out\" \"out\" ]; }"
  "builtins.derivationStrict { $D __structuredAttrs = true; }"
  # -- __structuredAttrs (ENG-12479 follow-on) -------------------------------
  # Every attribute becomes one member of a `__json` environment variable
  # instead of a variable of its own, so the ATerm differs and so does the
  # path. The scalar row is the one that pins the JSON rendering: an integer
  # written as a string, or a float rendered the printer's way rather than
  # nlohmann's, moves the hash.
  "builtins.derivationStrict { $D __structuredAttrs = true; n = 1234; l = [ \"a\" \"b\" ]; b = true; s = \"str\"; }"
  "builtins.derivationStrict { $D __structuredAttrs = true; nested = { a = { b = [ 1 2 ]; }; }; }"
  "builtins.derivationStrict { $D __structuredAttrs = true; outputs = [ \"dev\" \"out\" ]; }"
  "builtins.derivationStrict { $D __structuredAttrs = true; src = $W/src/f.txt; }"
  "let a = builtins.derivationStrict { name = \"a\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; }; in builtins.derivationStrict { $D __structuredAttrs = true; dep = a.out; }"
  "builtins.derivationStrict { $D __structuredAttrs = true; __ignoreNulls = true; skipped = null; kept = \"x\"; }"
  # The six attributes structuredAttrs silently disables. cppnix warns about
  # each, and the harness captures stderr, so a missing warning is a
  # difference here even though the path is right.
  "builtins.derivationStrict { $D __structuredAttrs = true; allowedReferences = [ ]; maxSize = 1234; }"
  "builtins.derivationStrict { $D allowedReferences = [ ]; maxSize = 1234; }"
  # A structured derivation whose output is named __json collides with the
  # variable the object is encoded in.
  "builtins.derivationStrict { $D __structuredAttrs = true; outputs = [ \"__json\" ]; }"
  "builtins.derivationStrict { $D __structuredAttrs = false; }"
  "builtins.derivationStrict { $D __contentAddressed = false; }"
  "builtins.derivationStrict { $D outputHash = \"0000000000000000000000000000000000000000000000000000\"; outputHashAlgo = \"sha256\"; outputHashMode = \"flat\"; }"
  "let a = builtins.derivationStrict { name = \"a\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; }; in builtins.derivationStrict { $D dep = a.drvPath; }"
  # -- string context introspection (ENG-12479) ------------------------------
  # The eta rule the corpus asserts, checked on the context and not on the
  # string: string equality ignores the context, so comparing the strings
  # passes for an appendContext that returns its argument untouched.
  "let a = builtins.derivationStrict { $D }; s = \"\${a.out}\${a.drvPath}\${$W/src/f.txt}\"; in builtins.getContext (builtins.appendContext (builtins.unsafeDiscardStringContext s) (builtins.getContext s))"
  "let a = builtins.derivationStrict { $D }; s = \"\${a.out}\${a.drvPath}\${$W/src/f.txt}\"; in s == builtins.appendContext (builtins.unsafeDiscardStringContext s) (builtins.getContext s)"
  # A key that is not a store path, and one that is well-formed but outside
  # the store directory: both are cppnix's isStorePath refusal, and the
  # second is why the store directory has to be handed over rather than
  # assumed.
  "builtins.appendContext \"x\" { \"/not/a/store/path\" = { path = true; }; }"
  "builtins.appendContext \"x\" { \"/tmp/x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-a\" = { path = true; }; }"
  # allOutputs and a non-empty outputs list on something that is not a
  # derivation; an empty list on the same key is allowed.
  "builtins.appendContext \"x\" { \"/nix/store/x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-a\" = { allOutputs = true; }; }"
  "builtins.appendContext \"x\" { \"/nix/store/x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-a\" = { outputs = [ \"out\" ]; }; }"
  "builtins.getContext (builtins.appendContext \"x\" { \"/nix/store/x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-a\" = { outputs = [ ]; }; })"
  # The output-dependency pair, and the three refusals cppnix has for
  # addDrvOutputDependencies: not a derivation, an output, and a context that
  # does not have exactly one element.
  "let a = builtins.derivationStrict { $D }; in builtins.getContext (builtins.unsafeDiscardOutputDependency a.drvPath)"
  "let a = builtins.derivationStrict { $D }; in builtins.getContext (builtins.addDrvOutputDependencies (builtins.unsafeDiscardOutputDependency a.drvPath))"
  "let a = builtins.derivationStrict { $D }; in builtins.getContext (builtins.addDrvOutputDependencies (builtins.addDrvOutputDependencies a.drvPath))"
  "builtins.addDrvOutputDependencies \"\${$W/src/f.txt}\""
  "let a = builtins.derivationStrict { $D }; in builtins.addDrvOutputDependencies a.out"
  "builtins.addDrvOutputDependencies \"plain\""
  "let a = builtins.derivationStrict { $D }; in builtins.addDrvOutputDependencies \"\${a.out}\${a.drvPath}\""
)

match=0; mismatch=0; unimpl=0; bothfail=0; n=0
if [ "$DRV_PARITY_ARMS" = build ]; then
  CASES=()
fi
for e in "${CASES[@]}"; do
  n=$((n+1))
  co=$(NIX_CONFIG="$CPP" "$NIXI" --eval --strict -E "$e" 2>&1); crc=$?
  ro=$(NIX_CONFIG="$RUST" "$NIXI" --eval --strict -E "$e" 2>&1); rrc=$?
  detail=
  if [ "$crc" = "$rrc" ] && [ "$co" = "$ro" ]; then
    match=$((match+1)); verdict=match
  elif printf "%s" "$ro" | grep -qiE "unimplemented|does not implement|rust-eval unimplemented"; then
    unimpl=$((unimpl+1)); verdict=unimplemented
  elif [ "$crc" != 0 ] && [ "$rrc" != 0 ]; then
    # Both arms failed. That is not agreement until the failures are the same
    # KIND of failure -- this bucket held 17 of 60 pairs while asserting
    # nothing, so a Rust arm that died for a completely unrelated reason
    # scored here beside cppnix's deliberate refusal. Same rule and the same
    # classifier lang-diff.sh's eval-fail arm uses.
    printf "%s" "$co" > "$W/co.err"; printf "%s" "$ro" > "$W/ro.err"
    class_c=$(error_class "$W/co.err"); class_r=$(error_class "$W/ro.err")
    detail="class cpp=$class_c rust=$class_r"
    if [ "$class_c" = "$class_r" ] && [ "$class_c" != unknown ]; then
      bothfail=$((bothfail+1)); verdict=both-fail-alike
    elif [ "$class_c" = unknown ] && [ "$class_r" = unknown ] \
         && [ -n "$(last_error "$W/co.err")" ] \
         && [ "$(last_error "$W/co.err")" = "$(last_error "$W/ro.err")" ]; then
      # Neither classifier pattern matched, but the terminal error lines are
      # byte-identical, so the two arms did say the same thing. Without this,
      # a novel-but-agreeing failure would read as a divergence.
      bothfail=$((bothfail+1)); verdict=both-fail-alike
    else
      mismatch=$((mismatch+1)); verdict=MISMATCH
    fi
  else
    mismatch=$((mismatch+1)); verdict=MISMATCH
  fi
  printf "%-24s %s\n" "$verdict" "$e"
  if [ "$verdict" != match ]; then
    [ -n "$detail" ] && printf "      %s\n" "$detail"
    printf "      cpp  rc=%s %s\n" "$crc" "$(printf "%s" "$co" | head -3 | tr "\n" "|")"
    printf "      rust rc=%s %s\n" "$rrc" "$(printf "%s" "$ro" | head -3 | tr "\n" "|")"
  fi
done
echo
echo "RESULT cases=$n match=$match mismatch=$mismatch unimplemented=$unimpl both-fail-alike=$bothfail \
expected-cases=$DRV_PARITY_CASES min-match=$DRV_PARITY_MIN_MATCH \
ratchets-from=$GATE_RATCHETS_MEASURED_AT@$GATE_RATCHETS_MEASURED_ON"
echo "binary=$NIXI sha256=$(sha256sum "$NIXI" | cut -d" " -f1) version=$("$NIXI" --version | head -1)"

ok=1
if [ "$DRV_PARITY_ARMS" = build ]; then
  echo "drv-parity: DRV_PARITY_ARMS=build, so the eval arm did not run and its counts above are zero by construction"
else
if [ "$mismatch" != 0 ]; then
  echo "drv-parity: $mismatch case(s) diverged"
  ok=0
fi
# The case list is checked in beside this script, so its length is a fact and
# not a measurement. An exact count is what catches an edit that drops cases
# on the way to a greener number.
if [ "$n" != "$DRV_PARITY_CASES" ]; then
  echo "drv-parity: ran $n cases, gate-ratchets.sh says $DRV_PARITY_CASES. Update DRV_PARITY_CASES in the same commit that changes the CASES array, so a list that shrank by accident cannot read as a pass."
  ok=0
fi
# And a floor under the pairs that actually agreed about a VALUE. Without it,
# refusing all 60 cases is mismatch=0 and a clean exit.
if [ "$match" -lt "$DRV_PARITY_MIN_MATCH" ]; then
  echo "drv-parity: match=$match is under the checked-in floor of $DRV_PARITY_MIN_MATCH; parity went backwards, or the rust arm stopped serving these cases."
  ok=0
fi
fi

# -- the nix build arm ------------------------------------------------------
#
# Same differential shape as above -- one binary, two backends, compared byte
# for byte -- asking the question the eval arms structurally cannot. Three
# things are compared per case, and all three have to agree:
#
#   1. the `.drv` EXISTS in the store after the rust arm ran, and its bytes
#      are what the cpp arm's `.drv` contains;
#   2. the drvPath printed by each arm;
#   3. the outPath printed by each arm, from a real build.
#
# The rust arm runs FIRST and the fixture's derivation name carries this
# run's PID, so its `.drv` cannot have been left behind by the cpp arm, by an
# earlier run of this script, or by anything else on the machine. Without
# that, "the file is there" is satisfied by a store that already had it,
# which is the loudest possible way for this arm to measure nothing. The
# fixture's absence beforehand is checked rather than assumed, by globbing
# the store for the name, and a fixture that is somehow already there fails
# the run instead of being built on top of.
#
# The `hello` case cannot have that property -- a machine that has ever built
# hello already has the `.drv` -- so its presence check is reported as
# `pre=present` and carries no weight. What it does carry is the byte
# comparison, which the fixture case cannot: hello's `.drv` is 3 KB of a real
# stdenv derivation rather than four lines.
if [ "$DRV_PARITY_ARMS" = eval ]; then
  echo
  echo "drv-parity: DRV_PARITY_ARMS=eval, so the nix build arm did not run"
  [ "$ok" = 1 ]
  exit
fi

echo
echo "-- nix build arm --------------------------------------------------"
NIXBIN=$BUILD/src/nix/nix
arm_require_clean_config "$NIXBIN"
if [ ! -x "$NIXBIN" ]; then
  echo "drv-parity: no nix at $NIXBIN, so the build arm cannot run"
  ok=0
fi

# A fresh name per run, so case 1 is a real assertion. `currentSystem` rather
# than a pinned one: this arm builds, so it has to target the machine it is
# on.
FRESH="drv-parity-fresh-$$"
STOREDIR=$(NIX_CONFIG="$CPP" "$NIXBIN" eval --raw --impure --expr builtins.storeDir 2>&1) || {
  echo "drv-parity: could not read builtins.storeDir, so the build arm cannot tell a fresh .drv from an old one: $STOREDIR"
  exit 2
}
cat > "$W/fresh.nix" <<NIXEOF
derivation {
  name = "$FRESH";
  system = builtins.currentSystem;
  builder = "/bin/sh";
  args = [ "-c" "echo drv-parity > \$out" ];
}
NIXEOF

# `hello` out of a real nixpkgs is the case that exercises stdenv, which is
# where the interesting refusals live. Skipped when NIXPKGS is unset -- and
# COUNTED as skipped, then checked against the ratchet, so a run that quietly
# lost it cannot read as a pass.
#
# Written as an applied import rather than `-f $NIXPKGS hello`, because the
# top level of nixpkgs is a *function* and `-f` on it makes cppnix auto-call
# the function with the formals' defaults. This backend refuses that by name
# (`auto-calling the function reached at 'hello'`, measured on
# dev-compute-6), and the refusal is the honest answer: nothing here applies
# a Nix function on the caller's behalf. Applying it in the expression is the
# same shape `nixpkgs-frontier.sh` uses for the same reason.
# A two-output derivation whose `meta.outputsToInstall` names one of them.
# This is the reduction `PackageInfo::queryOutputs(false, true)` applies and
# that nixpkgs relies on for every multi-output package: get it wrong and the
# build produces a different set of outputs from cpp's, with every path in it
# still correct, which is a divergence no single-output case can see.
#
# `meta` is attached with `//` rather than passed to `derivation`, because an
# attribute set passed in becomes an environment variable and does not
# coerce. That is also the shape nixpkgs ends up with: `mkDerivation` puts
# `meta` on the returned value, not in the `.drv`.
cat > "$W/outputs.nix" <<NIXEOF
let d = derivation {
      name = "$FRESH-multi";
      system = builtins.currentSystem;
      builder = "/bin/sh";
      outputs = [ "out" "dev" ];
      args = [ "-c" "echo out > \$out; echo dev > \$dev" ];
    };
in d // { meta.outputsToInstall = [ "dev" ]; }
NIXEOF

# A flake fixture with no inputs, so the flake pipeline is exercised without
# a network and without a registry. This is the case that says the *whole*
# entry point works: the installable is resolved and locked by cppnix, and
# `call-flake.nix` is then evaluated by whichever backend is selected, applied
# to the lock file, the overrides set and `fetchFinalTree`. Its derivation
# name carries the PID like the other fixtures, so its `.drv` cannot already
# be in the store and the presence check is a real assertion rather than a
# reading of what the machine happened to have.
# `pwd -P`, not `$W`: the path fetcher refuses a flake whose path traverses a
# symlink ("path '//var' is a symlink"), and on macOS `mktemp -d` hands back
# something under `/var/folders`, where `/var` is a link to `/private/var`.
# Both arms failed identically on it, which the loop below scores as a
# MISMATCH -- correctly, since neither produced an outPath -- so a fixture
# that cannot be fetched reads as a parity failure. Resolve it once, here.
FLAKEDIR=$(mkdir -p "$W/flake" && cd "$W/flake" && pwd -P)
# The system spelled out rather than `builtins.currentSystem`, which a flake
# cannot use: flakes evaluate under `pure-eval`, where cppnix's `addConstant`
# leaves `currentSystem` out of `builtins` entirely (`eval.cc:541`). Read
# impurely here instead, the same way `STOREDIR` above is.
#
# Worth the sentence, because writing it the other way is what found a real
# divergence: this fixture built on the rust arm and failed on cpp with
# `attribute 'currentSystem' missing`, because the backend was serving the
# constant under pure eval where cppnix has none. Fixed in
# `CPP_IMPURE_ONLY_CONSTANTS`; the fixture stays honest either way.
SYSTEM=$(NIX_CONFIG="$CPP" "$NIXBIN" eval --raw --impure --expr builtins.currentSystem 2>&1) || {
  echo "drv-parity: could not read builtins.currentSystem, so the flake fixture cannot name a system: $SYSTEM"
  exit 2
}
cat > "$FLAKEDIR/flake.nix" <<NIXEOF
{
  description = "drv-parity flake fixture";
  outputs = { self }: {
    packages."$SYSTEM".fixture = derivation {
      name = "$FRESH-flake";
      system = "$SYSTEM";
      builder = "/bin/sh";
      args = [ "-c" "echo drv-parity-flake > \$out" ];
    };
  };
}
NIXEOF

# label | kind | target | attrpath | the derivation name that must NOT already
# be in the store.
#
# `kind` is `file` or `flake`, and it decides how the two arms name the thing:
# `-f <target> <attr>` against a file, `<target>#<attr>` against a flake. Two
# spellings and not two loops, because everything the loop asserts -- the
# `.drv` is written, its bytes match, the drvPath matches, the outPath from a
# real build matches -- is the same question of both.
#
# Empty in the last field means "this case cannot be fresh", which is only
# `nixpkgs hello`: a machine that has ever built hello already has the `.drv`,
# so its presence proves nothing and is reported rather than asserted.
#
# The name is per case and not a shared prefix. Globbing on the prefix is what
# the first three-case run did, and the second fixture then found the first
# fixture's `.drv` sitting there and scored MISMATCH on a case whose drvPath,
# bytes and outPath all agreed.
declare -a BUILD_CASES=(
  "fresh fixture|file|$W/fresh.nix||$FRESH"
  "outputsToInstall|file|$W/outputs.nix||$FRESH-multi"
  "flake fixture|flake|path:$FLAKEDIR|fixture|$FRESH-flake"
)
if [ -n "${NIXPKGS:-}" ] && [ -d "${NIXPKGS:-}" ]; then
  printf '(import %s { }).hello\n' "$NIXPKGS" > "$W/hello.nix"
  BUILD_CASES+=("nixpkgs hello|file|$W/hello.nix||")
fi
# The same package reached through a flake reference rather than an import, so
# the flake path is measured against a real stdenv derivation and not only
# against a four-line fixture. Gated on a flake reference rather than on
# NIXPKGS, because a nixpkgs checkout is a directory and a flake reference is
# a locked pin, and the parity claim wants the pin: `nixpkgs` out of the
# registry is whatever the registry says today.
if [ -n "${NIXPKGS_FLAKE:-}" ]; then
  BUILD_CASES+=("flake nixpkgs hello|flake|$NIXPKGS_FLAKE|hello|")
fi

bmatch=0; bmismatch=0; bunimpl=0; bn=0
for case in "${BUILD_CASES[@]}"; do
  bn=$((bn+1))
  label=${case%%|*}; rest=${case#*|}
  kind=${rest%%|*}; rest=${rest#*|}
  target=${rest%%|*}; rest=${rest#*|}
  attr=${rest%%|*}; freshname=${rest#*|}
  # How each arm names the thing, once, so the eval and the build cannot end
  # up pointing at different values.
  if [ "$kind" = flake ]; then
    installable="$target#${attr}"
    drvinstallable="$target#${attr:+$attr.}drvPath"
    set -- build --no-link --print-out-paths "$installable"
    declare -a DRVARGS=(eval --raw "$drvinstallable")
  else
    set -- build --no-link --print-out-paths -f "$target"
    [ -n "$attr" ] && set -- "$@" "$attr"
    declare -a DRVARGS=(eval --raw -f "$target" "${attr:+$attr.}drvPath")
  fi

  # Whether anything already sits where this case's `.drv` will go, read
  # before either arm runs. For the fixture it must be nothing; for hello it
  # is whatever the machine happens to have and is reported, not asserted.
  pre=present
  if [ -n "$freshname" ]; then
    # A glob and an existence test rather than `ls`: an unmatched glob is left
    # literal by the shell, so `-e` is what tells "nothing is there" from "one
    # file is there" without parsing anything.
    pre=absent
    for candidate in "$STOREDIR"/*-"$freshname".drv; do
      [ -e "$candidate" ] || continue
      pre=present
      break
    done
  fi

  # The rust arm first, before anything else can have written the `.drv`, and
  # `nix eval` before `nix build` within it. The order is the assertion: a
  # bare evaluation is enough to put the `.drv` in the store, and checking
  # after the build instead would let a backend that wrote nothing at
  # evaluation time pass on the strength of what the builder did.
  rdrv=$(NIX_CONFIG="$RUST" "$NIXBIN" "${DRVARGS[@]}" 2>"$W/rdrv.err"); rdrvrc=$?
  [ "$rdrvrc" = 0 ] || rdrv=$(cat "$W/rdrv.err")
  # Opened, not just printed. This is the check the whole class turns on: a
  # computed path and a written one print identically, and every arm above
  # compares only what was printed, which is how ENG-12799 stayed invisible
  # through a green suite.
  present=absent
  [ -f "$rdrv" ] && present=present
  rsum=""
  [ "$present" = present ] && rsum=$(sha256sum "$rdrv" | cut -d" " -f1)

  # stdout and stderr kept apart, unlike the eval arm above, and that is not
  # tidiness. `--print-out-paths` writes the paths to stdout while the build
  # progress goes to stderr, and on a shared store the arm that runs first is
  # the one that does the building -- so a merged capture makes the two arms
  # differ by "this derivation will be built" every single time, which is
  # exactly what the first run of this arm scored: a MISMATCH on a case whose
  # drvPath, `.drv` bytes and outPath were all identical.
  ro=$(NIX_CONFIG="$RUST" "$NIXBIN" "$@" 2>"$W/rbuild.err"); rrc=$?

  co=$(NIX_CONFIG="$CPP" "$NIXBIN" "$@" 2>"$W/cbuild.err"); crc=$?
  cdrv=$(NIX_CONFIG="$CPP" "$NIXBIN" "${DRVARGS[@]}" 2>"$W/cdrv.err"); cdrvrc=$?
  [ "$cdrvrc" = 0 ] || cdrv=$(cat "$W/cdrv.err")
  csum=""
  [ -f "$cdrv" ] && csum=$(sha256sum "$cdrv" | cut -d" " -f1)

  if grep -qiE "rust-eval unimplemented" "$W/rbuild.err" "$W/rdrv.err"; then
    bunimpl=$((bunimpl+1)); verdict=unimplemented
  elif [ "$rrc" != 0 ] || [ "$crc" != 0 ] || [ "$rdrvrc" != 0 ] || [ "$cdrvrc" != 0 ]; then
    # A failure on either arm is a mismatch here and never an agreement: two
    # arms that both failed have produced no outPath to compare, which is the
    # whole question this arm asks.
    bmismatch=$((bmismatch+1)); verdict=MISMATCH
  elif [ "$present" != present ]; then
    bmismatch=$((bmismatch+1)); verdict=MISMATCH
  elif [ -n "$freshname" ] && [ "$pre" != absent ]; then
    # The one case whose freshness is the whole assertion. If it was already
    # there, this case proved nothing about whether the rust arm writes.
    bmismatch=$((bmismatch+1)); verdict=MISMATCH
  elif [ -z "$ro" ] || [ "$ro" != "$co" ] || [ "$rdrv" != "$cdrv" ] || [ -z "$rsum" ] || [ "$rsum" != "$csum" ]; then
    bmismatch=$((bmismatch+1)); verdict=MISMATCH
  else
    bmatch=$((bmatch+1)); verdict=match
  fi

  printf "%-24s %s\n" "$verdict" "$label"
  printf "      drv  rust=%s cpp=%s pre-rust=%s after-rust=%s\n" "$rdrv" "$cdrv" "$pre" "$present"
  printf "      sha  rust=%s cpp=%s\n" "${rsum:-<none>}" "${csum:-<none>}"
  if [ "$verdict" != match ]; then
    printf "      out  rust rc=%s stdout=[%s] stderr=[%s]\n" \
      "$rrc" "$(printf "%s" "$ro" | tr "\n" "|")" "$(tail -3 "$W/rbuild.err" | tr "\n" "|")"
    printf "      out  cpp  rc=%s stdout=[%s] stderr=[%s]\n" \
      "$crc" "$(printf "%s" "$co" | tr "\n" "|")" "$(tail -3 "$W/cbuild.err" | tr "\n" "|")"
  else
    printf "      out  %s\n" "$ro"
  fi
done

echo "RESULT drv-parity-build cases=$bn match=$bmatch mismatch=$bmismatch unimplemented=$bunimpl \
expected-cases=$DRV_PARITY_BUILD_CASES min-match=$DRV_PARITY_BUILD_MIN_MATCH store=$(dirname "${cdrv:-/nix/store/x}")"

if [ "$bmismatch" != 0 ]; then
  echo "drv-parity: $bmismatch build case(s) diverged"
  ok=0
fi
# Exact, for the reason the eval arm's count is: the case list is right here,
# except for the nixpkgs row, which is why NIXPKGS being unset changes the
# expected count rather than silently dropping a case.
want_build_cases=$DRV_PARITY_BUILD_CASES
if [ -z "${NIXPKGS:-}" ] || [ ! -d "${NIXPKGS:-}" ]; then
  want_build_cases=$((want_build_cases - 1))
  echo "drv-parity: NIXPKGS is unset or not a directory, so the 'nixpkgs hello' build case did not run; expecting $want_build_cases build cases rather than $DRV_PARITY_BUILD_CASES"
fi
if [ -z "${NIXPKGS_FLAKE:-}" ]; then
  want_build_cases=$((want_build_cases - 1))
  echo "drv-parity: NIXPKGS_FLAKE is unset, so the 'flake nixpkgs hello' build case did not run; expecting $want_build_cases build cases rather than $DRV_PARITY_BUILD_CASES"
fi
if [ "$bn" != "$want_build_cases" ]; then
  echo "drv-parity: ran $bn build cases, expected $want_build_cases. Update DRV_PARITY_BUILD_CASES in the same commit that changes BUILD_CASES."
  ok=0
fi
if [ "$bmatch" -lt "$((DRV_PARITY_BUILD_MIN_MATCH - (DRV_PARITY_BUILD_CASES - want_build_cases)))" ]; then
  echo "drv-parity: build match=$bmatch is under the floor; nix build stopped agreeing, or the rust arm stopped serving it."
  ok=0
fi

# -- pure-eval readback of store paths the evaluator itself minted ----------
#
# `builtins.path`, `builtins.filterSource` and `builtins.toFile` mint a store
# path, and cppnix registers each one on the pure-eval allow list at the site
# that mints it (`primops.cc:2995`, `:2997`, `:2836`). The bridge hooks used
# to answer with the path and skip the registration, so under flake eval
# (which is pure) the first READBACK of a minted tree failed on the rust arm
# alone with "access to absolute path ... is forbidden in pure evaluation
# mode". That one omission was 96 of the 100 rust-arm failures in the
# 2026-08-07 whole-ix sweep: every `lib.cleanSource`d tree came back
# unregistered. ENG-13138.
#
# One case per minting hook. Both arms must produce the bytes (rc 0), and the
# bytes must be equal; a pair that FAILS ALIKE is scored as a failure here,
# unlike the eval cases above, because cppnix passing these is the premise.
# `--no-eval-cache`, or the second arm would be served the first arm's answer
# without evaluating anything.
MINTDIR=$(mkdir -p "$W/mintflake/sub" && cd "$W/mintflake" && pwd -P)
echo "mint readback bytes" > "$MINTDIR/sub/f.txt"
cat > "$MINTDIR/flake.nix" <<'NIXEOF'
{
  description = "drv-parity mint-readback fixture";
  outputs = { self }: {
    viaPath = builtins.readFile (builtins.path { path = ./sub; name = "mint-path"; } + "/f.txt");
    viaFilter = builtins.readFile (builtins.filterSource (p: t: true) ./sub + "/f.txt");
    viaToFile = builtins.readFile (builtins.toFile "mint-tofile" "to-file bytes");
  };
}
NIXEOF
echo
echo "-- pure-eval readback of minted store paths (ENG-13138) ----------"
mintmatch=0
for attr in viaPath viaFilter viaToFile; do
  cppout=$(NIX_CONFIG="$CPP" "$NIXBIN" eval --no-eval-cache --raw "path:$MINTDIR#$attr" 2>&1); cpprc=$?
  rustout=$(NIX_CONFIG="$RUST" "$NIXBIN" eval --no-eval-cache --raw "path:$MINTDIR#$attr" 2>&1); rustrc=$?
  if [ "$cpprc" = 0 ] && [ "$rustrc" = 0 ] && [ "$cppout" = "$rustout" ]; then
    mintmatch=$((mintmatch+1))
    printf 'match                    %s\n' "$attr"
  else
    printf 'MISMATCH                 %s\n' "$attr"
    printf '      cpp  rc=%s %s\n' "$cpprc" "$cppout"
    printf '      rust rc=%s %s\n' "$rustrc" "$rustout"
  fi
done
echo "RESULT drv-parity-mint cases=3 match=$mintmatch expected-match=3"
if [ "$mintmatch" != 3 ]; then
  echo "drv-parity: a minted store path could not be read back under pure eval on both arms"
  ok=0
fi

# -- two flakes, one cache: warm the right one, never the wrong one ---------
#
# Every flake evaluates the *same* `call-flake.nix` from the same base
# directory, so the module digest, the settings fingerprint and the question
# are equal for two different flakes. What separates them is the argument
# axis of the memo key (ENG-12915): the lock file and the overrides document
# the bridge applies are hashed into the identity, byte for byte.
#
# Before that axis existed, `mayBeMemoised` kept the flake path away from the
# memo table entirely, and this arm checked distinctness only. That was worth
# having and was not a proof: breaking `mayBeMemoised` left this arm passing,
# because the two fixtures happened to read different store paths and so
# recorded different read sets. The read-set replay is not a discriminator in
# general -- a witness is filed under the identity alone, so two evaluands
# with one identity share one witness, and the second replays the first's
# questions and matches its row. `capi::warm_starts::two_flakes_over_one_cache_are_two_rows`
# is that demonstrated on a pair that reads nothing: with the argument axis
# removed it serves `/nix/store/aaaa-one` for a flake whose answer is
# `/nix/store/aaaa-two`.
#
# So this arm now asserts three things rather than one, because two of them
# are what stop it passing vacuously:
#
#   distinct   two flakes, two drvPaths. Fails if one flake is served the
#              other's answer.
#   warm       re-running flake 1 writes no new objects. Fails if the flake
#              path has stopped memoising, which is the state this whole
#              change exists to leave -- and which "two drvPaths" cannot see,
#              since a cache that never hits also never serves the wrong one.
#   cold       flake 2 writes new objects. Fails if the key has collapsed to
#              something that does not name the flake.
#
# The cache directory is this run's own, so a stale one cannot make it pass,
# and the first evaluation is what populates it.
echo
echo "-- two flakes, one cache (warm the right one, never the wrong one) --"
poison_ok=1
POISONCACHE=$W/poison-cache
# Written out twice rather than through a loop with `eval` and a dynamic
# variable name: shellcheck cannot see through that construct and reports the
# two paths as unassigned (SC2154), and a warning nobody can act on is how a
# real one gets ignored.
poison_flake() {
  local dir=$1 name=$2 body=$3
  cat > "$dir/flake.nix" <<NIXEOF
{
  outputs = { self }: {
    packages."$SYSTEM".fixture = derivation {
      name = "$name";
      system = "$SYSTEM";
      builder = "/bin/sh";
      args = [ "-c" "echo $body > \$out" ];
    };
  };
}
NIXEOF
}
poisondir1=$(mkdir -p "$W/poison1" && cd "$W/poison1" && pwd -P)
poisondir2=$(mkdir -p "$W/poison2" && cd "$W/poison2" && pwd -P)
poison_flake "$poisondir1" "$FRESH-poison1" poison1
poison_flake "$poisondir2" "$FRESH-poison2" poison2
poison_eval() { # dir  errfile
  NIX_CONFIG="$RUST
eval-cache-dir = $POISONCACHE" "$NIXBIN" eval --raw "path:$1#fixture.drvPath" 2>"$2"
}
poison_objs() { find "$POISONCACHE/objects" -type f 2>/dev/null | wc -l | tr -d " "; }

poison1=$(poison_eval "$poisondir1" "$W/poison1.err"); p1rc=$?
objs_after_1=$(poison_objs)
poison1b=$(poison_eval "$poisondir1" "$W/poison1b.err"); p1brc=$?
objs_after_1b=$(poison_objs)
poison2=$(poison_eval "$poisondir2" "$W/poison2.err"); p2rc=$?
objs_after_2=$(poison_objs)

poison_warm=0
poison_cold=0
if [ "$p1rc" != 0 ] || [ "$p1brc" != 0 ] || [ "$p2rc" != 0 ]; then
  echo "  FAILED: a flake did not evaluate, so nothing was compared"
  printf "      1  rc=%s %s\n" "$p1rc" "$(tail -2 "$W/poison1.err" | tr "\n" "|")"
  printf "      1b rc=%s %s\n" "$p1brc" "$(tail -2 "$W/poison1b.err" | tr "\n" "|")"
  printf "      2  rc=%s %s\n" "$p2rc" "$(tail -2 "$W/poison2.err" | tr "\n" "|")"
  poison_ok=0
else
  if [ "$objs_after_1" -eq 0 ]; then
    echo "  FAILED: the first flake wrote no objects, so nothing below is about a cache"
    poison_ok=0
  fi
  if [ "$poison1" = "$poison2" ]; then
    echo "  MISMATCH: two different flakes produced one drvPath, so the cache answered for the wrong one"
    printf "      both=%s\n" "$poison1"
    poison_ok=0
  else
    echo "  match: two flakes, two drvPaths"
    printf "      1=%s\n      2=%s\n" "$poison1" "$poison2"
  fi
  # A repeat of flake 1 must be served: same answer, and no new objects. The
  # object count is the fact; the answer agreeing proves nothing, because a
  # re-evaluation agrees too.
  if [ "$poison1b" = "$poison1" ] && [ "$objs_after_1b" = "$objs_after_1" ]; then
    poison_warm=1
    echo "  warm: re-running flake 1 was served (objects stayed at $objs_after_1)"
  else
    echo "  COLD: re-running flake 1 was not served, so the flake path is not memoising"
    printf "      objects %s -> %s, answer %s -> %s\n" \
      "$objs_after_1" "$objs_after_1b" "$poison1" "$poison1b"
    poison_ok=0
  fi
  # And flake 2 must miss, which is the same fact from the other side.
  if [ "$objs_after_2" -gt "$objs_after_1b" ]; then
    poison_cold=1
    echo "  miss: flake 2 wrote its own rows (objects $objs_after_1b -> $objs_after_2)"
  else
    echo "  SHARED: flake 2 wrote nothing, so it was answered out of flake 1's row"
    poison_ok=0
  fi
fi
[ "$poison_ok" = 1 ] || ok=0
echo "RESULT drv-parity-flake-cache distinct=$poison_ok warm=$poison_warm cold=$poison_cold"

# -- does a memo hit still write the `.drv`? (ENG-12801) --------------------
#
# The write leaves the evaluator as a question, and `RecordingHost` records it
# as the `Question::StoreText` it is, so validating a memoised result re-asks
# it and the embedder writes the file again. If that were wrong, a warm cache
# would produce builds pointing at `.drv` paths nothing wrote: ENG-12799 again
# but behind a cache, where it is worse.
#
# Two arms, both asserting, because there are two ways into the memo table
# and a warm cache has to keep this promise on each.
#
#   nix-instantiate  `--eval --strict --read-write-mode`, the one-call path
#                    (`ixe_eval_expr`). `--read-write-mode` turns off the
#                    read-only mode that would otherwise mean nothing is
#                    written either way.
#   handle path      `nix eval --raw -f`, which is the pipeline `nix build`
#                    shares. Until ENG-12830 this was a timing report with no
#                    assertion in it, because the path wrote objects into
#                    `eval-cache-dir` and served none of them, so there was no
#                    hit to make the assertion about. It hits now, through a
#                    memo key that carries the question as well as the module,
#                    and the assertion is the same one.
#
# Each arm: cold, delete the `.drv`, warm. The file must come back at the same
# path and hash, or the warm run must refuse by name.
#
# Measured on dev-compute-6 for arm 1 at the earlier branch tip: cold 2.195s,
# warm 0.044s (a 50x hit). Arm 2 measured on darwin at the warm-starts tip:
# cold 1.84s, warm 0.26s.
#
# Which side performs the write on a hit, since the whole case turns on it:
# the *replay*, not the served answer. `ResultCache::lookup` re-asks every
# recorded question in order to compute the key from the answers given now,
# and the write left the evaluator as one of those questions, so asking it
# again performs it. The served answer is a drvPath string and could not
# create a file if it tried.
#
# Watched failing on both arms, by making `RecordingHost::write_derivation`
# forward without noting. Arm 1 was broken that way when ENG-12801 landed;
# arm 2 was broken the same way here and behaved identically: cold 2.580s
# wrote the `.drv`, `store delete` removed it, and the warm run hit the memo
# (0.267s) and answered the same drvPath with no file behind it -- a build
# pointed at a path nothing wrote, which is ENG-12799 behind a cache. Arm 2
# was also broken by pointing `machine_and_host` at `RealFs`, which takes the
# whole walk out of the read set.
echo
echo "-- memo-hit drv write (ENG-12801) ---------------------------------"
MEMO_NAME="$FRESH-memo"
# Deliberately expensive to evaluate and trivial to build. A hit has to be
# visible over ~0.25s of process startup, and a 300k fold is not: measured at
# 0.25s cold and 0.25s warm, which reads as "no hit" whether or not there was
# one. Three million is ~2.2s, where a hit shows up as two orders of
# magnitude.
HEAVY_FOLD="builtins.foldl' (a: b: a + b) 0 (builtins.genList (i: i) 3000000)"
drv_fixture() { # name  outfile
  cat > "$2" <<NIXEOF
(derivation {
  name = "$1";
  system = builtins.currentSystem;
  builder = "/bin/sh";
  args = [ "-c" "echo memo > \$out" ];
  heavy = builtins.toString ($HEAVY_FOLD);
}).drvPath
NIXEOF
}
drv_fixture "$MEMO_NAME" "$W/memo.nix"
drv_fixture "$MEMO_NAME-cov" "$W/memo-cov.nix"

# One arm: cold, delete the `.drv`, warm, and require the file back at the
# same path and hash.
#
# A function and not two copies, and that is the whole point of this edit.
# ENG-12801 asked for this assertion and it was written for
# `nix-instantiate`, because that was the only entry point that could hit the
# memo. Now that the handle path hits too (ENG-12830) the choice was to extend
# the assertion or to leave half the surface covered by a timing report with
# no assertion in it. A second copy would have drifted the first time either
# was touched.
#
# Sets `arm_verdict`, `arm_detail`, `arm_cold`, `arm_warm`, `arm_objs`.
memo_arm() { # cachedir  tag  cmd...
  local cache=$1 tag=$2
  shift 2
  arm_verdict=UNMEASURED
  arm_detail=
  arm_cold="?"
  arm_warm="?"
  arm_objs=0
  rm -rf "$cache"
  local cfg="$RUST
eval-cache-dir = $cache"
  local t out rc drv out2 rc2 drv2 hit
  t=$(now)
  out=$(NIX_CONFIG="$cfg" "$@" 2>"$W/$tag-1.err")
  rc=$?
  arm_cold=$(elapsed "$t" "$(now)")
  drv=$(printf %s "$out" | tr -d '"')
  arm_objs=$(find "$cache/objects" -type f 2>/dev/null | wc -l | tr -d " ")

  if [ "$rc" != 0 ]; then
    arm_detail="the cold evaluation failed: $(tail -2 "$W/$tag-1.err" | tr "\n" "|")"
    return 0
  fi
  if [ ! -f "$drv" ]; then
    arm_detail="the cold evaluation printed $drv and did not write it, so ENG-12799 is back"
    return 0
  fi
  if [ "$arm_objs" -eq 0 ]; then
    arm_detail="eval-cache-dir wrote no objects, so the warm run below could not be one"
    return 0
  fi
  if ! "$NIXBIN" store delete "$drv" > "$W/$tag-del.out" 2>&1; then
    arm_detail="could not delete $drv, so the warm run would have found the cold run's file: $(tail -2 "$W/$tag-del.out" | tr "\n" "|")"
    return 0
  fi
  if [ -f "$drv" ]; then
    arm_detail="store delete reported success and $drv is still there"
    return 0
  fi

  t=$(now)
  out2=$(NIX_CONFIG="$cfg" "$@" 2>"$W/$tag-2.err")
  rc2=$?
  arm_warm=$(elapsed "$t" "$(now)")
  drv2=$(printf %s "$out2" | tr -d '"')
  hit=$(much_faster "$arm_cold" "$arm_warm")
  if [ "$hit" != yes ]; then
    # Timing decides, and a touched index row does not: a row is touched on a
    # write as well as on a hit, so reading the touch alone once scored this
    # case `match` on a run that had simply re-evaluated (0.257s warm against
    # 0.251s cold).
    arm_verdict=nohit
    arm_detail="the warm run took ${arm_warm}s against a cold ${arm_cold}s, so it re-evaluated and the assertion never ran"
  elif grep -qiE "rust-eval unimplemented" "$W/$tag-2.err"; then
    arm_verdict=refused
    arm_detail="hit the memo (${arm_cold}s to ${arm_warm}s) and refused by name: $(grep -o "rust-eval unimplemented: .*" "$W/$tag-2.err" | head -1 | cut -c1-90)"
  elif [ "$rc2" != 0 ]; then
    arm_detail="hit the memo (${arm_cold}s to ${arm_warm}s) and failed without a refusal token: $(tail -2 "$W/$tag-2.err" | tr "\n" "|")"
  elif [ "$drv2" != "$drv" ]; then
    arm_detail="the warm run answered $drv2 where the cold run answered $drv"
  elif [ ! -f "$drv" ]; then
    arm_detail="hit the memo (${arm_cold}s to ${arm_warm}s), answered $drv2, and left no file at it: a warm cache produces builds with missing .drvs"
  else
    arm_verdict=match
    arm_detail="hit the memo (${arm_cold}s to ${arm_warm}s) after the .drv was deleted, and the write was re-performed, sha $(sha256sum "$drv" | cut -d" " -f1 | cut -c1-16)"
  fi
  return 0
}

now() { python3 -c 'import time;print(f"{time.time():.4f}")'; }
elapsed() { python3 -c "print(f'{$2-$1:.3f}')"; }
much_faster() { python3 -c "print('yes' if $2 < $1 * 0.5 else 'no')"; }

# -- arm 1: nix-instantiate, the one-call path (`ixe_eval_expr`) --
#
# `--read-write-mode` turns off the read-only mode that would otherwise mean
# nothing is written either way.
memo_arm "$W/memo-cache" memo "$NIXI" --eval --strict --read-write-mode "$W/memo.nix"
memo_verdict=$arm_verdict
memo_detail=$arm_detail
a_cold=$arm_cold
a_warm=$arm_warm
a_objs=$arm_objs

# -- arm 2: the handle path, which `nix eval` and `nix build` share --
#
# Ratcheted separately from arm 1 rather than folded into it. The two are
# different mechanisms -- one memo key built from a module, one built from a
# module and a question -- and a single verdict over both would let one of
# them stop hitting while the other carried the pass.
memo_arm "$W/memo-cov-cache" memo-cov "$NIXBIN" eval --raw -f "$W/memo-cov.nix"
cov_verdict=$arm_verdict
cov_detail=$arm_detail
cov_cold=$arm_cold
cov_warm=$arm_warm

printf "%-24s %s\n" "$memo_verdict" "a memo hit still writes the .drv (nix-instantiate)"
printf "      %s\n" "${memo_detail:-<nothing recorded>}"
printf "%-24s %s\n" "$cov_verdict" "a memo hit still writes the .drv (the nix eval / nix build path)"
printf "      %s\n" "${cov_detail:-<nothing recorded>}"
echo "RESULT drv-parity-memo verdict=$memo_verdict cold=${a_cold}s warm=${a_warm:-?}s cache-objects=${a_objs:-0} \
build-path-verdict=$cov_verdict build-path-cold=${cov_cold}s build-path-warm=${cov_warm:-?}s \
expected=$DRV_PARITY_MEMO_VERDICT/$DRV_PARITY_MEMO_BUILD_PATH_HITS"

for arm in "nix-instantiate:$memo_verdict" "handle-path:$cov_verdict"; do
  case ${arm#*:} in
    match|refused) ;;
    *)
      echo "drv-parity: the memo-hit case did not pass on ${arm%%:*} (ENG-12801). A warm cache must not produce a drvPath with no file behind it."
      ok=0
      ;;
  esac
done
if [ "$memo_verdict" != "$DRV_PARITY_MEMO_VERDICT" ]; then
  echo "drv-parity: the memo case scored '$memo_verdict' where gate-ratchets.sh expects '$DRV_PARITY_MEMO_VERDICT'."
  ok=0
fi
# The handle path is ratcheted separately from the one-call path. It moved
# from `no` to `match` with ENG-12830; if it ever goes back to `nohit` the
# gate says which arm stopped hitting rather than reporting one verdict over
# both, where either arm could carry the other's pass.
if [ "$cov_verdict" != "$DRV_PARITY_MEMO_BUILD_PATH_HITS" ]; then
  echo "drv-parity: the nix eval / nix build path scored '$cov_verdict' where gate-ratchets.sh expects '$DRV_PARITY_MEMO_BUILD_PATH_HITS'. If it is 'nohit', eval-cache-dir has stopped serving the handle path and ENG-12830 has regressed."
  ok=0
fi

[ "$ok" = 1 ]
