#!/usr/bin/env bash
# The divergence id must be the same from two checkouts of the same tree.
#
#   shadow-id-portable.sh NIXBINDIR
#
# `eval-backend = shadow` reports each divergence with a 12-hex id whose whole
# purpose is grouping: a fleet query counts one finding as one row however many
# hosts hit it. That only works if the id is a function of the finding and not
# of where the tree happens to sit on disk.
#
# It was not. The id hashed the absolute path, so one lang-corpus divergence
# was `bc45769e3203` from a macOS worktree and `03c08a51a0bb` from a Linux
# checkout of the identical revision -- one finding, one row per host, which is
# the failure the stable-token vocabulary exists to prevent, reproduced one
# layer up. Found by running the corpus on two machines and diffing the ids,
# which is a thing nobody does by accident; hence this gate.
#
# Same expression, same divergence, two directories. Exit 0 iff one id.
set -u

# shellcheck source=./arm-config.sh
. "$(cd "$(dirname "$0")" && pwd)/arm-config.sh" || exit 2
# One owner of the gates' nix configuration, before anything reads the
# environment: an ambient `lint-url-literals = fatal` otherwise makes every
# rust arm refuse and every row score `unimplemented` (ENG-12996).
arm_pin_environment

bindir=${1:-}
[ -n "$bindir" ] || { echo "usage: shadow-id-portable.sh NIXBINDIR" >&2; exit 2; }
bindir=$(cd "$bindir" && pwd) || exit 2
instantiate=$bindir/nix-instantiate
[ -x "$instantiate" ] || { echo "shadow-id-portable: no nix-instantiate at $instantiate" >&2; exit 2; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# A divergence whose *text* carries no path, which is the whole subtlety here.
#
# The first draft used `builtins.unsafeGetAttrPos`, and it failed: that
# builtin's value is a position, so the C++ answer embeds the absolute file
# name and the id moves with the directory however portable the origin field
# is. That is not a bug in the id, it is a real residual limitation worth
# knowing -- a divergence whose *value* contains an absolute path is
# inherently per-machine, and no amount of care in the origin fixes it. What
# this gate can and must check is that the id does not move for a divergence
# whose content is machine-independent.
#
# Stack overflow is that: both arms produce the same words, and only the
# exception class differs (`error-class-lost`, ENG-12820).
expr='let f = n: f (n + 1); in f 0'

config="extra-experimental-features = rust-eval
eval-backend = shadow
$(arm_base_config)"

ids=()
for dir in "$work/alpha" "$work/beta/deeper/still"; do
  mkdir -p "$dir"
  printf '%s\n' "$expr" > "$dir/probe.nix"
  ( cd "$dir" && NIX_CONFIG="$config" "$instantiate" --eval --strict probe.nix > "$work/out" 2> "$work/err" )
  id=$(grep -oE 'id=[0-9a-f]{12}' "$work/err" | head -1 | cut -d= -f2)
  if [ -z "$id" ]; then
    echo "shadow-id-portable: no divergence reported from $dir; this gate cannot" >&2
    echo "  measure id stability without one, so this is a refusal and not a pass." >&2
    sed -e 's/^/  /' "$work/err" >&2
    exit 2
  fi
  echo "  $dir -> $id"
  ids+=("$id")
done

if [ "${ids[0]}" != "${ids[1]}" ]; then
  echo "shadow-id-portable: the same divergence got two ids, ${ids[0]} and ${ids[1]}," >&2
  echo "  so a fleet query grouping by id reports one finding as one row per host." >&2
  exit 1
fi
echo "RESULT one divergence, two directories, one id: ${ids[0]}"
