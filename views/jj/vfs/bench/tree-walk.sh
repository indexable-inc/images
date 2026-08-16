#!/usr/bin/env bash
# Times a tree walk, a stat sweep and a full read through a jj mount, against the
# same tree on local disk.
#
# Committed rather than typed once, because the numbers in docs/vfs.md came out of
# it and a published number should be reproducible by whoever doubts it.
#
# It checks the tree it built before timing anything. That is not ceremony: the
# first run of this measurement used a shell that happened to lack python3, so the
# tree was never created and every timing came back 0.00s. A benchmark over an
# empty input returns plausible-looking numbers, and the only thing that caught it
# was having printed the file count. So the count is checked twice, once on disk
# and once through the mount, and the script refuses rather than reporting.
#
# Usage: vfs/bench/tree-walk.sh /path/to/jj [fuse|nfs]
set -uo pipefail

JJ=${1:?usage: tree-walk.sh /path/to/jj [fuse|nfs]}
TRANSPORT=${2:-fuse}

# A source-tree shape rather than a uniform one, and fixed so two runs measure the
# same bytes.
DIRS=60
PER_DIR=50
EXPECT_FILES=$((DIRS * PER_DIR))
SEED=7

work=$(mktemp -d)
repo="$work/repo"
mnt="$work/mnt"
# Output of the timed commands goes to a file rather than being discarded, so a
# command that failed instantly cannot masquerade as a fast one.
sink="$work/sink"
mkdir -p "$repo" "$mnt"

# Resolved once, before anything is mounted. On macOS $TMPDIR is a symlink, so the
# mount table shows the /private realpath while $mnt is the /var path; grepping for
# the unresolved name finds nothing and concludes "not mounted" about a live mount.
mnt_real=$(cd "$mnt" && pwd -P)

still_mounted() {
  mount | grep -qF " $mnt_real " || mount | grep -qF " $mnt "
}

cleanup() {
  # Order matters. Stop the server so it unmounts, then confirm, and only then
  # delete anything.
  if [ -n "${server:-}" ] && kill -0 "$server" 2>>"$work/cleanup.log"; then
    kill -TERM "$server" || true
  fi
  for _ in $(seq 1 120); do
    still_mounted || break
    sleep 0.5
  done
  if still_mounted; then
    umount "$mnt_real" || umount -f "$mnt_real" || true
  fi
  if still_mounted; then
    # Never recursively delete a path that is still a live mount: rm -rf would
    # walk into the mount and delete the served tree rather than the mountpoint.
    # An earlier version of this script did exactly that, and only survived
    # because the mount happened to be read-only.
    echo "WARNING: $mnt_real is still mounted. Leaving $work alone rather than" >&2
    echo "deleting through it. Unmount it and remove $work by hand." >&2
    return
  fi
  rm -rf "$repo"
  rm -f "$sink" "$work"/*.log
  # rmdir rather than rm -rf, because it cannot recurse: if the mountpoint is
  # somehow not empty this fails loudly instead of deleting a filesystem.
  rmdir "$mnt" || true
  rmdir "$work" || true
}
trap cleanup EXIT

export JJ_USER=bench JJ_EMAIL=bench@example.com
cd "$repo"
"$JJ" git init . > "$work/init.log" 2>&1

python3 - "$DIRS" "$PER_DIR" "$SEED" <<'PY'
import os, random, sys
dirs, per_dir, seed = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
random.seed(seed)
for d in range(dirs):
    os.makedirs(f"d{d:02d}", exist_ok=True)
    for f in range(per_dir):
        n = random.randint(1024, 65536)
        with open(f"d{d:02d}/f{f:02d}.txt", "w") as fh:
            fh.write("x" * n)
PY
"$JJ" describe -m bench >> "$work/init.log" 2>&1

# The denominator, checked before any timing happens.
files=$(find . -name '*.txt' -type f | wc -l | tr -d ' ')
# python rather than `find -printf`, which is GNU-only: on macOS it fails and the
# total silently comes back 0, which is exactly the plausible-looking zero this
# script exists to refuse.
bytes=$(python3 -c 'import os,sys; print(sum(os.path.getsize(os.path.join(r,f)) for r,_,fs in os.walk(".") for f in fs if f.endswith(".txt")))')
if [ "$files" -ne "$EXPECT_FILES" ]; then
  echo "REFUSING TO MEASURE: built $files files, expected $EXPECT_FILES." >&2
  echo "An empty or partial tree produces plausible timings that mean nothing." >&2
  exit 1
fi
echo "tree: $files files, $bytes bytes, transport $TRANSPORT"

"$JJ" fs mount -r @ --transport "$TRANSPORT" "$mnt" > "$work/mount.log" 2>&1 &
server=$!
for _ in $(seq 1 120); do
  still_mounted && break
  sleep 0.5
done
if ! still_mounted; then
  echo "REFUSING TO MEASURE: the mount never appeared." >&2
  cat "$work/mount.log" >&2
  exit 1
fi

# The same check through the mount. A mount serving an empty tree would otherwise
# time three very fast operations over nothing.
seen=$(find "$mnt" -type f | wc -l | tr -d ' ')
if [ "$seen" -ne "$EXPECT_FILES" ]; then
  echo "REFUSING TO MEASURE: the mount shows $seen files, expected $EXPECT_FILES." >&2
  exit 1
fi

time_ms() {
  local start end
  start=$(date +%s%N)
  eval "$1" > "$sink" 2>&1
  end=$(date +%s%N)
  printf '  %-34s %6d ms\n' "$2" $(( (end - start) / 1000000 ))
}

echo "--- through the mount ---"
time_ms "find $mnt -type f | wc -l"               "find -type f, names only"
time_ms "find $mnt -type f -size +0c | wc -l" "stat sweep (one stat per file)"
time_ms "find $mnt -type f -exec cat {} +"        "read every file"
echo "--- same tree on local disk ---"
time_ms "find $repo -name '*.txt' -type f | wc -l"               "find -type f, names only"
time_ms "find $repo -name '*.txt' -type f -size +0c | wc -l" "stat sweep (one stat per file)"
time_ms "find $repo -name '*.txt' -type f -exec cat {} +"        "read every file"

