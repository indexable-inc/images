#!/usr/bin/env bash
# Cross-backend parity for flakes that HAVE INPUTS.
#
# `drv-parity.sh` proves the flake entry point works for a flake with no
# inputs. That flake exercises exactly one node of `call-flake.nix`: the root,
# which always carries an override, so `allNodes` never recurses,
# `fetchTreeFinal` is never called, `resolveInput` never sees a list and the
# `isRelative` branch never runs. `rust-flake-entry.md` named that as the
# largest untested area of the flake change; this is the gate that closes it.
#
# Every fixture is built from scratch in a scratch directory and reaches
# nothing but the local filesystem -- `path:`, `git+file://`,
# `tarball+file://` -- so the gate runs offline and cannot flake on someone
# else's server. What it therefore does NOT cover is `github:` proper, which
# cannot be fetched without a network; the tarball fixture's header says what
# stands in for it and what that substitution does and does not buy.
#
# Tier 1 where a store path is computed: drvPath, outPath and the bytes of
# the `.drv` itself are compared byte for byte with no allowlist, for the
# reason drv-parity.sh gives. Tier 2 for the rest, where two failures are
# compared by error class rather than by wording.
#
# ## Pre-locking is not a convenience, it selects the code path
#
# Each fixture is locked once, by cppnix, before either arm runs, and the
# lock file's hash is asserted unchanged afterwards. That is the single most
# important line in this file.
#
# `computeLocks` (`flake.cc`) populates `nodePaths` only for nodes it actually
# fetches. On the run that CREATES a lock file every node is fetched, so every
# node lands in `nodePaths`, so `flakeOverridesJSON` hands `call-flake.nix` an
# override for every node, `hasOverride` is true everywhere, and
# `fetchTreeFinal` is dead code. On a run against an up-to-date lock file the
# `!mustRefetch` branch keeps the child lazily and does NOT add it to
# `nodePaths`, so the override is absent and `call-flake.nix` calls
# `fetchTreeFinal` for that node.
#
# So an unlocked fixture and a locked one measure different halves of the
# program, and only the locked one reaches the `TreeFetcher::FinalTree`
# builtin this fork added. A gate that let each arm lock for itself would also
# be comparing the first arm's locking run against the second arm's lazy run,
# which is two programs rather than two backends.
#
# Needs a nix built with the Rust evaluator linked in, which the default build
# is not:
#
#   nix develop -c meson setup build -Dnix:rust-eval=enabled
#   nix develop -c ninja -C build src/nix/nix
#
# Point it at one with NIX_BUILD_DIR; the default is ./build-rust.
#
# ## Run it a second time with a memo store
#
# `EXTRA_NIX_CONFIG` is appended to both arms, so this whole comparison can be
# repeated under an additional setting and required to come out the same. The
# one worth repeating is the memo:
#
#   EXTRA_NIX_CONFIG="eval-cache-dir = $(mktemp -d)" ./flake-inputs-parity.sh
#
# Eight distinct flakes evaluate one identical `call-flake.nix` source here,
# differing only in the arguments applied to it, which is exactly the shape
# that breaks if the memo key does not carry those arguments -- one key, eight
# right answers, and the second flake served the first one's derivation.
# ENG-12915 put the applied arguments in the key; this corpus is a wider probe
# of that than the two-flake case in `drv-parity.sh`.
#
# Measured on this Mac after merging ix-patched at 242e89701: 40 of 40 both
# ways, with 125 objects written under the cache directory, so the memo was
# populated rather than inert.
#
# Exits non-zero unless all of these hold:
#
#   mismatch == 0
#   rows     == FLAKE_INPUTS_ROWS        (gate-ratchets.sh)
#   match    >= FLAKE_INPUTS_MIN_MATCH   (gate-ratchets.sh)
#   drv rows == FLAKE_INPUTS_DRV_ROWS, and every one of them wrote a `.drv`
#   every fixture reached the node shape it exists to reach (the shape assert)
#   every lock file is byte-identical before and after the comparison
set -u
BUILD=${NIX_BUILD_DIR:-$PWD/build-rust}
NIXBIN=$BUILD/src/nix/nix
[ -x "$NIXBIN" ] || { echo "no nix at $NIXBIN"; exit 2; }

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=./arm-config.sh
. "$here/arm-config.sh" || exit 2
# Before anything reads the environment: one owner of the gates' nix
# configuration, so an ambient `lint-url-literals = fatal` cannot make every
# rust arm refuse and every row score `unimplemented` (ENG-12996).
arm_pin_environment
arm_require_clean_config "$NIXBIN"
# shellcheck source=./gate-ratchets.sh
. "$here/gate-ratchets.sh" || exit 2
# shellcheck source=./error-class.sh
. "$here/error-class.sh" || exit 2
# shellcheck source=./compare-arms.sh
. "$here/compare-arms.sh" || exit 2

# The three parser lints pinned into both arms rather than inherited, the way
# lang-diff.sh and shadow-corpus.sh do it. `NIX_CONFIG` is applied on top of
# whatever conf files are in scope, so a machine carrying `lint-url-literals =
# fatal` in ~/.config/nix/nix.conf makes the rust arm refuse every evaluation
# by name -- it has no parser lint to honour a fatal setting with. That is a
# fact about the machine and it does not belong in the comparison. `warn` and
# not `ignore`, so a fixture that trips a lint still says so on both arms.
BASE="extra-experimental-features = rust-eval nix-command flakes
$(arm_base_config)${EXTRA_NIX_CONFIG:+
$EXTRA_NIX_CONFIG}"
CPP="$BASE
eval-backend = cpp"
RUST="$BASE
eval-backend = rust"

# `pwd -P`, not `mktemp -d`'s answer: the path fetcher refuses a flake whose
# path traverses a symlink, and on macOS `mktemp -d` returns something under
# `/var/folders` where `/var` is a link to `/private/var`. Both arms fail
# identically on that, which scores as `fail-both` -- an agreement about
# nothing. Resolve it once, here.
WRAW=$(mktemp -d)
W=$(cd "$WRAW" && pwd -P)
cleanup() { chmod -R u+w "$WRAW" 2>/dev/null; rm -rf "$WRAW"; }
trap cleanup EXIT
FIX="$W/fix"; mkdir -p "$FIX"

# ---------------------------------------------------------------- probes ---
# A setting is not a capability: `nix config show` reports `eval-backend =
# rust` on a binary compiled without the backend. Ask the binary, then ask the
# counter which backend served -- the first check passes on a binary where
# both arms are silently cpp.
probe_arm() { # ARM -> what that arm evaluated `1` to
  local cfg
  case $1 in cpp) cfg=$CPP ;; *) cfg=$RUST ;; esac
  NIX_CONFIG="$cfg" "$NIXBIN" eval --expr 1 2>&1
}
arms_probe probe_arm cpp rust
for arm in cpp rust; do
  case $arm in cpp) cfg=$CPP ;; *) cfg=$RUST ;; esac
  NIX_CONFIG="$cfg" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/stats-$arm.json" \
    "$NIXBIN" eval --expr 1 > /dev/null 2>&1
  ev=$(python3 -c 'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("evaluator", "<absent>"))
except OSError:
    print("<no stats file>")' "$W/stats-$arm.json")
  [ "$ev" = "$arm" ] || {
    echo "flake-inputs-parity: the $arm arm asked for the '$arm' evaluator, NIX_SHOW_STATS reports '$ev';"
    echo "  the two arms would be the same backend and every comparison below would be vacuous."
    exit 2
  }
  echo "probe: $arm arm evaluates, NIX_SHOW_STATS confirms the '$ev' backend ran"
done

# The system spelled out rather than `builtins.currentSystem`: a flake
# evaluates under pure-eval, where cppnix leaves `currentSystem` out of
# `builtins` entirely (`eval.cc:541`), so a fixture naming it fails on cpp and
# succeeded on rust until ENG-12886.
SYSTEM=$(NIX_CONFIG="$CPP" "$NIXBIN" eval --raw --impure --expr builtins.currentSystem 2>&1) || {
  echo "flake-inputs-parity: could not read builtins.currentSystem: $SYSTEM"; exit 2; }
# In every derivation name, so no fixture's `.drv` can already be in the store
# and "the rust arm wrote this file" is an assertion rather than a reading of
# what the machine happened to have.
FRESH="flakeinputs-$$"
echo "fixtures: system=$SYSTEM fresh=$FRESH scratch=$W"

# -------------------------------------------------------------- fixtures ---
# Two leaf flakes with no inputs of their own. They are what the root flakes
# point at, and `mid` below is what makes a node two hops from the root.
#
# Each carries a data file as well as an output, because a derivation that
# merely mentions `dep.marker` proves only that a string crossed; one whose
# builder arguments contain `${dep}/marker.txt` puts the input's store path
# into the `.drv` and into its `inputSrcs`. That is the check the `outPath`
# escape in `flakeOverridesJSON` exists for: JSON cannot carry string context,
# so a lost `Opaque` element is a vanished derivation input, and it is
# invisible in every comparison that only looks at printed strings.
leafflake() { # DIR MARKER
  mkdir -p "$1"
  printf '%s\n' "$2" > "$1/marker.txt"
  cat > "$1/flake.nix" <<NIXEOF
{
  description = "flake-inputs-parity leaf $2";
  outputs = { self }: {
    marker = "$2";
    number = ${#2};
  };
}
NIXEOF
}
leafflake "$FIX/dep" dep-one
leafflake "$FIX/leaf" leaf-two

# `mid` has inputs of its own, so a root flake pointing at it has a node two
# hops away -- the "input of an input" shape. It also re-exports what it saw,
# so the `follows` fixture can assert that redirection happened rather than
# merely that evaluation succeeded.
mkdir -p "$FIX/mid"
printf 'mid-three\n' > "$FIX/mid/marker.txt"
cat > "$FIX/mid/flake.nix" <<NIXEOF
{
  description = "flake-inputs-parity mid";
  inputs.dep.url = "path:$FIX/dep";
  inputs.leaf.url = "path:$FIX/leaf";
  outputs = { self, dep, leaf }: {
    marker = "mid-three";
    sawDep = dep.marker;
    sawLeaf = leaf.marker;
  };
}
NIXEOF

# A git repository, served over `git+file://`. This is the one fixture with
# revision metadata: `rev`, `shortRev`, `revCount` and `lastModified` only
# exist for a git input, and those are exactly the attributes
# `flakeOverridesJSON` serialises one at a time and `fetchTreeFinal` returns
# from the lock. A `path:` input has none of them, so without this fixture the
# metadata half of the boundary is untested.
GITDEP="$FIX/gitdep"
leafflake "$GITDEP" git-four
git -C "$GITDEP" -c init.defaultBranch=main init -q
git -C "$GITDEP" add -A
git -C "$GITDEP" -c user.email=parity@example.invalid -c user.name=parity \
  -c commit.gpgsign=false commit -q -m "flake-inputs-parity git fixture"

# A tarball, served over `tarball+file://`.
#
# This stands in for `github:` and the substitution is worth stating plainly,
# because it is the one shape this gate cannot do honestly. A `github:` input
# resolves through `GitArchiveInputScheme`, which downloads a codeload tarball
# and hands it to the same tarball machinery this fixture uses; so the
# evaluator-side surface -- a locked non-path node, fetched through
# `fetchTreeFinal`, whose `sourceInfo` carries `narHash` and `lastModified`
# but no `rev` unless the lock names one -- is the same. What is NOT covered
# is `github:`'s own resolution: the API call, the rev-to-tarball mapping and
# the `lastModified` it reads out of a header. Those need a network, a test
# needing a network is a test that flakes, and none of them is evaluator code.
TARSRC="$FIX/tarsrc"
leafflake "$TARSRC" tar-five
TARBALL="$FIX/tarsrc.tar.gz"
# Members named individually and packed from inside the directory, so
# `flake.nix` is at the archive root. Packing the directory itself instead
# produces a tarball whose only top-level entry is `tarsrc/`, and this fork's
# tarball fetcher unpacks it into the Git cache without stripping that
# component, so locking fails with `'«…tarsrc.tar.gz»/flake.nix' does not
# exist` -- a fixture bug that reads exactly like a fetcher bug.
tar -czf "$TARBALL" -C "$TARSRC" flake.nix marker.txt

# Each root fixture below gets the same output surface, so one case table can
# drive all of them:
#
#   marker      a string that could only have been produced by traversing the
#               input graph
#   depOutPath  the input's outPath as a string, which is what a lost store
#               path context would corrupt
#   meta        the source metadata that crossed the JSON boundary
#   packages.$SYSTEM.fixture
#               a derivation whose builder arguments name the input tree
rootflake() { # DIR NAME INPUTS_NIX BINDINGS MARKER_EXPR DEPOUT_EXPR META_EXPR
  mkdir -p "$1"
  cat > "$1/flake.nix" <<NIXEOF
{
  description = "flake-inputs-parity root $2";
$3
  outputs = { self, $4 }: {
    marker = $5;
    depOutPath = $6;
    meta = $7;
    packages."$SYSTEM".fixture = derivation {
      name = "$FRESH-$2";
      system = "$SYSTEM";
      builder = "/bin/sh";
      args = [ "-c" "cat \${$6}/marker.txt > \$out" ];
    };
  };
}
NIXEOF
}

# An alternative dep, so the `follows` fixture can assert that redirection
# HAPPENED rather than that evaluation succeeded. `mid` names `$FIX/dep` for
# its own `dep` input; the follows fixture points its root `dep` at this one
# instead, so `mid.sawDep` reads `dep-alt` if the follows took effect and
# `dep-one` if it silently did not. Without two different markers the fixture
# passes whether or not `resolveInput` ever sees a list.
leafflake "$FIX/depalt" dep-alt

# 1. one absolute path input.
rootflake "$FIX/r-abspath" abspath \
  "  inputs.dep.url = \"path:$FIX/dep\";" \
  "dep" \
  '"abs+" + dep.marker' \
  'dep.outPath' \
  '{ inherit (dep.sourceInfo) narHash; }'

# 2. one RELATIVE path input, which is the `isRelative` branch of
# `call-flake.nix`: its node has no `sourceInfo` of its own and its `outPath`
# is built by string concatenation from the parent node's. Nothing had run it.
#
# Its `meta` row is the attribute NAMES of that sourceInfo and not a hash off
# it, because a relative node has no hash: cppnix answers `["outPath"]` here
# where every other fixture answers `["lastModified", "lastModifiedDate",
# "narHash", "outPath"]`. Asking for `narHash` made both arms fail with
# `attribute 'narHash' missing` -- identically, so it was honest parity, but a
# row that produces no value says less than one that produces the difference.
mkdir -p "$FIX/r-relpath"
leafflake "$FIX/r-relpath/dep" rel-six
rootflake "$FIX/r-relpath" relpath \
  '  inputs.dep.url = "path:./dep";' \
  "dep" \
  '"rel+" + dep.marker' \
  'dep.outPath' \
  '{ names = builtins.attrNames dep.sourceInfo; }'

# 3. a tarball input, `github:`'s stand-in. See the fixture header.
rootflake "$FIX/r-tarball" tarball \
  "  inputs.tb.url = \"tarball+file://$TARBALL\";" \
  "tb" \
  '"tar+" + tb.marker' \
  'tb.outPath' \
  '{ inherit (tb.sourceInfo) narHash; }'

# 4. a git input, the only fixture with revision metadata.
rootflake "$FIX/r-git" git \
  "  inputs.g.url = \"git+file://$GITDEP\";" \
  "g" \
  '"git+" + g.marker' \
  'g.outPath' \
  '{ inherit (g.sourceInfo) narHash rev shortRev revCount lastModified; }'

# 5. `follows`: `mid`'s own `dep` is redirected to the root's, which points at
# a DIFFERENT flake. This is the only shape that makes `resolveInput` receive
# a list and `getInputByPath` recurse.
rootflake "$FIX/r-follows" follows \
  "  inputs.dep.url = \"path:$FIX/depalt\";
  inputs.mid.url = \"path:$FIX/mid\";
  inputs.mid.inputs.dep.follows = \"dep\";" \
  "dep, mid" \
  '"follows+" + mid.sawDep + "+" + mid.sawLeaf' \
  'mid.outPath' \
  '{ inherit (mid.sourceInfo) narHash; }'

# 6. nested inputs: the root names only `mid`, and reads through it to nodes
# two hops away, including `mid.inputs.leaf`, which is the `inputs` attribute
# of a non-root node.
rootflake "$FIX/r-nested" nested \
  "  inputs.mid.url = \"path:$FIX/mid\";" \
  "mid" \
  '"nested+" + mid.sawDep + "+" + mid.inputs.leaf.marker' \
  'mid.inputs.leaf.outPath' \
  '{ inherit (mid.inputs.leaf.sourceInfo) narHash; }'

# 7. a non-flake input, which is `call-flake.nix`'s `node.flake or true`
# false branch: the node's result is its `sourceInfo` and nothing else, so
# there is no `outputs` call and the `isFunction` assert never runs. Read
# through it with `readFile` so the row also proves a plain read out of a
# fetched tree works under a flake's pure eval (ENG-12792).
rootflake "$FIX/r-nonflake" nonflake \
  "  inputs.data = { url = \"path:$FIX/dep\"; flake = false; };" \
  "data" \
  '"nonflake+" + builtins.readFile (data + "/marker.txt")' \
  'data.outPath' \
  '{ inherit (data) narHash; }'

# 8. an input that names a subdirectory of the tree it fetched, `?dir=sub`.
# This is the other half of `call-flake.nix`'s `outPath`: `sourceInfo.outPath`
# plus `"/" + subdir`, where `subdir` comes from the override's `dir` or the
# node's `locked.dir`. Every fixture above has an empty subdir, so the
# concatenation was never non-trivial.
mkdir -p "$FIX/multi"
leafflake "$FIX/multi/sub" sub-eight
rootflake "$FIX/r-subdir" subdir \
  "  inputs.s.url = \"path:$FIX/multi?dir=sub\";" \
  "s" \
  '"sub+" + s.marker' \
  's.outPath' \
  '{ inherit (s.sourceInfo) narHash; }'

# label -> directory. Ordered, because the report reads better grouped by
# fixture and because the shape assertions below are indexed by label.
declare -a FIXTURES=(
  "abspath:$FIX/r-abspath"
  "relpath:$FIX/r-relpath"
  "tarball:$FIX/r-tarball"
  "git:$FIX/r-git"
  "follows:$FIX/r-follows"
  "nested:$FIX/r-nested"
  "nonflake:$FIX/r-nonflake"
  "subdir:$FIX/r-subdir"
)

# ------------------------------------------------- lock, then assert shape ---
# Locked by cppnix, once, before either arm runs. See the header: this is what
# puts the fixtures on the lazy branch of `computeLocks`, which is the only
# branch that leaves a node without an override and so the only one that
# reaches `fetchTreeFinal`.
for entry in "${FIXTURES[@]}"; do
  label=${entry%%:*}; dir=${entry#*:}
  if ! NIX_CONFIG="$CPP" "$NIXBIN" flake lock "path:$dir" > "$W/lock-$label.log" 2>&1; then
    echo "flake-inputs-parity: could not lock the '$label' fixture, so it cannot be compared:"
    sed 's/^/    /' "$W/lock-$label.log"
    exit 2
  fi
done

# A fixture that does not have the shape it is named for is a row that passes
# for the wrong reason. `relpath` reduced to an absolute path input, or
# `follows` whose follows was silently dropped, would agree across both arms
# and tell nobody that the branch it exists to cover never ran. So read the
# lock cppnix wrote and require the shape in it.
cat > "$W/shape.py" <<'PYEOF'
import json, sys

label, path = sys.argv[1], sys.argv[2]
lock = json.load(open(path))
nodes = lock["nodes"]
root = lock["root"]
others = {k: v for k, v in nodes.items() if k != root}


def locked(pred):
    return [k for k, v in others.items() if "locked" in v and pred(v["locked"])]


def fail(why):
    print("no: " + why)
    sys.exit(1)


if label == "abspath":
    hits = locked(lambda l: l.get("type") == "path" and l.get("path", "").startswith("/"))
    if not hits:
        fail("no absolute path node")
elif label == "relpath":
    hits = locked(lambda l: l.get("type") == "path" and not l.get("path", "/").startswith("/"))
    if not hits:
        fail("no relative path node, so call-flake.nix's isRelative branch never runs")
elif label == "tarball":
    if not locked(lambda l: l.get("type") == "tarball"):
        fail("no tarball node")
elif label == "git":
    hits = locked(lambda l: l.get("type") == "git" and l.get("rev"))
    if not hits:
        fail("no git node carrying a rev, so there is no revision metadata to compare")
elif label == "follows":
    # A follows is spelled as a LIST in a node's inputs, and it is the only
    # thing that makes resolveInput recurse through getInputByPath.
    fols = [
        (k, i, spec)
        for k, v in nodes.items()
        for i, spec in (v.get("inputs") or {}).items()
        if isinstance(spec, list)
    ]
    if not fols:
        fail("no node has a list-valued input, so nothing follows anything")
elif label == "nested":
    # Depth: some non-root node has an input of its own that is a real node.
    deep = [
        k
        for k, v in others.items()
        if any(isinstance(s, str) and s in nodes for s in (v.get("inputs") or {}).values())
    ]
    if not deep:
        fail("no node has inputs of its own, so no node is two hops from the root")
elif label == "nonflake":
    if not [k for k, v in others.items() if v.get("flake") is False]:
        fail("no node is marked flake=false")
elif label == "subdir":
    # The subdirectory has to survive into the lock, or `call-flake.nix`
    # concatenates an empty string and the row proves nothing.
    if not locked(lambda l: l.get("dir")):
        fail("no node carries a 'dir', so the subdir concatenation is trivial")
else:
    fail("unknown fixture label")

print("ok: nodes=%d" % len(nodes))
PYEOF

shapes_ok=1
for entry in "${FIXTURES[@]}"; do
  label=${entry%%:*}; dir=${entry#*:}
  if [ ! -f "$dir/flake.lock" ]; then
    echo "shape $label: no flake.lock was written, so nothing about this fixture is pinned"
    shapes_ok=0
    continue
  fi
  verdict=$(python3 "$W/shape.py" "$label" "$dir/flake.lock") || shapes_ok=0
  printf "shape %-9s %s\n" "$label" "$verdict"
done
[ "$shapes_ok" = 1 ] || {
  echo "flake-inputs-parity: a fixture does not have the shape it is named for; the rows it"
  echo "  contributes would agree without covering the branch they exist to cover."
  exit 2
}

# ------------------------------------- did anything reach the tree fetcher ---
# The header claims pre-locking is what sends a node through `fetchTreeFinal`.
# This is where that claim is checked rather than asserted, because it is
# invisible in every value the fixtures produce: an overridden node and a
# fetched one yield the same store path, the same narHash and the same drv.
#
# `flakeOverridesJSON` emits its coverage at `debug`. `covered < nodes` means
# at least one node had no override and `call-flake.nix` called
# `fetchTreeFinal` for it. `covered == nodes` means the tree fetcher was dead
# code for that fixture, and a row from it says nothing about the fetcher
# however green it is.
#
# Measured on this Mac, aarch64-darwin. `relpath` is the one fixture that is
# expected to be fully covered and it is not an accident: a kept flake whose
# lock subtree holds a relative-path input forces `mustRefetch` in
# `computeLocks` (the NixOS/nix#14762 guard), which re-fetches and therefore
# re-adds the node to `nodePaths`. So the relative-path branch of
# `call-flake.nix` and the `fetchTreeFinal` branch are mutually exclusive by
# construction, which is worth knowing before reading a coverage number as a
# regression.
declare -a COVER_EXPECT=(
  "abspath:fetcher"
  "relpath:all-overridden"
  "tarball:fetcher"
  "git:fetcher"
  "follows:fetcher"
  "nested:fetcher"
  "nonflake:fetcher"
  "subdir:fetcher"
)
cover_ok=1
fetcher_fixtures=0
for entry in "${COVER_EXPECT[@]}"; do
  label=${entry%%:*}; want=${entry#*:}
  dir=""
  for f in "${FIXTURES[@]}"; do [ "${f%%:*}" = "$label" ] && dir=${f#*:}; done
  [ -n "$dir" ] || { echo "coverage: no fixture named '$label'"; cover_ok=0; continue; }
  NIX_CONFIG="$RUST" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/fstats-$label.json" \
    "$NIXBIN" eval --raw -vvvv "path:$dir#marker" \
    > /dev/null 2> "$W/cover-$label.err"

  # Which evaluator read this flake's OUTPUTS. The probe at the top of the
  # file established that `-E 1` runs on the VM; it says nothing about a
  # flake, and a flake path that quietly fell back to cppnix would make all
  # thirty-five rows below agree for the emptiest possible reason. So this is
  # asked per fixture, off the counters rather than off the setting.
  #
  # `cppFlakeLock` is expected to be non-zero and is not a failure: locking
  # evaluates flake.nix to read its `inputs`, which is the one sanctioned C++
  # evaluation under `eval-backend = rust`, and it is counted apart precisely
  # so a flake run can still be read as `evaluator: rust`. What may not
  # happen is a plain `cpp` call, and `rust` must be positive -- an
  # all-zero row would satisfy "cpp == 0" perfectly.
  prov=$(python3 -c 'import json,sys
try:
    d = json.load(open(sys.argv[1]))
except OSError:
    print("no-stats"); raise SystemExit
c = d.get("evaluatorCalls") or {}
print("%s cpp=%s lock=%s rust=%s" % (d.get("evaluator", "<absent>"),
                                     c.get("cpp", "<absent>"),
                                     c.get("cppFlakeLock", "<absent>"),
                                     c.get("rust", "<absent>")))' "$W/fstats-$label.json")
  case $prov in
    "rust cpp=0 lock="*" rust="*) ;;
    *)
      echo "provenance $label: $prov"
      echo "  the flake's outputs did not evaluate on the VM with locking as the only C++ call."
      echo "  Every row from this fixture would agree because both arms were cppnix."
      cover_ok=0
      ;;
  esac
  case $prov in
    *" rust=0"|*" rust=<absent>"*) echo "provenance $label: rust=0, so the VM served nothing"; cover_ok=0 ;;
    *" lock=0"|*" lock=<absent>"*) echo "provenance $label: lock=$prov, so no flake was locked"; cover_ok=0 ;;
  esac
  printf "provenance %-8s %s\n" "$label" "$prov"

  line=$(LC_ALL=C grep -a 'flake overrides cover' "$W/cover-$label.err" | tail -1)
  if [ -z "$line" ]; then
    # An absence is not a pass. No line means the bridge never built an
    # overrides document, which is a different program from the one being
    # measured -- most likely the binary predates the debug line.
    echo "coverage $label: the bridge emitted no coverage line, so this run cannot say"
    echo "  whether the tree fetcher was reached. Rebuild the binary from this tree."
    cover_ok=0
    continue
  fi
  covered=$(printf '%s' "$line" | sed -n 's/.*cover \([0-9]*\) of \([0-9]*\).*/\1/p')
  total=$(printf '%s' "$line" | sed -n 's/.*cover \([0-9]*\) of \([0-9]*\).*/\2/p')
  if [ "$covered" -lt "$total" ]; then got=fetcher; else got=all-overridden; fi
  [ "$got" = fetcher ] && fetcher_fixtures=$((fetcher_fixtures+1))
  printf "coverage %-9s %s (%s of %s overridden)\n" "$label" "$got" "$covered" "$total"
  [ "$got" = "$want" ] || {
    echo "  expected $want. If this flipped to all-overridden the fixture stopped exercising"
    echo "  fetchTreeFinal, and its rows now measure the override path twice."
    cover_ok=0
  }
done
[ "$cover_ok" = 1 ] || exit 2
# A positive count of the thing, not an absence of complaints: the loop above
# is satisfied by an empty COVER_EXPECT.
if [ "$fetcher_fixtures" != "$FLAKE_INPUTS_FETCHER_FIXTURES" ]; then
  echo "flake-inputs-parity: $fetcher_fixtures fixture(s) reached fetchTreeFinal, expected $FLAKE_INPUTS_FETCHER_FIXTURES"
  exit 2
fi

# ------------------------------- the one refusal on this path, fired by name ---
# `rustEvaluandOf` refuses a flake installable outright while the read-set
# tracker is on: `emitTreeAttrs` answers with a per-attribute recording thunk
# under the tracker, and the overrides document forces every one of them, so
# serialising them would both record reads the flake never made and lose the
# ones it does make.
#
# Checked here rather than trusted. `rust-flake-entry.md` carried that refusal
# as "written and has not been provoked" from the day it landed, and a refusal
# nobody has fired is a branch nobody has run. It also needs the cpp arm as a
# control, because this is the one assertion in the file whose passing state
# is a failure: a binary that refused everything, or an arm that failed for an
# unrelated reason, would satisfy "the rust arm failed" perfectly.
tracker_dir=$FIX/r-abspath
NIX_CONFIG="$RUST
read-set-trace-file = $W/tracker-rust.json" "$NIXBIN" eval --raw "path:$tracker_dir#marker" \
  > "$W/tracker-rust.out" 2> "$W/tracker-rust.err"; trrc=$?
NIX_CONFIG="$CPP
read-set-trace-file = $W/tracker-cpp.json" "$NIXBIN" eval --raw "path:$tracker_dir#marker" \
  > "$W/tracker-cpp.out" 2> "$W/tracker-cpp.err"; tcrc=$?
if [ "$trrc" = 0 ]; then
  echo "tracker: the rust arm SERVED a flake installable with the read-set tracker on."
  echo "  Either the refusal went away without the VM growing the recorded tree attributes"
  echo "  it stands in for, or this is not the binary being described. Serving it loses the"
  echo "  provenance graph quietly, which is the failure nobody notices."
  exit 2
fi
if [ "$tcrc" != 0 ] || [ ! -s "$W/tracker-cpp.out" ]; then
  echo "tracker: the CPP arm also failed with the tracker on, so the rust arm's refusal says"
  echo "  nothing about the backend: $(last_error "$W/tracker-cpp.err")"
  exit 2
fi
if ! LC_ALL=C grep -aq 'token=command-unsupported' "$W/tracker-rust.err"; then
  echo "tracker: the rust arm refused, but not with the command-unsupported token:"
  last_error "$W/tracker-rust.err" | sed 's/^/    /'
  exit 2
fi
echo "tracker: rust refuses by name (command-unsupported), cpp serves '$(cat "$W/tracker-cpp.out")'"

# The hash of every lock, read now and re-read at the end. If an arm re-locks,
# the two arms evaluated different lock files and every row below compared two
# different programs.
for entry in "${FIXTURES[@]}"; do
  label=${entry%%:*}; dir=${entry#*:}
  shasum -a 256 "$dir/flake.lock" | cut -d' ' -f1 > "$W/lockhash-$label"
done

# ------------------------------------------------------------ comparison ---
# attr | how to render it. `raw` and `json` are ordinary `nix eval` rows
# scored on bytes; `drv` additionally opens the `.drv` the evaluation wrote
# and hashes it, because a computed path and a written one print identically
# and only the second is a store object (ENG-12799).
declare -a ATTRS=(
  "marker|raw"
  "depOutPath|raw"
  "meta|json"
  "packages.$SYSTEM.fixture.drvPath|drv"
  "packages.$SYSTEM.fixture.outPath|raw"
)

rows=0; match=0; mismatch=0; unimpl=0; failboth=0; empty=0
drvrows=0; drvwritten=0
declare -a MISMATCHES=()

run_arm() { # ARM INSTALLABLE RENDER OUTFILE ERRFILE -> rc
  local cfg
  case $1 in cpp) cfg=$CPP ;; *) cfg=$RUST ;; esac
  case $3 in
    json) NIX_CONFIG="$cfg" "$NIXBIN" eval --json "$2" > "$4" 2> "$5" ;;
    *)    NIX_CONFIG="$cfg" "$NIXBIN" eval --raw  "$2" > "$4" 2> "$5" ;;
  esac
}

for entry in "${FIXTURES[@]}"; do
  label=${entry%%:*}; dir=${entry#*:}
  for spec in "${ATTRS[@]}"; do
    attr=${spec%%|*}; render=${spec#*|}
    rows=$((rows+1))
    installable="path:$dir#$attr"
    # The rust arm first, before anything else can have written the `.drv`.
    # The order is the assertion: an evaluation alone puts the `.drv` in the
    # store, so checking after the cpp arm would credit the rust arm for what
    # cpp wrote.
    run_arm rust "$installable" "$render" "$W/r.out" "$W/r.err"; rrc=$?
    wrote=n/a
    if [ "$render" = drv ]; then
      drvrows=$((drvrows+1))
      wrote=absent
      rpath=$(cat "$W/r.out")
      if [ -n "$rpath" ] && [ -f "$rpath" ]; then
        wrote=present
        drvwritten=$((drvwritten+1))
        shasum -a 256 "$rpath" | cut -d' ' -f1 > "$W/r.drvsum"
      else
        : > "$W/r.drvsum"
      fi
    fi
    run_arm cpp "$installable" "$render" "$W/c.out" "$W/c.err"; crc=$?
    if [ "$render" = drv ]; then
      cpath=$(cat "$W/c.out")
      if [ -n "$cpath" ] && [ -f "$cpath" ]; then
        shasum -a 256 "$cpath" | cut -d' ' -f1 > "$W/c.drvsum"
      else
        : > "$W/c.drvsum"
      fi
    fi

    arms_score "$W/c.out" "$crc" "$W/r.out" "$rrc"
    verdict=$ARMS_VERDICT
    note=""

    # A named refusal is a gap, never a wrong answer, and it is reported
    # apart from a divergence. It can only be claimed when the rust arm is
    # the one that refused: two arms failing for unrelated reasons is a
    # divergence wearing a refusal's clothes.
    if [ "$verdict" != match ] && [ "$rrc" != 0 ] \
       && LC_ALL=C grep -aq 'rust-eval unimplemented' "$W/r.err"; then
      verdict=unimplemented
      note=$(last_error "$W/r.err")
    elif [ "$verdict" = fail-both ]; then
      # Both failed. Whether that is agreement is a tier-2 question with a
      # tier-2 answer: the error CLASS, not the wording.
      cclass=$(error_class "$W/c.err"); rclass=$(error_class "$W/r.err")
      note="cpp=$cclass rust=$rclass"
      if [ "$cclass" != "$rclass" ]; then
        verdict=differ
        note="failed differently: $note"
      fi
    fi

    # Tier 1: for a drv row, agreeing on the printed path is not enough. The
    # file has to exist and its bytes have to match.
    if [ "$verdict" = match ] && [ "$render" = drv ]; then
      if [ "$wrote" != present ]; then
        verdict=differ
        note="the rust arm printed a drvPath and left no file at it"
      elif ! cmp -s "$W/r.drvsum" "$W/c.drvsum"; then
        verdict=differ
        note="same drvPath, different .drv bytes: rust=$(cat "$W/r.drvsum") cpp=$(cat "$W/c.drvsum")"
      elif [ ! -s "$W/r.drvsum" ]; then
        verdict=differ
        note="no .drv was hashed on either arm, so the bytes were not compared"
      fi
    fi

    case $verdict in
      match)         match=$((match+1)) ;;
      unimplemented) unimpl=$((unimpl+1)) ;;
      fail-both)     failboth=$((failboth+1)) ;;
      empty)         empty=$((empty+1)) ;;
      # Only this bucket is collected for the summary list below. A named
      # refusal and a both-arms-failed pair are also "not match", and listing
      # them under "diverged" would name rows that did not diverge.
      *)             mismatch=$((mismatch+1)); MISMATCHES+=("$label $attr") ;;
    esac

    printf "%-14s %-9s %s\n" "$verdict" "$label" "$attr"
    if [ "$verdict" = match ]; then
      out=$(head -c 200 "$W/c.out" | tr '\n' '|')
      printf "      %s%s\n" "$out" "$([ "$render" = drv ] && printf " (.drv %s, sha %s)" "$wrote" "$(cut -c1-16 < "$W/c.drvsum")")"
    else
      printf "      cpp  rc=%s out=[%s] err=[%s]\n" \
        "$crc" "$(head -c 200 "$W/c.out" | tr '\n' '|')" "$(last_error "$W/c.err" | head -c 240)"
      printf "      rust rc=%s out=[%s] err=[%s]\n" \
        "$rrc" "$(head -c 200 "$W/r.out" | tr '\n' '|')" "$(last_error "$W/r.err" | head -c 240)"
      [ -n "$note" ] && printf "      note %s\n" "$note"
    fi
  done
done

# ------------------------------------------- the same flakes, via getFlake ---
# `builtins.getFlake` reaches the same machinery from inside an expression
# rather than from the command line, and ENG-12995 put both on one seam:
# `rustLockFlake` and `rustEvaluandOf` call the same `callFlakeSource()` and
# the same `flakeOverridesJSON`, and the VM evaluates `outputs` either way.
#
# So every fixture above is asked a second time, and each row is scored twice
# rather than once:
#
#   cross-arm   rust getFlake vs cpp getFlake, the ordinary comparison
#   oracle      rust getFlake vs the rust COMMAND LINE for the same flake
#
# The second is the one worth having and it is what "one seam" means as a
# testable claim. A cross-arm comparison alone would pass if both entry points
# were wrong in the same way -- if `getFlake` grew a second, subtly different
# overrides document, both arms would agree with themselves and the store paths
# would move together. The oracle is what refuses that: the command line's
# answer is already known good from the rows above, so a getFlake row that
# disagrees with it is a divergence between the two entry points even when both
# arms agree.
#
# `--impure` on both arms, and it is cppnix's rule rather than a convenience:
# `prim_getFlake` refuses an unlocked flake reference under pure eval, and a
# `path:` reference with no narHash is unlocked. Both arms get it, so it moves
# no comparison.
gf_rows=0; gf_match=0; gf_mismatch=0; gf_unimpl=0; gf_oracle_ok=0
declare -a GF_MISMATCHES=()

gf_arm() { # ARM DIR ATTR RENDER OUTFILE ERRFILE -> rc
  local cfg expr
  case $1 in cpp) cfg=$CPP ;; *) cfg=$RUST ;; esac
  expr="(builtins.getFlake \"path:$2\").$3"
  case $4 in
    json) NIX_CONFIG="$cfg" "$NIXBIN" eval --json --impure --expr "$expr" > "$5" 2> "$6" ;;
    *)    NIX_CONFIG="$cfg" "$NIXBIN" eval --raw  --impure --expr "$expr" > "$5" 2> "$6" ;;
  esac
}

echo "-- the same fixtures through builtins.getFlake (ENG-12995) --"
for entry in "${FIXTURES[@]}"; do
  label=${entry%%:*}; dir=${entry#*:}
  for spec in "${ATTRS[@]}"; do
    attr=${spec%%|*}; render=${spec#*|}
    gf_rows=$((gf_rows+1))
    gf_arm rust "$dir" "$attr" "$render" "$W/gr.out" "$W/gr.err"; grrc=$?
    gf_arm cpp  "$dir" "$attr" "$render" "$W/gc.out" "$W/gc.err"; gcrc=$?
    # The oracle: the same flake and attribute through the command line, on
    # the rust arm, which the rows above already compared against cpp.
    run_arm rust "path:$dir#$attr" "$render" "$W/go.out" "$W/go.err"; gorc=$?

    arms_score "$W/gc.out" "$gcrc" "$W/gr.out" "$grrc"
    verdict=$ARMS_VERDICT
    note=""
    if [ "$verdict" != match ] && [ "$grrc" != 0 ] \
       && LC_ALL=C grep -aq 'rust-eval unimplemented' "$W/gr.err"; then
      verdict=unimplemented
      note=$(last_error "$W/gr.err")
    elif [ "$verdict" = fail-both ]; then
      cclass=$(error_class "$W/gc.err"); rclass=$(error_class "$W/gr.err")
      note="cpp=$cclass rust=$rclass"
      if [ "$cclass" != "$rclass" ]; then verdict=differ; note="failed differently: $note"; fi
    fi
    # The oracle is checked only where there is something to compare: a row
    # both entry points refuse has no bytes, and calling that agreement would
    # be the empty-agreement bug this file exists to refuse.
    if [ "$verdict" = match ]; then
      if [ "$gorc" != 0 ] || ! cmp -s "$W/gr.out" "$W/go.out"; then
        verdict=differ
        note="getFlake and the command line disagree for the same flake: cmdline rc=$gorc [$(head -c 120 "$W/go.out" | tr '\n' '|')]"
      else
        gf_oracle_ok=$((gf_oracle_ok+1))
      fi
    fi

    case $verdict in
      match)         gf_match=$((gf_match+1)) ;;
      unimplemented) gf_unimpl=$((gf_unimpl+1)) ;;
      *)             gf_mismatch=$((gf_mismatch+1)); GF_MISMATCHES+=("$label $attr") ;;
    esac
    printf "%-14s %-9s getFlake %s\n" "$verdict" "$label" "$attr"
    if [ "$verdict" != match ]; then
      printf "      cpp  rc=%s out=[%s] err=[%s]\n" \
        "$gcrc" "$(head -c 160 "$W/gc.out" | tr '\n' '|')" "$(last_error "$W/gc.err" | head -c 200)"
      printf "      rust rc=%s out=[%s] err=[%s]\n" \
        "$grrc" "$(head -c 160 "$W/gr.out" | tr '\n' '|')" "$(last_error "$W/gr.err" | head -c 200)"
      [ -n "$note" ] && printf "      note %s\n" "$note"
    fi
  done
done

# The refusal that is still a refusal, asserted with the cpp arm as a control
# for the reason the installable one above is.
#
# A DIFFERENT token from the installable refusal, and the difference is the
# point rather than an inconsistency. `command-unsupported` says the command
# layer would not build an installable; here the command runs fine and a
# *builtin* is what cannot be served, which is `unimplemented-builtin` -- the
# same token `builtins.getFlake` reported before it was implemented at all.
# A census filtering on one of them should not be shown the other.
NIX_CONFIG="$RUST
read-set-trace-file = $W/gf-tracker.json" "$NIXBIN" eval --raw --impure \
  --expr "(builtins.getFlake \"path:$FIX/r-abspath\").marker" \
  > "$W/gft-rust.out" 2> "$W/gft-rust.err"; gftrc=$?
NIX_CONFIG="$CPP
read-set-trace-file = $W/gf-tracker-cpp.json" "$NIXBIN" eval --raw --impure \
  --expr "(builtins.getFlake \"path:$FIX/r-abspath\").marker" \
  > "$W/gft-cpp.out" 2> "$W/gft-cpp.err"; gftcrc=$?
gf_tracker=refused
if [ "$gftrc" = 0 ]; then
  echo "getFlake tracker: the rust arm SERVED getFlake with the read-set tracker on."
  echo "  The overrides it hands over are emitTreeAttrs sets, which are per-attribute recording"
  echo "  thunks under the tracker; serving them loses the provenance graph quietly."
  gf_tracker=served
elif [ "$gftcrc" != 0 ] || [ ! -s "$W/gft-cpp.out" ]; then
  echo "getFlake tracker: the CPP arm also failed, so the rust refusal says nothing: $(last_error "$W/gft-cpp.err")"
  gf_tracker=uncontrolled
elif ! LC_ALL=C grep -aq 'token=unimplemented-builtin' "$W/gft-rust.err"; then
  echo "getFlake tracker: refused, but not with the unimplemented-builtin token:"
  last_error "$W/gft-rust.err" | sed 's/^/    /'
  gf_tracker=wrong-token
fi
echo "getFlake tracker: $gf_tracker (cpp serves '$(cat "$W/gft-cpp.out")')"

echo "RESULT flake-inputs-getflake rows=$gf_rows match=$gf_match mismatch=$gf_mismatch \
unimplemented=$gf_unimpl oracle-ok=$gf_oracle_ok tracker=$gf_tracker \
expected-rows=$FLAKE_INPUTS_ROWS min-match=$FLAKE_GETFLAKE_MIN_MATCH"

# ---------------------------------------------------------------- verdict ---
ok=1

if [ "$gf_mismatch" != 0 ]; then
  echo "flake-inputs-parity: $gf_mismatch getFlake row(s) diverged:"
  printf '    %s\n' "${GF_MISMATCHES[@]}"
  ok=0
fi
if [ "$gf_rows" != "$FLAKE_INPUTS_ROWS" ]; then
  echo "flake-inputs-parity: ran $gf_rows getFlake rows against $FLAKE_INPUTS_ROWS command-line rows;"
  echo "  the two arms must cover the same fixtures or the oracle covers less than it claims."
  ok=0
fi
if [ "$gf_match" -lt "$FLAKE_GETFLAKE_MIN_MATCH" ]; then
  echo "flake-inputs-parity: getFlake match=$gf_match is under the floor $FLAKE_GETFLAKE_MIN_MATCH."
  ok=0
fi
# A positive count of the thing, not an absence of complaints: every matching
# row must have been checked against the command line, or "one seam" is a
# claim this gate did not test.
if [ "$gf_oracle_ok" != "$gf_match" ]; then
  echo "flake-inputs-parity: $gf_oracle_ok of $gf_match matching getFlake rows were checked against"
  echo "  the command line. They must be equal: an unchecked row is the one that could differ."
  ok=0
fi
if [ "$gf_tracker" != refused ]; then
  echo "flake-inputs-parity: the getFlake read-set-tracker refusal is '$gf_tracker', not 'refused'."
  ok=0
fi

# Did an arm re-lock? If so the two arms did not evaluate the same lock file
# and everything above compared two programs rather than two backends.
for entry in "${FIXTURES[@]}"; do
  label=${entry%%:*}; dir=${entry#*:}
  now=$(shasum -a 256 "$dir/flake.lock" | cut -d' ' -f1)
  was=$(cat "$W/lockhash-$label")
  [ "$now" = "$was" ] || {
    echo "flake-inputs-parity: the '$label' fixture's flake.lock changed during the run"
    echo "  ($was -> $now). An arm re-locked, so the rows above compared two lock files."
    ok=0
  }
done

arms_require_rows "$rows" "flake input rows"

echo "RESULT flake-inputs-parity rows=$rows match=$match mismatch=$mismatch unimplemented=$unimpl \
fail-both=$failboth empty=$empty drv-rows=$drvrows drv-written=$drvwritten \
expected-rows=$FLAKE_INPUTS_ROWS min-match=$FLAKE_INPUTS_MIN_MATCH expected-drv-rows=$FLAKE_INPUTS_DRV_ROWS"

if [ "$mismatch" != 0 ]; then
  echo "flake-inputs-parity: $mismatch row(s) diverged:"
  printf '    %s\n' "${MISMATCHES[@]}"
  ok=0
fi
if [ "$empty" != 0 ]; then
  echo "flake-inputs-parity: $empty row(s) had both arms succeed and print nothing. That is an"
  echo "  agreement about nothing -- every one of these fixtures produces a value."
  ok=0
fi
if [ "$rows" != "$FLAKE_INPUTS_ROWS" ]; then
  echo "flake-inputs-parity: ran $rows rows, expected $FLAKE_INPUTS_ROWS. Update FLAKE_INPUTS_ROWS"
  echo "  in the same commit that changes FIXTURES or ATTRS."
  ok=0
fi
if [ "$match" -lt "$FLAKE_INPUTS_MIN_MATCH" ]; then
  echo "flake-inputs-parity: match=$match is under the floor $FLAKE_INPUTS_MIN_MATCH."
  ok=0
fi
# The tier-1 half, asserted apart from the floor. A run where every drv row
# turned into a refusal would still clear a low match floor while proving
# nothing about store paths, which is the one thing this gate may not do.
if [ "$drvrows" != "$FLAKE_INPUTS_DRV_ROWS" ]; then
  echo "flake-inputs-parity: ran $drvrows drv rows, expected $FLAKE_INPUTS_DRV_ROWS."
  ok=0
fi
if [ "$drvwritten" != "$FLAKE_INPUTS_DRV_ROWS" ]; then
  echo "flake-inputs-parity: the rust arm wrote a .drv for $drvwritten of $drvrows drv rows."
  echo "  A printed drvPath with no file at it is ENG-12799 all over again."
  ok=0
fi

[ "$ok" = 1 ] || exit 1
echo "flake-inputs-parity: OK"
