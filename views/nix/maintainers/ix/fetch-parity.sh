#!/usr/bin/env bash
# Cross-backend parity for the fixed-output fetchers, builtins.fetchurl and
# builtins.fetchTarball. Every expression below is evaluated twice through ONE
# binary, once with eval-backend=cpp and once with rust, and the two are
# compared byte for byte on stdout, stderr and exit code.
#
# Differential and not golden, for the reason drv-parity.sh gives.
#
# Tier 1, and it stays there (CLAUDE.md, "Parity bar"): the pinned cases below
# produce a store path, and for a store path byte identity IS functional
# identity. There is no allowlist here.
#
# ## Hermetic on purpose, and the gate enforces it rather than hoping
#
# A fetch gate that downloads is a network test wearing a parity test's
# clothes: it goes red when a mirror is slow and green when the two arms agree
# that the network is down. So:
#
#   * `substituters =` is empty in BOTH arms, so a pinned path can only be
#     answered out of the LOCAL store -- which is exactly cppnix's early exit
#     (fetchTree.cc:540), the branch that makes evaluation in CI hermetic and
#     the one this gate exists to hold;
#   * the fixture is inserted into that store here, with `nix-store
#     --add-fixed` and `nix store add-path`, so the sha256 the cases pin is a
#     hash of bytes this script wrote;
#   * every URL that is fetched for real is a `file://` URL under $W.
#
# Nothing below reaches the network. A run with the cable pulled must produce
# the same RESULT line, and if it does not, this comment is wrong.
#
# ## What it refuses to be satisfied by
#
# `produced` counts the pairs where BOTH arms printed a /nix/store path. It has
# a checked-in exact value, because every other number here is satisfied by two
# arms that failed identically: a broken fixture, a store that lost the paths
# or a backend that refuses everything would score mismatch=0 and exit 0 with
# nothing measured. That is the vacuous pass this gate is shaped around.
#
# Needs a built nix with the Rust evaluator linked in, which the default build
# is not (`rust-eval` defaults to `disabled`):
#
#   nix develop --command nix shell nixpkgs#cargo nixpkgs#rustc --command bash -c \
#     'meson setup build-rust --prefix="$out" -Dnix:rust-eval=enabled && ninja -C build-rust'
#
# Point it at one with NIX_BUILD_DIR; the default is ./build-rust.
#
# Run it inside `nix develop`: the capability probe reads NIX_SHOW_STATS with
# python3, which is not on a dev-compute node's bare PATH. Outside the shell
# the probe cannot tell the two backends apart, and it exits 2 rather than
# comparing an arm against itself.
set -u
command -v python3 > /dev/null || {
  echo "fetch-parity: no python3, so the capability probe cannot read NIX_SHOW_STATS and cannot tell the two arms apart. Run this inside 'nix develop'."
  exit 2
}
BUILD=${NIX_BUILD_DIR:-$PWD/build-rust}
NIXI=$BUILD/src/nix/nix-instantiate
[ -x "$NIXI" ] || { echo "no nix-instantiate at $NIXI"; exit 2; }

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=./gate-ratchets.sh
. "$here/gate-ratchets.sh" || exit 2
# shellcheck source=./error-class.sh
. "$here/error-class.sh" || exit 2

BASE="extra-experimental-features = rust-eval
substituters =
${EXTRA_NIX_CONFIG:-}"
CPP="$BASE
eval-backend = cpp"
RUST="$BASE
eval-backend = rust"

W=$(mktemp -d); trap 'rm -rf "$W"' EXIT

# Capability probe, both arms, same shape as drv-parity.sh: `nix config show`
# reports eval-backend = rust on a binary compiled without the Rust evaluator,
# and every case below would then refuse and exit 0.
for arm in CPP RUST; do
  case $arm in CPP) cfg=$CPP ;; *) cfg=$RUST ;; esac
  got=$(NIX_CONFIG="$cfg" "$NIXI" --eval --strict -E 1 2>&1)
  [ "$got" = 1 ] || {
    echo "fetch-parity: the $arm arm cannot evaluate the probe expression '1'; nothing below would mean anything:"
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
    echo "fetch-parity: the $arm arm asked for the '$want' evaluator, NIX_SHOW_STATS reports '$ev'; the two arms would be the same backend and every comparison below would be vacuous"
    exit 2
  }
  echo "probe: $arm arm evaluates, NIX_SHOW_STATS confirms the '$ev' backend ran"
done

# -- the fixture -------------------------------------------------------------
# A file and a tree, hashed and inserted, so the pinned cases below name store
# paths this store demonstrably holds. Every step is checked: a fixture that
# silently failed to insert turns the whole gate into a download test.
mkdir -p "$W/tree/sub"
printf 'hermetic fetch fixture\n' > "$W/hello-1.0.tar.gz"
printf 'a\n' > "$W/tree/a.txt"
printf 'b\n' > "$W/tree/sub/b.txt"

FLAT_PATH=$(nix-store --add-fixed sha256 "$W/hello-1.0.tar.gz") || {
  echo "fetch-parity: could not insert the flat fixture into the store"; exit 2; }
FLAT_SRI=$(nix hash file --type sha256 --sri "$W/hello-1.0.tar.gz") || exit 2
NAR_PATH=$(nix store add-path --name source "$W/tree") || {
  echo "fetch-parity: could not insert the tree fixture into the store"; exit 2; }
NAR_SRI=$(nix hash path --type sha256 --sri "$W/tree") || exit 2

case "$FLAT_PATH" in /nix/store/*-hello-1.0.tar.gz) ;; *)
  echo "fetch-parity: the flat fixture landed at an unexpected path: $FLAT_PATH"; exit 2 ;; esac
case "$NAR_PATH" in /nix/store/*-source) ;; *)
  echo "fetch-parity: the tree fixture landed at an unexpected path: $NAR_PATH"; exit 2 ;; esac
echo "fixture: $FLAT_PATH $FLAT_SRI"
echo "fixture: $NAR_PATH $NAR_SRI"

# A URL nothing can serve. The pinned cases use it deliberately: if either arm
# answers, it answered from the store without downloading, which is the whole
# point. If either arm tries to fetch it, the case fails loudly rather than
# quietly taking a slow path.
ABSENT="file://$W/definitely-not-here.tar.gz"
LOCAL="file://$W/hello-1.0.tar.gz"
ZERO="sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="

declare -a CASES=(
  # -- the hermetic pinned path, which is the reason this gate exists --------
  "builtins.fetchurl { url = \"$ABSENT\"; sha256 = \"$FLAT_SRI\"; name = \"hello-1.0.tar.gz\"; }"
  "builtins.fetchurl { url = \"$LOCAL\"; sha256 = \"$FLAT_SRI\"; }"
  "builtins.fetchTarball { url = \"$ABSENT\"; sha256 = \"$NAR_SRI\"; }"
  "builtins.fetchTarball { url = \"$ABSENT\"; sha256 = \"$NAR_SRI\"; name = \"source\"; }"
  # The result is a string, not a path, and it carries the fetched path as its
  # own context and nothing else. Both are visible to a program.
  "builtins.typeOf (builtins.fetchurl { url = \"$ABSENT\"; sha256 = \"$FLAT_SRI\"; name = \"hello-1.0.tar.gz\"; })"
  "builtins.getContext (builtins.fetchurl { url = \"$ABSENT\"; sha256 = \"$FLAT_SRI\"; name = \"hello-1.0.tar.gz\"; })"
  "builtins.getContext (builtins.fetchTarball { url = \"$ABSENT\"; sha256 = \"$NAR_SRI\"; })"
  # A fetched path feeding a derivation: the outPath below is a hash OF the
  # fetched store path, so this is the tier-1 case that a wrong fetch answer
  # cannot survive.
  "(builtins.derivationStrict { name = \"g\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; src = builtins.fetchurl { url = \"$ABSENT\"; sha256 = \"$FLAT_SRI\"; name = \"hello-1.0.tar.gz\"; }; }).out"
  "(builtins.derivationStrict { name = \"g\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; src = builtins.fetchTarball { url = \"$ABSENT\"; sha256 = \"$NAR_SRI\"; }; }).drvPath"
  # -- the name rules, which decide the store path --------------------------
  # fetchurl defaults its name to the URL's base name and fetchTarball to
  # "source"; both are hashed into the path, so an unpinned-but-well-formed
  # name is still a real difference. These two fail (nothing to download) and
  # must fail alike.
  "builtins.fetchurl \"$ABSENT\""
  "builtins.fetchTarball \"$ABSENT\""
  # -- errors raised before any IO ------------------------------------------
  "builtins.fetchurl { url = \"$LOCAL\"; rev = \"abc\"; }"
  "builtins.fetchTarball { url = \"$LOCAL\"; rev = \"abc\"; }"
  "builtins.fetchurl { }"
  "builtins.fetchTarball { }"
  "builtins.fetchurl \"file:///tmp/a b\""
  "builtins.fetchurl { url = \"file:///tmp/a b\"; }"
  "builtins.fetchurl { url = \"$LOCAL\"; name = \"no spaces here\"; }"
  "builtins.fetchurl { url = \"$LOCAL\"; name = \"\"; }"
  "builtins.fetchurl { url = \"$LOCAL\"; name = \"..-x\"; }"
  "builtins.fetchurl 1"
  "builtins.fetchTarball [ ]"
  "builtins.fetchurl { url = 1; }"
  "builtins.fetchurl { url = \"$LOCAL\"; sha256 = \"not-a-hash\"; }"
  # `channel:` is rewritten by fetchTarball only, and an unrewritten one has a
  # colon in its base name, which checkName rejects. Two cases, one rule.
  "builtins.fetchurl \"channel:nixos-24.05\""
  "builtins.fetchTarball { url = \"channel:nixos-24.05\"; sha256 = \"$NAR_SRI\"; }"
  # -- the hash check, over a local file so it stays hermetic ---------------
  "builtins.fetchurl { url = \"$LOCAL\"; sha256 = \"$ZERO\"; name = \"hello-1.0.tar.gz\"; }"
  # An empty sha256 is the all-zero hash plus a warning on stderr, which this
  # harness captures -- so a backend that dropped the warning diverges here
  # even though it fails the same way.
  "builtins.fetchurl { url = \"$LOCAL\"; sha256 = \"\"; name = \"hello-1.0.tar.gz\"; }"
  # -- lazy, and tryEval ----------------------------------------------------
  # The fetch never runs, so neither arm may touch the store.
  "builtins.typeOf (x: builtins.fetchurl x)"
  "builtins.length [ (builtins.fetchurl \"$ABSENT\") ]"
  # cppnix's prim_tryEval catches AssertionError only, so a fetch failure is
  # NOT caught and kills the evaluation. A backend that turned it into
  # { success = false; } would be quietly wrong.
  "(builtins.tryEval (builtins.fetchurl { url = \"$LOCAL\"; sha256 = \"$ZERO\"; name = \"hello-1.0.tar.gz\"; })).success"
  "(builtins.tryEval (builtins.fetchurl { })).success"
)

match=0; mismatch=0; unimpl=0; bothfail=0; produced=0; statusdiff=0; n=0
for e in "${CASES[@]}"; do
  n=$((n+1))
  co=$(NIX_CONFIG="$CPP" "$NIXI" --eval --strict -E "$e" 2>&1); crc=$?
  ro=$(NIX_CONFIG="$RUST" "$NIXI" --eval --strict -E "$e" 2>&1); rrc=$?
  detail=
  if [ "$crc" = "$rrc" ] && [ "$co" = "$ro" ]; then
    match=$((match+1)); verdict=match
    # Counted only when BOTH arms produced a store path. Two identical
    # failures are agreement about a failure, which is not what this gate is
    # for.
    if [ "$crc" = 0 ] && printf "%s" "$co" | grep -q "/nix/store/"; then
      produced=$((produced+1))
    fi
  elif printf "%s" "$ro" | grep -qiE "unimplemented|does not implement|rust-eval unimplemented"; then
    unimpl=$((unimpl+1)); verdict=unimplemented
  elif [ "$crc" != 0 ] && [ "$rrc" != 0 ]; then
    printf "%s" "$co" > "$W/co.err"; printf "%s" "$ro" > "$W/ro.err"
    class_c=$(error_class "$W/co.err"); class_r=$(error_class "$W/ro.err")
    detail="class cpp=$class_c rust=$class_r"
    if [ "$class_c" = "$class_r" ] && [ "$class_c" != unknown ]; then
      bothfail=$((bothfail+1)); verdict=both-fail-alike
    elif [ "$class_c" = unknown ] && [ "$class_r" = unknown ] \
         && [ -n "$(last_error "$W/co.err")" ] \
         && [ "$(last_error "$W/co.err")" = "$(last_error "$W/ro.err")" ]; then
      bothfail=$((bothfail+1)); verdict=both-fail-alike
    else
      mismatch=$((mismatch+1)); verdict=MISMATCH
    fi
    # The both-fail bucket compares the error CLASS, so it is blind to the
    # exit status -- and cppnix carries a meaningful one: a fixed-output hash
    # mismatch is `.withExitStatus(102)`, which scripts branch on. The Rust
    # arm's fetch hook has no channel for it and exits 1. Counted rather than
    # tolerated silently, so the number can only move deliberately (ENG-12719).
    if [ "$crc" != "$rrc" ]; then
      statusdiff=$((statusdiff+1))
      detail="$detail; exit status cpp=$crc rust=$rrc"
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
produced=$produced status-differs=$statusdiff expected-cases=$FETCH_PARITY_CASES \
expected-produced=$FETCH_PARITY_PRODUCED expected-status-differs=$FETCH_PARITY_STATUS_DIFFERS \
ratchets-from=$GATE_RATCHETS_MEASURED_AT@$GATE_RATCHETS_MEASURED_ON"
echo "binary=$NIXI sha256=$(sha256sum "$NIXI" | cut -d" " -f1) version=$("$NIXI" --version | head -1)"

ok=1
if [ "$mismatch" != 0 ]; then
  echo "fetch-parity: $mismatch case(s) diverged"
  ok=0
fi
if [ "$n" != "$FETCH_PARITY_CASES" ]; then
  echo "fetch-parity: ran $n cases, gate-ratchets.sh says $FETCH_PARITY_CASES. Update FETCH_PARITY_CASES in the same commit that changes the CASES array."
  ok=0
fi
# The one number that cannot be satisfied by agreement about a failure.
if [ "$produced" != "$FETCH_PARITY_PRODUCED" ]; then
  echo "fetch-parity: produced=$produced, gate-ratchets.sh says $FETCH_PARITY_PRODUCED. Every other number on the RESULT line is satisfied by two arms that failed identically, so this is the one that says a store path was actually computed. A drop here means the fixture did not insert, the store lost it, or a backend stopped serving pinned fetches."
  ok=0
fi
# Exact, not a ceiling. This is a known gap with a ticket, and the number
# going UP means a new one appeared; going DOWN means it was fixed and this
# file should say so.
if [ "$statusdiff" != "$FETCH_PARITY_STATUS_DIFFERS" ]; then
  echo "fetch-parity: status-differs=$statusdiff, gate-ratchets.sh says $FETCH_PARITY_STATUS_DIFFERS. The two arms failed alike but exited differently. ENG-12719 covers the known set (cppnix's exit status 102 for a fixed-output hash mismatch, which the Rust arm reports as a plain error); anything beyond it is new."
  ok=0
fi
if [ "$unimpl" != 0 ]; then
  echo "fetch-parity: $unimpl case(s) refused by the rust arm. The fetchers are implemented, so a refusal here is coverage leaving the gate, and same() counts it as neither match nor mismatch."
  ok=0
fi
[ "$ok" = 1 ] || exit 1
echo "fetch-parity: OK"
