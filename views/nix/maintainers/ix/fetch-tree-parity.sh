#!/usr/bin/env bash
# Cross-backend parity for the tree fetchers, builtins.fetchTree and
# builtins.fetchGit. Same shape as fetch-parity.sh: one binary, two
# eval-backend settings, compared byte for byte on stdout, stderr and exit
# code, and tier 1 because the pairs below produce store paths.
#
# ## Hermetic, and the gate builds what it measures
#
# The fixture is a git repository this script creates, with the author and
# committer dates pinned, so `rev`, `revCount`, `lastModified` and the store
# path are all fixed values rather than whatever the clock said. No network,
# no registry, no remote: every URL is a local path.
#
# ## What this gate is FOR, over and above the store path
#
# The answer to a tree fetch is an attribute set, and every attribute in it can
# be read by a lock file. So the cases below ask for the attributes one at a
# time -- `rev`, `shortRev`, `revCount`, `lastModified`, `lastModifiedDate`,
# `narHash`, `submodules` -- and for `attrNames`. Comparing only `outPath`
# would pass while `revCount` was off by one.
#
# `--json` on the whole set is deliberately NOT used: printValueAsJSON collapses
# an attrset carrying `outPath` to that string alone, so a whole-set comparison
# would silently be an outPath comparison.
#
# ## Refusals are expected here, and counted exactly
#
# Unlike fetch-parity.sh, this backend refuses two shapes by name: a bare
# string or path argument (cppnix routes it through Input::fromURL or
# fixGitURL, which is URL parsing the evaluator does not do) and `publicKeys`
# (which cppnix renders with printValueAsJSON into an input attribute). Those
# are named gaps, not wrong answers, and the count is exact so that a new one
# cannot appear quietly and closing one cannot pass unnoticed.
#
# Needs a built nix with the Rust evaluator linked in; see fetch-parity.sh.
# Run it inside `nix develop` (python3, git).
set -u
command -v python3 > /dev/null || {
  echo "fetch-tree-parity: no python3, so the capability probe cannot read NIX_SHOW_STATS. Run this inside 'nix develop'."
  exit 2
}
command -v git > /dev/null || { echo "fetch-tree-parity: no git"; exit 2; }

BUILD=${NIX_BUILD_DIR:-$PWD/build-rust}
NIXI=$BUILD/src/nix/nix-instantiate
[ -x "$NIXI" ] || { echo "no nix-instantiate at $NIXI"; exit 2; }

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=./gate-ratchets.sh
. "$here/gate-ratchets.sh" || exit 2
# shellcheck source=./error-class.sh
. "$here/error-class.sh" || exit 2

BASE="extra-experimental-features = rust-eval flakes nix-command
substituters =
${EXTRA_NIX_CONFIG:-}"
CPP="$BASE
eval-backend = cpp"
RUST="$BASE
eval-backend = rust"

W=$(mktemp -d); trap 'rm -rf "$W"' EXIT

for arm in CPP RUST; do
  case $arm in CPP) cfg=$CPP ;; *) cfg=$RUST ;; esac
  got=$(NIX_CONFIG="$cfg" "$NIXI" --eval --strict -E 1 2>&1)
  [ "$got" = 1 ] || {
    echo "fetch-tree-parity: the $arm arm cannot evaluate the probe expression '1':"
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
    echo "fetch-tree-parity: the $arm arm asked for '$want', NIX_SHOW_STATS reports '$ev'; the two arms would be the same backend and every comparison below would be vacuous"
    exit 2
  }
  echo "probe: $arm arm evaluates, NIX_SHOW_STATS confirms the '$ev' backend ran"
done

# -- the fixture -------------------------------------------------------------
# Dates pinned so every derived attribute is a fixed value. A repository whose
# lastModified was `now` would make this gate's expectations un-writable and
# its failures un-reproducible.
R=$W/repo
mkdir -p "$R/sub"
( cd "$R" \
  && git init -q -b main . \
  && printf 'one\n' > a.txt \
  && printf 'nested\n' > sub/b.txt \
  && git add -A \
  && GIT_AUTHOR_DATE="2020-01-01T00:00:00Z" GIT_COMMITTER_DATE="2020-01-01T00:00:00Z" \
     git -c user.email=t@t -c user.name=t commit -q -m one ) || {
  echo "fetch-tree-parity: could not build the git fixture"; exit 2; }
REV=$(git -C "$R" rev-parse HEAD) || exit 2
case "$REV" in ????????????????????????????????????????) ;; *)
  echo "fetch-tree-parity: the fixture revision is not a sha1: $REV"; exit 2 ;; esac
echo "fixture: $R at $REV"

# A second, dirty worktree. cppnix's fetchGit on a dirty local repo takes a
# different branch -- an empty-sha1 `rev`, `revCount` 0, and `dirtyRev` -- and
# that branch is where a wrong answer would be least visible, so it gets its
# own copy rather than dirtying the pinned one.
D=$W/dirty
if ! cp -r "$R" "$D" || ! printf 'two\n' >> "$D/a.txt"; then
  echo "fetch-tree-parity: could not build the dirty fixture"; exit 2
fi
echo "fixture: $D (dirty worktree)"

G="builtins.fetchGit { url = \"$R\"; rev = \"$REV\"; }"
T="builtins.fetchTree { type = \"git\"; url = \"$R\"; rev = \"$REV\"; }"
P="builtins.fetchTree { type = \"path\"; path = \"$R\"; }"
DIRTY="builtins.fetchGit { url = \"$D\"; }"

declare -a CASES=(
  # -- every attribute of a pinned fetchGit, one at a time ------------------
  "($G).outPath"
  "($G).rev"
  "($G).shortRev"
  "($G).revCount"
  "($G).lastModified"
  "($G).lastModifiedDate"
  "($G).narHash"
  "($G).submodules"
  "builtins.attrNames ($G)"
  # -- the same tree through fetchTree, which must land in the same place ---
  "($T).outPath"
  "($T).rev"
  "($T).narHash"
  "builtins.attrNames ($T)"
  "($G).outPath == ($T).outPath"
  # -- a path input ----------------------------------------------------------
  "($P).outPath"
  "builtins.attrNames ($P)"
  # -- a branch rather than a revision --------------------------------------
  "(builtins.fetchGit { url = \"$R\"; ref = \"main\"; }).rev"
  "(builtins.fetchGit { url = \"$R\"; ref = \"main\"; }).outPath"
  # -- the dirty worktree, where a wrong answer would be least visible ------
  "builtins.attrNames ($DIRTY)"
  "($DIRTY).rev"
  "($DIRTY).revCount"
  "($DIRTY).dirtyShortRev"
  "($DIRTY).outPath"
  # -- context and a derivation ---------------------------------------------
  "builtins.getContext ($G).outPath"
  "builtins.getContext ($G).narHash"
  "(builtins.derivationStrict { name = \"g\"; system = \"x86_64-linux\"; builder = \"/bin/sh\"; src = ($G).outPath; }).out"
  # -- shape errors, all raised before any IO --------------------------------
  "builtins.fetchTree { url = \"$R\"; }"
  "builtins.fetchGit { url = \"$R\"; type = \"git\"; }"
  "builtins.fetchTree { type = \"path\"; path = \"$R\"; name = \"n\"; }"
  "builtins.fetchGit { url = \"$R\"; revCount = -1; }"
  "builtins.fetchTree { type = \"path\"; path = \"$R\"; extra = [ ]; }"
  "builtins.fetchTree { type = \"path\"; path = \"$R\"; extra = null; }"
  "builtins.fetchTree { }"
  # -- the shapes this backend refuses by name -------------------------------
  # Expected `unimplemented`, counted exactly below. cppnix serves all three.
  "(builtins.fetchGit \"$R\").rev"
  "builtins.typeOf (builtins.fetchTree \"path:$R\")"
  "builtins.fetchTree { type = \"git\"; url = \"$R\"; publicKeys = [ { key = \"k\"; } ]; }"
)

match=0; mismatch=0; unimpl=0; bothfail=0; produced=0; statusdiff=0; n=0
for e in "${CASES[@]}"; do
  n=$((n+1))
  co=$(NIX_CONFIG="$CPP" "$NIXI" --eval --strict -E "$e" 2>&1); crc=$?
  ro=$(NIX_CONFIG="$RUST" "$NIXI" --eval --strict -E "$e" 2>&1); rrc=$?
  detail=
  if [ "$crc" = "$rrc" ] && [ "$co" = "$ro" ]; then
    match=$((match+1)); verdict=match
    # An answer, not an agreement about a failure. A store path OR any other
    # produced value counts here, because for this gate the revision and the
    # revCount matter as much as the path.
    [ "$crc" = 0 ] && produced=$((produced+1))
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
    [ "$crc" != "$rrc" ] && { statusdiff=$((statusdiff+1)); detail="$detail; exit status cpp=$crc rust=$rrc"; }
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
produced=$produced status-differs=$statusdiff expected-cases=$FETCH_TREE_PARITY_CASES \
expected-produced=$FETCH_TREE_PARITY_PRODUCED expected-unimplemented=$FETCH_TREE_PARITY_REFUSED \
ratchets-from=$GATE_RATCHETS_MEASURED_AT@$GATE_RATCHETS_MEASURED_ON"
echo "binary=$NIXI sha256=$(sha256sum "$NIXI" | cut -d" " -f1) version=$("$NIXI" --version | head -1)"

ok=1
if [ "$mismatch" != 0 ]; then
  echo "fetch-tree-parity: $mismatch case(s) diverged"
  ok=0
fi
if [ "$n" != "$FETCH_TREE_PARITY_CASES" ]; then
  echo "fetch-tree-parity: ran $n cases, gate-ratchets.sh says $FETCH_TREE_PARITY_CASES. Update FETCH_TREE_PARITY_CASES in the same commit that changes the CASES array."
  ok=0
fi
# The number two identically-failing arms cannot satisfy.
if [ "$produced" != "$FETCH_TREE_PARITY_PRODUCED" ]; then
  echo "fetch-tree-parity: produced=$produced, gate-ratchets.sh says $FETCH_TREE_PARITY_PRODUCED. This is the only number on the RESULT line that says a value was actually computed; every other one is satisfied by two arms that failed the same way."
  ok=0
fi
# Exact, in both directions: a new refusal is coverage leaving the gate, and a
# refusal that got fixed should make this fail until the number is updated.
if [ "$unimpl" != "$FETCH_TREE_PARITY_REFUSED" ]; then
  echo "fetch-tree-parity: unimplemented=$unimpl, gate-ratchets.sh says $FETCH_TREE_PARITY_REFUSED. The named refusals are the bare-string argument and 'publicKeys'; anything else refusing is coverage leaving the gate, and a refusal that started working should move this number deliberately."
  ok=0
fi
if [ "$statusdiff" != 0 ]; then
  echo "fetch-tree-parity: $statusdiff case(s) failed alike but exited differently; ENG-12719 is the known fetchurl set and does not cover these."
  ok=0
fi
[ "$ok" = 1 ] || exit 1
echo "fetch-tree-parity: OK"
