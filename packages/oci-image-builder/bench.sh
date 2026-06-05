#!/usr/bin/env bash
# Cold/warm benchmark for the per-layer describe path against the eager OCI
# one-shot. Measures the Rust engine directly on one `conf.json`, so the numbers
# are the builder's cost with no Nix-eval overhead. Run from a repo checkout on
# an x86_64-linux host:
#
#   packages/oci-image-builder/bench.sh [flake-image-attr] [runs]
#
# Default image attr is `non-nix-ubuntu`. Needs `nix` and pulls `hyperfine`.
#
# Definitions:
#   OCI one-shot     the eager path: plan -> full OCI tar in one process. It is a
#                    single derivation, so any change re-tars the whole image;
#                    cold and warm are the same.
#   NEW cold         build the durable image.json from scratch: describe every
#                    layer, the base, then stitch. (Nix runs the per-layer
#                    describes in parallel, so the real cold wall-clock is lower
#                    than this sequential sum.)
#   NEW warm         one store path changed: re-tar only its layer, then stitch.
#                    The other layers and the base are derivation cache hits.
#   materialize      regenerate the full OCI tar from image.json. Unchanged by
#                    this work; only needed when real bytes are required (push).
set -euo pipefail

IMAGE="${1:-non-nix-ubuntu}"
RUNS="${2:-3}"

nb() { nix build --no-link --print-out-paths "$@" 2>/dev/null; }

engine="$(nb ".#oci-image-builder")/bin/oci-image-builder"
conf="$(nb ".#${IMAGE}.passthru.stream.passthru.conf")"
archive="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["from_image"])' "$conf")"
mapfile -t layers < <(python3 -c \
  'import json,sys;print("\n".join(" ".join(g) for g in json.load(open(sys.argv[1]))["store_layers"]))' "$conf")
n="${#layers[@]}"

echo "image=$IMAGE  store_layers=$n  base=$(basename "$archive") ($(stat -c%s "$archive") bytes)"

cache="$(mktemp -d)"
work="$(mktemp -d)"
trap 'rm -rf "$cache" "$work"' EXIT

# Warm cache: the base description and one description per store layer, i.e. the
# derivation outputs Nix would already have. Also find the biggest layer to use
# as the worst-case "changed" layer.
"$engine" base-desc "$archive" "$cache/base.json" >/dev/null
descs=(); biggest=""; biggest_size=0; biggest_index=0
for i in "${!layers[@]}"; do
  "$engine" layer-desc --uid 0 --gid 0 --mtime 1 "$cache/layer-$i.json" ${layers[$i]} >/dev/null
  descs+=("$cache/layer-$i.json")
  size="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["size"])' "$cache/layer-$i.json")"
  if [ "$size" -gt "$biggest_size" ]; then biggest_size="$size"; biggest="${layers[$i]}"; biggest_index="$i"; fi
done
"$engine" assemble-desc --base "$cache/base.json" "$conf" "$cache/image.json" "${descs[@]}" >/dev/null
echo "biggest store layer = $biggest_size bytes (layer $biggest_index)"

cold="\"$engine\" base-desc \"$archive\" \"$work/b.json\""
for i in $(seq 0 $((n - 1))); do
  cold="$cold; \"$engine\" layer-desc --uid 0 --gid 0 --mtime 1 \"$work/l$i.json\" ${layers[$i]} >/dev/null"
done
cold="$cold; \"$engine\" assemble-desc --base \"$work/b.json\" \"$conf\" \"$work/img.json\" ${descs[*]}"
warm="\"$engine\" layer-desc --uid 0 --gid 0 --mtime 1 \"$cache/layer-$biggest_index.json\" $biggest"
warm="$warm; \"$engine\" assemble-desc --base \"$cache/base.json\" \"$conf\" \"$work/warm.json\" ${descs[*]}"

for run in $(seq 1 "$RUNS"); do
  echo
  echo "################## run $run/$RUNS ##################"
  nix run nixpkgs#hyperfine -- --warmup 2 --runs 10 \
    --command-name "OCI one-shot conf->tar (cold==warm)"   "\"$engine\" --skip-efficiency-check \"$conf\" \"$work/oci.tar\"" \
    --command-name "NEW cold: describe all -> image.json"  "bash -c '$cold'" \
    --command-name "NEW warm: 1 layer -> image.json"       "bash -c '$warm'" \
    --command-name "materialize image.json -> tar"         "\"$engine\" materialize \"$cache/image.json\" \"$work/m.tar\""
done
