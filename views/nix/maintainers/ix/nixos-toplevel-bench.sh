#!/usr/bin/env bash
# Time and count one NixOS toplevel evaluation, on either backend.
#
#   NIX_BUILD_DIR=$PWD/build-rust ./nixos-toplevel-bench.sh [rust|cpp] [runs]
#
# This is the measurement `maintainers/ix/nixos-toplevel-profile.md` and
# `maintainers/ix/perf-counter-overhead.md` describe, as a script rather than
# as a paragraph, so a before/after pair is one command twice instead of a
# reconstruction. It prints the median of N runs and, on the rust arm, the
# `rustEvalPerf` counters from the median run.
#
# # Why it prints the drvPath beside the time
#
# A perf change that also changes the answer is not a perf change. The store
# path is the Tier 1 bytes for this expression, so it is printed on the same
# line as the number and a caller diffing two runs sees a semantic regression
# in the same glance as a speedup. A harness that reported only seconds would
# rank a broken evaluator first.
#
# # Why real and user, not one of them
#
# The rust arm crosses the bridge tens of thousands of times and the embedder
# does IO on the far side, so a wall-clock number carries page-cache state
# that a CPU number does not. Reporting both is what makes a claim like "the
# work went away" separable from "the disk was warm this time".
set -u
# shellcheck source=./arm-config.sh
. "$(cd "$(dirname "$0")" && pwd)/arm-config.sh" || exit 2
# One owner of the gates' nix configuration, before anything reads the
# environment: an ambient `lint-url-literals = fatal` otherwise makes every
# rust arm refuse and every row score `unimplemented` (ENG-12996).
arm_pin_environment


ARM=${1:-rust}
RUNS=${2:-5}
case $ARM in rust|cpp) ;; *) echo "arm must be rust or cpp (got '$ARM')"; exit 2 ;; esac

BUILD=${NIX_BUILD_DIR:-$PWD/build-rust}
NIXI=$BUILD/src/nix/nix-instantiate
[ -x "$NIXI" ] || { echo "no nix-instantiate at $NIXI"; exit 2; }

W=$(mktemp -d); trap 'rm -rf "$W"' EXIT

# The tree flake.lock pins, as nixpkgs-frontier.sh resolves it, so a number
# taken here and a frontier row refer to the same nixpkgs. NIXPKGS overrides
# it -- the two published profiles predate the lock pin and used
# llgwlxshmy0ifvxh7f8wq53vk5x7vd13-source, so reproducing them needs it set.
NIXPKGS=${NIXPKGS:-}
LOCK=$(cd "$(dirname "$0")/../.." && pwd)/flake.lock
if [ -z "$NIXPKGS" ]; then
  [ -f "$LOCK" ] || { echo "no flake.lock at $LOCK, and NIXPKGS is unset"; exit 2; }
  lock_type=$(jq -r '.nodes.nixpkgs.locked.type // "?"' "$LOCK")
  [ "$lock_type" = tarball ] || {
    echo "flake.lock pins nixpkgs as '$lock_type'; this script only fetches a"
    echo "'tarball' node. Set NIXPKGS explicitly."
    exit 2
  }
  lock_url=$(jq -r '.nodes.nixpkgs.locked.url' "$LOCK")
  lock_hash=$(jq -r '.nodes.nixpkgs.locked.narHash' "$LOCK")
  NIXPKGS=$(nix eval --raw --impure --expr \
    "builtins.fetchTarball { url = \"$lock_url\"; sha256 = \"$lock_hash\"; }" 2>"$W/err") || {
    echo "could not fetch the pinned nixpkgs ($lock_url):"; cat "$W/err"; exit 2
  }
fi
[ -d "$NIXPKGS" ] || { echo "no nixpkgs source at '$NIXPKGS' (set NIXPKGS)"; exit 2; }

# The profile's configuration, unchanged. `documentation.enable = false` is
# load-bearing for the runtime, and grub stays off because a grub-enabled
# config did not finish on the rust arm when this was written (ENG-12863).
EXPR='(import <nixpkgs/nixos> {
  configuration = {
    boot.loader.grub.enable = false;
    fileSystems."/" = { device = "/dev/sda1"; fsType = "ext4"; };
    system.stateVersion = "24.05";
    documentation.enable = false;
  };
  system = "x86_64-linux";
}).system.drvPath'

CFG="extra-experimental-features = rust-eval
eval-backend = $ARM
$(arm_base_config)"

# A setting is not a capability: `nix config show` reports `eval-backend =
# rust` on a binary built without the Rust evaluator, and every number below
# would then describe the cpp arm under a rust label.
probe=$(NIX_CONFIG="$CFG" "$NIXI" --eval --strict -E 1 2>&1)
[ "$probe" = 1 ] || { echo "the $ARM arm cannot evaluate '1'; nothing below would mean anything:"; echo "$probe"; exit 2; }
if [ "$ARM" = rust ]; then
  # ...and on the rust arm, that the counters exist. An absent rustEvalPerf
  # block is how a cpp-arm run would masquerade as a rust one here.
  NIX_CONFIG="$CFG" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/probe.json" \
    "$NIXI" --eval --strict -E 1 >/dev/null 2>&1
  jq -e '.rustEvalPerf' "$W/probe.json" >/dev/null || {
    echo "no rustEvalPerf block on the rust arm: the binary has no counters,"
    echo "so every per-unit number below would be a zero meaning 'not built in'."
    exit 2
  }
fi

echo "arm=$ARM runs=$RUNS"
echo "nixi=$NIXI"
echo "sha256=$(shasum -a 256 "$NIXI" | cut -d' ' -f1)"
echo "nixpkgs=$NIXPKGS"

for i in $(seq 1 "$RUNS"); do
  # `time` writes to stderr in the shell's own format; %R/%U keeps real and
  # user separable without parsing a locale-dependent line.
  TIMEFORMAT='%R %U'
  { t=$( { time NIX_CONFIG="$CFG" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/stats-$i.json" \
      "$NIXI" --eval --strict -I "nixpkgs=$NIXPKGS" -E "$EXPR" \
      > "$W/out-$i" 2> "$W/err-$i"; } 2>&1 ); } || true
  real=${t%% *}; user=${t##* }
  drv=$(tr -d '"' < "$W/out-$i")
  [ -n "$drv" ] || { echo "run $i produced nothing:"; tail -20 "$W/err-$i"; exit 1; }
  cpu=$(jq -r '.cpuTime' "$W/stats-$i.json")
  echo "run=$i real=${real}s user=${user}s cpu=${cpu}s drv=$drv"
  echo "$real $i" >> "$W/times"
  echo "$user" >> "$W/users"
  echo "$cpu" >> "$W/cpus"
done

# The median run, by real time, and its counters. The median rather than the
# mean because a single scheduling stall on a laptop moves a mean and not a
# median, and rather than the min because the min of five is a different
# estimator on each side of a comparison.
median_i=$(sort -n "$W/times" | awk -v n="$RUNS" 'NR==int((n+1)/2){print $2}')
med() { sort -g "$1" | awk -v n="$RUNS" 'NR==int((n+1)/2){print $1}'; }
# Each metric's own median, not the median run's value for it. cpuTime and
# user are much steadier than wall clock on a shared laptop, so tying them to
# whichever run happened to have the middle wall time throws away the very
# robustness they have -- which is how a 0.2s difference in a single sampled
# cpuTime came to stand in for a measurement here once.
echo "median_real=$(med "$W/times")s median_user=$(med "$W/users")s median_cpu=$(med "$W/cpus")s"
if [ "$ARM" = rust ]; then
  jq -r '.rustEvalPerf | to_entries | map("\(.key)=\(.value)") | join(" ")' \
    "$W/stats-$median_i.json"
fi
jq -r '"cpuTime=\(.cpuTime) nrFunctionCalls=\(.nrFunctionCalls) nrThunks=\(.nrThunks) nrPrimOpCalls=\(.nrPrimOpCalls) nrLookups=\(.nrLookups)"' \
  "$W/stats-$median_i.json"
