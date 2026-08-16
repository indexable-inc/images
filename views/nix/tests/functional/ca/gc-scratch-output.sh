#!/usr/bin/env bash

# Regression test: a garbage collection while a floating-CA derivation is
# building must not delete the builder's scratch output path. On non-chroot
# builds (sandbox = false) the builder writes $out directly at the scratch
# store path, which is not a valid path; without a temp root the GC treats
# it as garbage and the build later fails with "failed to produce output
# path". See https://github.com/indexable-inc/index/issues/2354.

source common.sh

# Blocks rather than fails when the store doing the work is a daemon that does
# not carry this fork's patch, so it must not run in the daemon-compat lanes.
# Same shape as gc-during-build.sh: the scratch-output temp root is this
# fork's addition, and without it the fifo handshake with the parked builder
# never completes.
#
# In run 30626908044 the compat suite stalled at 222/225 with this test among
# the three that never reported, and the job was killed at its 90-minute wall
# after 38 minutes of total silence. nixpkgs' mesonCheckPhase passes
# `--timeout-multiplier=0`, so meson's per-test `timeout: 300` is disabled and one
# blocked test costs the whole job instead of failing with its own name. It had
# never been seen because `Run flake checks and prepare the installer tarball`
# was skipped on every recent run by an earlier failing step.
#
# needLocalStore rather than requireDaemonNewerThan deliberately: a patched
# daemon might well satisfy these, but nobody has observed that, and a version
# compare would assert it. This states the configuration the test is known to
# work in.
needLocalStore "the GC under test must be performed by the store this test controls"

clearStoreIfPossible

fifo="$TEST_ROOT/gc-scratch-output.fifo"
rm -f "$fifo"
mkfifo "$fifo"

# A fresh seed forces a rebuild (and fresh scratch content) even on a
# dirty store.
seed="$RANDOM$RANDOM$$"

# Start the build in the background; the builder writes its scratch
# output, then parks on the fifo.
nix build -f gc-scratch-output.nix top --no-link \
    --argstr seed "$seed" --argstr fifo "$fifo" &
buildPid=$!
# shellcheck disable=SC2064
trap "kill $buildPid 2>/dev/null || true" EXIT

# Wait until the builder has written the scratch output. The scratch path
# has the derivation's name but a rewrite-fingerprint hash, so find it by
# name and by this run's content.
scratch=""
for _ in $(seq 300); do
    for p in "$NIX_STORE_DIR"/*-gc-scratch-output; do
        if grep -qx "scratch-$seed" "$p/c" 2>/dev/null; then
            scratch=$p
            break 2
        fi
    done
    sleep 0.2
done
[[ -n "$scratch" ]]

# Run the GC while the builder is parked. Without a temp root on the
# scratch path, this used to delete it.
nix-store --gc

# The scratch output must have survived the GC.
[[ -e "$scratch/c" ]]

# Unblock the builder; the build must now succeed.
echo go > "$fifo"
wait $buildPid
trap - EXIT

# The final output must exist and contain both writes.
out=$(nix build -f gc-scratch-output.nix top --no-link --print-out-paths \
    --argstr seed "$seed" --argstr fifo "$fifo")
grep -q "scratch-$seed" "$out/c"
grep -q 'done' "$out/c"
