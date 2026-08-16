#!/usr/bin/env bash
# Benchmark evaluator arms with hyperfine and record the result beside the
# inputs that produced it. Baselines land in maintainers/ix/bench/ named
# baseline-<host>-<forkrev>-<nixpkgsrev>.json so a number can never be read
# without the machine and revisions it belongs to.
#
#   eval-bench.sh NIXBINDIR BENCH ARM [ARM...]
#
# BENCH ids:
#   b3-hello         nixpkgs#hello drvPath, the micro end-to-end
#   b2-nixpkgs-sweep full top-level nix-env -qaP sweep (the cold ceiling)
#   b1-toplevel      one ix host toplevel drvPath fresh eval; PE_IX must
#                    point at an ix flake checkout (host: hil-compute-1)
#
# The command line for each arm is embedded in hyperfine's JSON verbatim, so
# the artifact states what was measured inside the measurement.
set -u

[ $# -ge 3 ] || { grep '^#' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
NIXBINDIR=$1 BENCH=$2; shift 2

NIX=$NIXBINDIR/nix
NIX_ENV=$NIXBINDIR/nix-env
command -v hyperfine >/dev/null || { echo "eval-bench: hyperfine not on PATH" >&2; exit 2; }

repo_root=$(cd "$(dirname "$0")/../.." && pwd)
fork_rev=$(git -C "$repo_root" rev-parse --short=12 HEAD)
outdir=$repo_root/maintainers/ix/bench
mkdir -p "$outdir"

# Config and state isolation, as in lang-diff.sh and nixpkgs-drv-diff.sh:
# the machine nix.conf leaks settings that change what is measured
# (abort-on-warn killed the b2 sweep). The conf pins system=x86_64-linux
# so numbers compare across machines, and grants flakes for b1.
iso=$(mktemp -d /tmp/eval-bench.XXXXXX)
trap 'rm -rf "$iso"' EXIT
mkdir -p "$iso/conf"
printf 'experimental-features = nix-command flakes ca-derivations\nsystem = x86_64-linux\n' > "$iso/conf/nix.conf"
# NIX_STATE_DIR also relocates the daemon socket path, so on a daemon-based
# host (every fleet Linux box) overriding it silently downgrades nix to
# direct store access, which fails read-only ("creating directory ...:
# Read-only file system", dev-compute-2, 2026-08-03). Isolate state only
# where there is no daemon to lose.
iso_state=""
if [ ! -S /nix/var/nix/daemon-socket/socket ]; then
  iso_state="NIX_STATE_DIR=$iso/state-nonexistent"
fi
iso_env="NIX_CONF_DIR=$iso/conf NIX_USER_CONF_FILES='' $iso_state"

arm_config() {
  case $1 in
    none) ;;
    eval-backend=rust) printf 'extra-experimental-features = rust-eval\neval-backend = rust\n' ;;
    *=*) printf '%s = %s\n' "${1%%=*}" "${1#*=}" ;;
    *) echo "eval-bench: bad arm spec '$1'" >&2; exit 2 ;;
  esac
}

case $BENCH in
  b3-hello)
    NIXPKGS=${NIXPKGS:?set NIXPKGS to a nixpkgs checkout}
    input_rev=${NIXPKGS_REV:-$(git -C "$NIXPKGS" rev-parse --short=12 HEAD 2>/dev/null || echo unknown)}
    cmd() { echo "$iso_env NIX_CONFIG='$(arm_config "$1")' $NIX eval --raw --file $NIXPKGS hello.drvPath"; } ;;
  b2-nixpkgs-sweep)
    NIXPKGS=${NIXPKGS:?set NIXPKGS to a nixpkgs checkout}
    input_rev=${NIXPKGS_REV:-$(git -C "$NIXPKGS" rev-parse --short=12 HEAD 2>/dev/null || echo unknown)}
    cmd() { echo "$iso_env NIX_CONFIG='$(arm_config "$1")' $NIX_ENV -f $NIXPKGS -qaP --drv-path --option allow-import-from-derivation false"; } ;;
  b1-toplevel)
    # Host toplevels import-from-derivation x86_64-linux sources
    # (cargo-unit-planner-src), so this bench only runs where that can
    # build. Refuse early on darwin rather than fail after the fetch.
    [ "$(uname -s)" = Linux ] || { echo "eval-bench: b1-toplevel needs a Linux host (IFD of x86_64-linux drvs); run it on the gate box" >&2; exit 2; }
    PE_IX=${PE_IX:?set PE_IX to an ix flake checkout}
    input_rev=$(git -C "$PE_IX" rev-parse --short=12 HEAD 2>/dev/null || echo unknown)
    cmd() { echo "$iso_env NIX_CONFIG='$(arm_config "$1")' $NIX eval --raw $PE_IX#nixosConfigurations.hil-compute-1.config.system.build.toplevel.drvPath"; } ;;
  *) echo "eval-bench: unknown bench '$BENCH'" >&2; exit 2 ;;
esac

declare -a hf_args=()
for arm in "$@"; do
  hf_args+=(--command-name "$BENCH/$arm" "$(cmd "$arm")")
done

out=$outdir/baseline-$(hostname -s)-$fork_rev-$input_rev.json
merge=""
[ -f "$out" ] && merge=$out
# Run counts scale to per-run cost: the full sweep is ~13 min a run on a
# laptop, where 5 runs buys variance data worth less than the 40 extra
# minutes; the second run already gives a warm-cache mean.
case $BENCH in
  b2-nixpkgs-sweep) hf_runs=(--warmup 0 --runs 2) ;;
  b1-toplevel)      hf_runs=(--warmup 1 --runs 3) ;;
  *)                hf_runs=(--warmup 2 --runs 5) ;;
esac
if ! hyperfine "${hf_runs[@]}" --export-json "$out.new" "${hf_args[@]}"; then
  # A benchmark whose command failed must not leave a baseline behind:
  # rc=0 plus a WROTE line here once reported a failed run as captured.
  rm -f "$out.new"
  echo "eval-bench: hyperfine failed for $BENCH; no baseline written" >&2
  exit 1
fi
if [ -n "$merge" ]; then
  jq -s '{results: (map(.results) | add)}' "$merge" "$out.new" > "$out.merged" \
    && mv "$out.merged" "$out" && rm "$out.new"
else
  mv "$out.new" "$out"
fi
echo "WROTE $out"
jq -r '.results[] | "\(.command | if length > 80 then .[0:80]+"..." else . end)  mean=\(.mean | .*1000 | round / 1000)s"' "$out"
