#!/usr/bin/env bash
# Does the Rust driver do the same thing as the C++ CLI?
#
# `nix-eval-driver` is the first ship-of-Theseus plank for the *entry point*:
# a Rust binary that evaluates and instantiates without the C++ CLI shell
# (CLAUDE.md, "Direction: full Rust, ship of Theseus"). This gate is the
# proof obligation that goes with it. Every case below is instantiated three
# ways --
#
#   driver   nix-eval-driver, no C++ in the process at all
#   cpp      the bridge binary with eval-backend = cpp
#   rust     the bridge binary with eval-backend = rust
#
# -- and all three must agree on the drvPath, on the outPath, and on the
# `.drv` file BYTE FOR BYTE.
#
# ## This is tier 1, and there is no allowlist
#
# CLAUDE.md's parity bar puts drvPath, outPath and `.drv` bytes in tier 1:
# byte identity IS functional identity there, because a different outPath is a
# different store path and nothing substitutes. So this script deliberately
# does not read eval-allowlist.toml, which can only ever waive tier 2. A
# single byte of divergence is a defect to fix, never an entry to add.
#
# ## Why three store roots, and why that is what makes the byte check real
#
# Each arm writes into its own root via cppnix's `--store local?root=DIR` and
# the driver's `--store-root DIR`. Both spellings keep `/nix/store` as the
# *hashed* store directory while putting the real directory at
# `DIR/nix/store`, so all three arms compute the same path and land three
# separate files.
#
# That separation is the point. `drv-parity.sh`'s build arm compares two arms
# that both write the real store, where the second write is a no-op onto an
# existing path and the two "files" being compared are one file -- a
# comparison that cannot fail. Here they are three distinct files on disk and
# a byte difference shows up as one.
#
# It also means the check covers a failure that comparing printed paths
# cannot: an arm that prints the right drvPath and writes nothing. That
# happened for real on 2026-08-06 on dev-compute-6 (ENG-12799), so each arm's
# `.drv` is required to EXIST and is reported as `no-drv-written(arm)` when it
# does not.
#
# ## Why the bridge arms run `nix eval` and not `nix-instantiate`
#
# Measured, not assumed: `nix-instantiate` under `eval-backend = rust` refuses
# every case with `[command-unsupported] nix-instantiate without --eval` --
# the rust backend has no instantiate path in that command yet (ENG-13109).
# So the bridge arms ask for `(expr).drvPath` through `nix eval --raw`, which
# is what `drv-parity.sh` does for the same reason. The driver keeps its own
# `instantiate` command, since having one is the plank.
#
# ## Requirements
#
# A bridge binary with the Rust evaluator linked in, which the default build
# is not:
#
#   nix develop --command nix shell nixpkgs#cargo nixpkgs#rustc --command bash -c \
#     'meson setup build-rust --prefix="$out" -Dnix:rust-eval=enabled && ninja -C build-rust'
#
# and the driver, built from this tree:
#
#   cargo build --release -p nix-eval-driver
#
# `rust-driver-parity-selftest.sh` is this gate's own guard: it hands the gate
# drivers broken in specific ways and requires it to fail naming each one. Run
# it whenever this file changes.
#
# Usage: rust-driver-parity.sh BUILDDIR/src/nix [PATH-TO-nix-eval-driver]
#
# Exits non-zero unless all of these hold:
#
#   mismatch      == 0
#   cases         == RUST_DRIVER_PARITY_CASES           (gate-ratchets.sh)
#   match         >= RUST_DRIVER_PARITY_MIN_MATCH       (gate-ratchets.sh)
#   unimplemented <= RUST_DRIVER_PARITY_MAX_UNIMPLEMENTED
#
# The last three are what stop the gate being satisfied by measuring nothing.
# A driver that refused every case would score `unimplemented=N mismatch=0`
# and exit 0 on the first condition alone; so would a run whose case list had
# silently shrunk. That failure mode is not hypothetical in this repo --
# `drv-parity.sh`'s own header records it happening -- and it is the reason
# every count below is checked against a number somebody wrote down.

set -u
# `pipefail` as well: nothing here reads a pipeline exit code today, but a
# gate is exactly the place a silently-swallowed failure is expensive.
set -o pipefail

BIN=${1:-}
[ -n "$BIN" ] || { echo "usage: rust-driver-parity.sh BUILDDIR/src/nix [DRIVER]"; exit 2; }
[ -x "$BIN/nix" ] || { echo "no nix at $BIN/nix"; exit 2; }
NIX=$BIN/nix
NIXI=$BIN/nix-instantiate

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
DRIVER=${2:-$repo/rust/target/release/nix-eval-driver}
[ -x "$DRIVER" ] || {
  echo "no nix-eval-driver at $DRIVER"
  echo "  build it with: cargo build --release -p nix-eval-driver   (in $repo/rust)"
  exit 2
}

# shellcheck source=./arm-config.sh
. "$here/arm-config.sh" || exit 2
# One owner of the gates' nix configuration, before anything reads the
# environment: an ambient `lint-url-literals = fatal` otherwise makes the rust
# arm refuse every row and the gate score `unimplemented` throughout
# (ENG-12996).
arm_pin_environment
# shellcheck source=./gate-ratchets.sh
. "$here/gate-ratchets.sh" || exit 2

BASE="extra-experimental-features = rust-eval nix-command flakes
$(arm_base_config)"
CPP="$BASE
eval-backend = cpp"
RUST="$BASE
eval-backend = rust"

arm_require_clean_config "$NIX"

# `pwd -P` and not the raw `mktemp -d`: on macOS that returns a path under
# `/var`, which is a symlink to `/private/var`, and cppnix refuses a store
# root reached through one -- "error: the path '/var' is a symlink". Every
# case failed that way on the first run of this gate.
W=$(cd "$(mktemp -d)" && pwd -P)

# cpp writes its store directories read-only, so a plain `rm -rf` on a root it
# made fails with "Permission denied" and leaves the next case's store
# polluted with the last case's `.drv`. Make them writable first.
scrub() {
  for victim in "$@"; do
    [ -e "$victim" ] || continue
    chmod -R u+w "$victim" 2>/dev/null
    rm -rf "$victim"
  done
}
trap 'scrub "$W"' EXIT

# ---------------------------------------------------------------------------
# Capability probes
#
# Three of them, one per arm, and each asks the arm to actually evaluate.
# `nix config show` reports `eval-backend = rust` on a binary compiled with
# `-Dnix:rust-eval=disabled`, so reading the setting back proves nothing: a
# setting is not a capability (ENG-12542). For the two bridge arms the
# stronger form is used as well -- NIX_SHOW_STATS names which evaluator served
# the evaluation, derived from a count rather than echoed from the setting --
# because two arms that were secretly the same backend would make every
# comparison below vacuous while passing.
# ---------------------------------------------------------------------------
for arm in CPP RUST; do
  case $arm in CPP) cfg=$CPP ;; *) cfg=$RUST ;; esac
  got=$(NIX_CONFIG="$cfg" "$NIXI" --eval --strict -E 1 2>&1)
  [ "$got" = 1 ] || {
    echo "rust-driver-parity: the $arm arm cannot evaluate the probe expression '1':"
    echo "$got"
    exit 2
  }
  case $arm in CPP) want=cpp ;; *) want=rust ;; esac
  NIX_CONFIG="$cfg" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/stats-$arm.json" \
    "$NIXI" --eval --strict -E 1 > /dev/null 2>&1
  ev=$(python3 -c 'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("evaluator", "<absent>"))
except OSError:
    print("<no stats file>")' "$W/stats-$arm.json")
  [ "$ev" = "$want" ] || {
    echo "rust-driver-parity: the $arm arm asked for the '$want' evaluator, NIX_SHOW_STATS reports '$ev'; the two bridge arms would be one backend and every comparison below vacuous"
    exit 2
  }
  echo "probe: $arm arm evaluates, NIX_SHOW_STATS confirms the '$ev' backend ran"
done

got=$("$DRIVER" eval -E 1 2>&1)
[ "$got" = 1 ] || {
  echo "rust-driver-parity: the driver cannot evaluate the probe expression '1':"
  echo "$got"
  exit 2
}
echo "probe: driver arm evaluates"

# ---------------------------------------------------------------------------
# What the arms must agree about the world
#
# Read off the bridge binary rather than hardcoded, and then GIVEN to the
# driver, because all three are in the derivation's own bytes: `storeDir` is
# hashed into every store path, `currentSystem` is a field of the `.drv`, and
# `nixVersion` is observable from a program. A driver defaulting to a
# different `system` than the binary was built for would produce a mismatch in
# every case and it would say nothing about the evaluator.
# ---------------------------------------------------------------------------
STORE_DIR=$(NIX_CONFIG="$CPP" "$NIXI" --eval --raw -E 'builtins.storeDir' 2>/dev/null)
SYSTEM=$(NIX_CONFIG="$CPP" "$NIXI" --eval --raw -E 'builtins.currentSystem' 2>/dev/null)
NIXVER=$(NIX_CONFIG="$CPP" "$NIXI" --eval --raw -E 'builtins.nixVersion' 2>/dev/null)
for pair in "STORE_DIR:$STORE_DIR" "SYSTEM:$SYSTEM" "NIXVER:$NIXVER"; do
  [ -n "${pair#*:}" ] || { echo "rust-driver-parity: could not read ${pair%%:*} off $NIXI"; exit 2; }
done
echo "world: storeDir=$STORE_DIR system=$SYSTEM nixVersion=$NIXVER"

D="name = \"g\"; system = \"$SYSTEM\"; builder = \"/bin/sh\";"

# ---------------------------------------------------------------------------
# The corpus
#
# Chosen so that a wrong answer moves a store path rather than only a printed
# string: multiple outputs, cross-derivation references (which put a path in
# the input map AND in the environment), the list-join separator rule that
# ENG-12527 was about, `__ignoreNulls`, `outputHash` fixed-output derivations
# (a different hashing path entirely), and names with the awkward characters a
# store path is allowed to carry.
#
# `builtins.derivationStrict` and not `derivation`: the latter is a Nix-level
# wrapper around it that adds `.drvPath`, `.outPath` and the output attributes,
# which is what the arms then select -- so `derivation` is used where the case
# needs those, and `derivationStrict` where the case is about the primop.
# ---------------------------------------------------------------------------
declare -a CASES=(
  "derivation { $D }"
  "derivation { $D outputs = [ \"dev\" \"lib\" \"out\" ]; }"
  "derivation { $D outputs = [ \"bin\" ]; }"
  "derivation { $D args = [ \"-c\" \"echo hi\" ]; }"
  "derivation { $D n = 42; f = 1.5; t = true; e = false; l = [ \"a\" \"b\" ]; }"
  # A list holding an empty list: cppnix drops the separator after it, so the
  # coerced bytes -- and therefore the outPath -- differ if the join rule is
  # wrong (ENG-12527).
  "derivation { $D l = [ \"a\" [ ] \"b\" ]; }"
  "derivation { $D l = [ [ ] \"b\" ]; }"
  "derivation { $D __ignoreNulls = true; skipped = null; kept = \"x\"; }"
  "derivation { $D kept = null; }"
  "derivation { name = \"a-b.c_d+e?f=g\"; system = \"$SYSTEM\"; builder = \"/bin/sh\"; }"
  # A reference to another derivation: its drvPath enters the input map and
  # its outPath enters the environment, so this is the case where the string
  # context has to be threaded correctly to get either right.
  "let a = derivation { name = \"a\"; system = \"$SYSTEM\"; builder = \"/bin/sh\"; }; in derivation { $D dep = a; }"
  "let a = derivation { name = \"a\"; system = \"$SYSTEM\"; builder = \"/bin/sh\"; outputs = [ \"dev\" \"out\" ]; }; in derivation { $D d1 = a.dev; d2 = a.out; }"
  "let a = derivation { name = \"a\"; system = \"$SYSTEM\"; builder = \"/bin/sh\"; }; in let b = derivation { name = \"b\"; system = \"$SYSTEM\"; builder = \"/bin/sh\"; dep = a; }; in derivation { $D dep = b; args = [ \"-x\" \"\${a}\" ]; }"
  # Fixed-output: a different store-path computation (makeFixedOutputPath, not
  # the input-addressed one), so agreeing on the cases above says nothing
  # about agreeing here.
  "derivation { $D outputHashMode = \"flat\"; outputHashAlgo = \"sha256\"; outputHash = \"0000000000000000000000000000000000000000000000000000000000000000\"; }"
  "derivation { $D outputHashMode = \"recursive\"; outputHashAlgo = \"sha256\"; outputHash = \"0000000000000000000000000000000000000000000000000000000000000000\"; }"
  "derivation { $D outputHashAlgo = \"sha1\"; outputHashMode = \"flat\"; outputHash = \"0000000000000000000000000000000000000000\"; }"
  # builtins.toFile in the mix: a second store write, whose path is then part
  # of the derivation's environment and input sources.
  "derivation { $D conf = builtins.toFile \"conf\" \"key = value\"; }"
  "let f = builtins.toFile \"s\" \"contents\"; in derivation { $D a = f; b = f; }"
  "derivation { $D env = builtins.toFile \"e\" \"\${builtins.toFile \"inner\" \"x\"}\"; }"
  # The two settings this gate hands the driver on the command line, actually
  # consulted. Every case above writes `system` as a literal, so nothing read
  # `builtins.currentSystem` and nothing read `builtins.nixVersion` -- which
  # meant a driver told the wrong `--system` produced identical bytes and the
  # gate passed. That was measured, not imagined: the break test for the
  # drvPath arm mangled `--system`, all 19 cases still matched, and the gate
  # exited 0. Two cases where the value reaches the `.drv` close it.
  "derivation { name = \"g\"; system = builtins.currentSystem; builder = \"/bin/sh\"; }"
  "derivation { $D marker = builtins.nixVersion; }"
)

n=0; match=0; mismatch=0; unimpl=0; ratchet_msg=""

# sha256 of a `.drv`, or a marker when the arm left nothing there. `<absent>`
# and not an empty string, so a missing file can never compare equal to
# another missing file by accident.
drv_bytes() { # PATH
  if [ -f "$1" ]; then
    shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1
  else
    echo "<absent>"
  fi
}

for e in "${CASES[@]}"; do
  n=$((n + 1))
  RD=$W/case-$n/driver; RC=$W/case-$n/cpp; RR=$W/case-$n/rust
  scrub "$W/case-$n"
  mkdir -p "$RD" "$RC" "$RR"

  # The driver. `--store-dir` is the *hashed* directory and `--store-root` the
  # real one, which is the whole reason three arms can compute one path and
  # write three files.
  d_out=$("$DRIVER" instantiate \
    --store-dir "$STORE_DIR" --store-root "$RD" \
    --system "$SYSTEM" --nix-version "$NIXVER" --quiet \
    -E "$e" 2>"$W/driver.err")
  d_rc=$?

  c_out=$(NIX_CONFIG="$CPP" "$NIX" eval --raw --impure --store "local?root=$RC" \
    --expr "($e).drvPath" 2>"$W/cpp.err")
  c_rc=$?
  r_out=$(NIX_CONFIG="$RUST" "$NIX" eval --raw --impure --store "local?root=$RR" \
    --expr "($e).drvPath" 2>"$W/rust.err")
  r_rc=$?

  # A driver refusal is exit 2 and is a named gap, never a wrong answer. It is
  # counted separately and capped by the ratchet, so gaps cannot quietly grow
  # into the whole corpus.
  if [ $d_rc -eq 2 ]; then
    unimpl=$((unimpl + 1))
    echo "case $n unimplemented: $(head -1 "$W/driver.err")"
    continue
  fi
  if [ $d_rc -ne 0 ] || [ $c_rc -ne 0 ] || [ $r_rc -ne 0 ]; then
    mismatch=$((mismatch + 1))
    echo "case $n MISMATCH exit driver=$d_rc cpp=$c_rc rust=$r_rc"
    echo "  expr: $e"
    echo "  driver: $(head -1 "$W/driver.err")"
    echo "  cpp:    $(head -1 "$W/cpp.err")"
    echo "  rust:   $(head -1 "$W/rust.err")"
    continue
  fi

  bad=""

  # 1. drvPath.
  if [ "$d_out" != "$c_out" ] || [ "$d_out" != "$r_out" ]; then
    bad="$bad drvPath(driver=$d_out cpp=$c_out rust=$r_out)"
  fi

  # 2. outPath. A separate question from the drvPath: two derivations can
  # agree on where the `.drv` goes and disagree about where the build lands,
  # because the output path is computed from the drv's own hash modulo its
  # references.
  # Exit codes checked and stderr kept, both deliberately. With `2>/dev/null`
  # and no exit-code test, three arms that ALL failed left three empty
  # strings, both comparisons held, and the case scored a match having
  # measured nothing -- "an assertion whose passing state is an absence",
  # which CLAUDE.md names as a failure mode and this gate's own header claims
  # to have closed. Found in review.
  do_out=$("$DRIVER" eval --store-dir "$STORE_DIR" --system "$SYSTEM" \
    --nix-version "$NIXVER" --quiet -E "($e).outPath" 2>"$W/driver-out.err")
  do_rc=$?
  co_out=$(NIX_CONFIG="$CPP" "$NIX" eval --raw --impure --store "local?root=$RC" \
    --expr "($e).outPath" 2>"$W/cpp-out.err")
  co_rc=$?
  ro_out=$(NIX_CONFIG="$RUST" "$NIX" eval --raw --impure --store "local?root=$RR" \
    --expr "($e).outPath" 2>"$W/rust-out.err")
  ro_rc=$?
  for pair in "driver:$do_rc:$W/driver-out.err" "cpp:$co_rc:$W/cpp-out.err" "rust:$ro_rc:$W/rust-out.err"; do
    arm=${pair%%:*}; rest=${pair#*:}; armrc=${rest%%:*}; errf=${rest#*:}
    [ "$armrc" -eq 0 ] || bad="$bad outPath-failed($arm rc=$armrc: $(head -1 "$errf"))"
  done
  # The driver's `eval` quotes its strings, the bridge's `--raw` does not.
  do_out=${do_out%\"}; do_out=${do_out#\"}
  if [ "$do_out" != "$co_out" ] || [ "$do_out" != "$ro_out" ]; then
    bad="$bad outPath(driver=$do_out cpp=$co_out rust=$ro_out)"
  fi

  # 3. The `.drv` itself: present, and byte for byte the same in all three
  # roots. This is the check the eval arms of every other gate cannot make,
  # because they only ever compare printed paths (ENG-12799).
  # Each arm is looked for at the path IT printed, not at the driver's. When
  # the paths agree those are the same file name and it makes no difference;
  # when they do not, keying all three off the driver's path reports the other
  # two as `no-drv-written` when they wrote perfectly well, which points the
  # reader at the wrong arm. Seen while break-testing this gate.
  fd=$RD$STORE_DIR/${d_out#"$STORE_DIR"/}
  fc=$RC$STORE_DIR/${c_out#"$STORE_DIR"/}
  fr=$RR$STORE_DIR/${r_out#"$STORE_DIR"/}
  for pair in "driver:$fd" "cpp:$fc" "rust:$fr"; do
    [ -f "${pair#*:}" ] || bad="$bad no-drv-written(${pair%%:*})"
  done
  hd=$(drv_bytes "$fd"); hc=$(drv_bytes "$fc"); hr=$(drv_bytes "$fr")
  if [ "$hd" != "$hc" ] || [ "$hd" != "$hr" ]; then
    bad="$bad drv-bytes(driver=$hd cpp=$hc rust=$hr)"
  fi

  if [ -n "$bad" ]; then
    mismatch=$((mismatch + 1))
    echo "case $n MISMATCH:$bad"
    echo "  expr: $e"
    # The first differing line, which is far more use than three hashes when
    # the divergence is one field of the ATerm.
    if [ -f "$fd" ] && [ -f "$fc" ]; then
      echo "  diff driver/cpp: $(diff <(tr ',' '\n' < "$fd") <(tr ',' '\n' < "$fc") | head -4 | tr '\n' ' ')"
    fi
  else
    match=$((match + 1))
  fi
done

# The ratchet checks run BEFORE the verdict is decided, so `verdict` reflects
# everything the gate exits non-zero for. Deriving it from `mismatch` alone
# printed `RESULT rust-driver-parity pass cases=0 ...` for an emptied corpus
# and then exited 1 from the ratchet below -- and a log scraper keying on the
# RESULT line reads the word, not the exit code. Found in review.
rc=0
if [ "$mismatch" -ne 0 ]; then
  ratchet_msg="$ratchet_msg
rust-driver-parity: $mismatch case(s) diverged. Tier 1 has no allowlist: a differing drvPath, outPath or .drv byte is a defect to fix (CLAUDE.md, 'Parity bar')."
  rc=1
fi
if [ "$n" -ne "$RUST_DRIVER_PARITY_CASES" ]; then
  ratchet_msg="$ratchet_msg
rust-driver-parity: ran $n cases, gate-ratchets.sh says $RUST_DRIVER_PARITY_CASES. Move RUST_DRIVER_PARITY_CASES in the same commit that changes the CASES array, so a list that shrank by accident cannot read as a pass."
  rc=1
fi
if [ "$match" -lt "$RUST_DRIVER_PARITY_MIN_MATCH" ]; then
  ratchet_msg="$ratchet_msg
rust-driver-parity: $match cases matched, the floor is $RUST_DRIVER_PARITY_MIN_MATCH. A run where cases turned into refusals keeps mismatch at 0 while proving nothing."
  rc=1
fi
if [ "$unimpl" -gt "$RUST_DRIVER_PARITY_MAX_UNIMPLEMENTED" ]; then
  ratchet_msg="$ratchet_msg
rust-driver-parity: $unimpl cases were refused, the ceiling is $RUST_DRIVER_PARITY_MAX_UNIMPLEMENTED. A gap that grows is a regression even though no byte differs."
  rc=1
fi

verdict=pass
[ "$rc" -eq 0 ] || verdict=fail

echo "RESULT rust-driver-parity $verdict cases=$n match=$match mismatch=$mismatch unimplemented=$unimpl \
expected-cases=$RUST_DRIVER_PARITY_CASES min-match=$RUST_DRIVER_PARITY_MIN_MATCH \
max-unimplemented=$RUST_DRIVER_PARITY_MAX_UNIMPLEMENTED \
driver=$DRIVER driver-sha256=$(shasum -a 256 "$DRIVER" | cut -d' ' -f1) \
bin=$NIX bin-sha256=$(shasum -a 256 "$NIX" | cut -d' ' -f1) version='$NIXVER' \
ratchets-from=$RUST_DRIVER_PARITY_MEASURED_AT@$RUST_DRIVER_PARITY_MEASURED_ON"

[ -z "$ratchet_msg" ] || echo "$ratchet_msg"
exit $rc
