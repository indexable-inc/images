#!/usr/bin/env bash
#
# Measure what `write-through-store` costs a build.
#
# The number this produces is the one that decides build concurrency on a host
# with the setting on. Publication runs on the worker thread and blocks the
# build loop, so a dispatcher's usable parallelism is bounded by publication
# throughput rather than by `--max-jobs`, and that bound is not knowable from
# the derivation count.
#
# Run it against a scratch destination, never a production namespace.
#
# Always set `compression` explicitly on the destination URL. It dominates every
# other term by two orders of magnitude, measured on dev-compute-5 publishing a
# 256 MiB incompressible output to a local file:// cache:
#
#   compression=xz (the default!)    2.6 MiB/s
#   compression=zstd               579   MiB/s
#   compression=none               621   MiB/s
#
# A destination URL that does not say otherwise gets xz, single threaded, on the
# build worker thread.
#
#   ./write-through-throughput.sh --to "file:///tmp/wt-bench-cache"
#   ./write-through-throughput.sh --to "s3://scratch-bucket?endpoint=..." --sizes 1,64,256 --reps 5
#
# Everything happens in a scratch store root under --root, so the host's own
# store is untouched and the whole run can be deleted afterwards.

set -euo pipefail

nixBin="${NIX_BIN:-nix}"
dst=""
sizesArg="1,16,128,512"
reps=3
root="/tmp/wt-throughput"
out=""
payload="random"

usage() {
    cat >&2 <<'USAGE'
usage: write-through-throughput.sh --to <store-url> [options]

  --to <url>      destination store, e.g. file:///tmp/c or s3://bucket?endpoint=...
  --sizes <list>  comma-separated output sizes in MiB (default: 1,16,128,512)
  --reps <n>      repetitions per size (default: 3)
  --root <dir>    scratch store root (default: /tmp/wt-throughput)
  --payload <k>   output content: `random` (default), `text`, or `elf`
  --out <file>    write JSON results here (default: <root>/results.json)
  --nix <path>    nix binary to measure (default: $NIX_BIN, or `nix` on PATH)

Pick the payload to match the question:

  random  incompressible. The compressor finds nothing and gives up fast, so
          this measures transfer and protocol, not compression. Use it to find
          the wire or disk floor.
  text    random characters from a 63-symbol alphabet, so about 1.33x under
          zstd. Some real work, but a weak signal for thread scaling.
  elf     a real linked binary repeated to size, which is what build outputs
          mostly are, and compresses like one. Use this to measure whether
          thread count helps: against `random` the compressor finds nothing and
          the curve is flat for a reason that has nothing to do with threading.

Report the nix revision you measured, not its version string: a version string
does not distinguish two builds of 2.34.7.
USAGE
    exit 2
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --to) dst="${2:?--to needs a value}"; shift 2 ;;
        --sizes) sizesArg="${2:?--sizes needs a value}"; shift 2 ;;
        --reps) reps="${2:?--reps needs a value}"; shift 2 ;;
        --root) root="${2:?--root needs a value}"; shift 2 ;;
        --payload) payload="${2:?--payload needs a value}"; shift 2 ;;
        --out) out="${2:?--out needs a value}"; shift 2 ;;
        --nix) nixBin="${2:?--nix needs a value}"; shift 2 ;;
        -h|--help) usage ;;
        *) echo "unknown argument: $1" >&2; usage ;;
    esac
done

[[ -n $dst ]] || usage
case "$payload" in
    random | text | elf) ;;
    *) echo "unknown payload: $payload (want random or text)" >&2; usage ;;
esac
[[ -n $out ]] || out="$root/results.json"

command -v "$nixBin" > /dev/null || { echo "no such nix binary: $nixBin" >&2; exit 1; }

# A scratch store, so this never writes to the host's own store and the builds
# are guaranteed to be real rather than already-valid no-ops.
rm -rf "$root"
mkdir -p "$root"/store "$root"/var/nix "$root"/var/log/nix "$root"/etc "$root"/home
export NIX_STORE_DIR="$root/store"
export NIX_LOCALSTATE_DIR="$root/var"
export NIX_LOG_DIR="$root/var/log/nix"
export NIX_STATE_DIR="$root/var/nix"
export NIX_CONF_DIR="$root/etc"
export NIX_REMOTE=""
export HOME="$root/home"
unset NIX_PATH NIX_USER_CONF_FILES XDG_CONFIG_HOME XDG_CACHE_HOME 2> /dev/null || true

cat > "$root/etc/nix.conf" <<'CONF'
experimental-features = nix-command
sandbox = false
build-users-group =
substituters =
CONF

bashExe=$(readlink -f "$(command -v bash)")
coreutilsBin=$(dirname "$(readlink -f "$(command -v dd)")")
trBin=$(dirname "$(readlink -f "$(command -v tr)")")
elfSrc=$(readlink -f "$(command -v "$nixBin")")

# `$out` is deliberately left unexpanded here: it is the builder's variable,
# substituted when the derivation runs, not this script's.
# shellcheck disable=SC2016
case "$payload" in
    random) makeBlob='dd if=/dev/urandom of="$out/blob" bs=1M count=MIB status=none' ;;
    # A small alphabet, so the bytes are still unpredictable but highly
    # compressible, which is what makes thread count observable.
    # A real binary, repeated to the requested size. zstd's window is far
    # smaller than the source file, so repeating does not degenerate into
    # long-range dedup and the ratio stays representative of a build output.
    elf) makeBlob='while [ "$(stat -c%s "$out/blob" 2>/dev/null || echo 0)" -lt $((MIB*1048576)) ]; do cat ELFSRC >> "$out/blob"; done; truncate -s $((MIB*1048576)) "$out/blob"' ;;
    text) makeBlob='tr -dc "a-zA-Z0-9 \n" < /dev/urandom | dd of="$out/blob" bs=1M count=MIB iflag=fullblock status=none' ;;
esac

# `dd` from /dev/urandom, so the payload does not compress. Measuring against
# zeroes would report a throughput the wire will never deliver.
#
# The run id makes every build produce a store path that has never existed.
# Without it the second and later reps would find the path already valid on the
# destination, `copyPaths` would skip, and the harness would report an enormous
# throughput for having transferred nothing.
writeDrv() {
    local file="$1" mib="$2" runId="$3"
    local blob=${makeBlob//MIB/$mib}
    blob=${blob//ELFSRC/$elfSrc}
    cat > "$file" <<NIXEXPR
derivation {
  system = builtins.currentSystem;
  name = "wt-throughput-${payload}-${mib}m-${runId}";
  builder = "$bashExe";
  args = [
    "-e"
    "-c"
    ''
      export PATH="$coreutilsBin:$trBin"
      mkdir "\$out"
      ${blob}
    ''
  ];
}
NIXEXPR
}

# Seconds as a float, from a monotonic-enough source.
now() { date +%s.%N; }

elapsed() { echo "$1 $2" | awk '{printf "%.3f", $2 - $1}'; }

median() {
    # stdin: one number per line
    sort -g | awk '{a[NR]=$1} END {if (NR==0) {print "0"} else if (NR%2) {print a[(NR+1)/2]} else {printf "%.3f", (a[NR/2]+a[NR/2+1])/2}}'
}

build() {
    # build <expr-file> [extra args...]; prints the out path
    local file="$1"; shift
    "$nixBin" build --impure --file "$file" --no-link --print-out-paths "$@"
}

IFS=',' read -r -a sizes <<< "$sizesArg"

rev="unknown"
if git -C "$(dirname "$0")" rev-parse HEAD > /dev/null 2>&1; then
    rev=$(git -C "$(dirname "$0")" rev-parse HEAD)
fi

echo "destination: $dst"
echo "nix binary:  $nixBin"
echo "resolved to: $(readlink -f "$(command -v "$nixBin")")"
echo "harness rev: $rev"
echo "host:        $(hostname)"
echo "scratch:     $root"
echo

printf '%-8s %-10s %-12s %-12s %-14s %s\n' \
    "sizeMiB" "narBytes" "offSec" "onSec" "publishSec" "logicalMiBps"

results=""
for mib in "${sizes[@]}"; do
    offTimes=""
    onTimes=""
    narBytes=0
    wireBytes=0

    for ((r = 1; r <= reps; r++)); do
        # Baseline: identical work, publication off.
        writeDrv "$root/off-$mib-$r.nix" "$mib" "off-$mib-$r-$$"
        t0=$(now)
        build "$root/off-$mib-$r.nix" > /dev/null
        t1=$(now)
        offTimes+="$(elapsed "$t0" "$t1")"$'\n'

        # Same work, publication on, against a path that has never existed.
        writeDrv "$root/on-$mib-$r.nix" "$mib" "on-$mib-$r-$$"
        t2=$(now)
        outPath=$(build "$root/on-$mib-$r.nix" --option write-through-store "$dst")
        t3=$(now)
        onTimes+="$(elapsed "$t2" "$t3")"$'\n'

        # A missing narinfo means nothing was published, and a publication that
        # did not happen would otherwise read as infinite throughput. Refuse.
        if ! "$nixBin" path-info --store "$dst" "$outPath" > /dev/null 2>&1; then
            echo "FATAL: $outPath is not on $dst after a build that should have published it" >&2
            exit 1
        fi

        narBytes=$("$nixBin" path-info -S "$outPath" | awk '{print $2}')

        # Wire bytes are only readable directly for a local binary cache. For
        # anything else report logical bytes and say so, rather than guessing.
        if [[ $dst == file://* ]]; then
            hashPart=$(basename "$outPath"); hashPart=${hashPart%%-*}
            narinfo="${dst#file://}/$hashPart.narinfo"
            if [[ -e $narinfo ]]; then
                wireBytes=$(awk -F': ' '/^FileSize: /{print $2}' "$narinfo")
            fi
        fi
    done

    offMedian=$(printf '%s' "$offTimes" | median)
    onMedian=$(printf '%s' "$onTimes" | median)
    publish=$(echo "$offMedian $onMedian" | awk '{d = $2 - $1; if (d < 0) d = 0; printf "%.3f", d}')
    mibps=$(echo "$narBytes $publish" | awk '{if ($2 > 0) printf "%.1f", ($1 / 1048576) / $2; else print "n/a"}')

    printf '%-8s %-10s %-12s %-12s %-14s %s\n' \
        "$mib" "$narBytes" "$offMedian" "$onMedian" "$publish" "$mibps"

    results+=$(printf '{"size_mib":%s,"nar_bytes":%s,"wire_bytes":%s,"off_median_s":%s,"on_median_s":%s,"publish_s":%s,"logical_mib_per_s":"%s","reps":%s},' \
        "$mib" "$narBytes" "${wireBytes:-0}" "$offMedian" "$onMedian" "$publish" "$mibps" "$reps")
done

cat > "$out" <<JSON
{
  "note": "write-through-store publication cost. publish_s is the median wall-clock delta between an identical build with the setting off and with it on, so it includes nix-copy protocol overhead as well as transfer. wire_bytes is only populated for file:// destinations.",
  "destination": "$dst",
  "payload": "$payload",
  "nix_binary": "$(readlink -f "$(command -v "$nixBin")")",
  "harness_rev": "$rev",
  "host": "$(hostname)",
  "date": "$(date -Is)",
  "results": [${results%,}]
}
JSON

echo
echo "wrote $out"
echo
echo "Reading these numbers:"
echo "  publish_s is a delta between two separate builds, so it carries build-time"
echo "  variance as well as publication cost. Raise --reps before believing a small"
echo "  number, and quote the spread alongside the median."
echo "  A file:// destination measures protocol overhead with the wire removed."
echo "  Only a run against the real endpoint measures the real thing."
