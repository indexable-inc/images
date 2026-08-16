#!/usr/bin/env bash

# Regression test: a mid-build garbage collection (e.g. the min-free
# auto-GC) must not delete the freshly built output of a CA derivation
# that is only referenced by still-queued downstream (resolved)
# derivations. See https://github.com/indexable-inc/index/issues/2334.

source common.sh

# Blocks rather than fails when the store doing the work is a daemon that does
# not carry this fork's patch, so it must not run in the daemon-compat lanes.
# Here the GC is performed by whichever store the client talks to, and the
# temp root that keeps the in-flight CA output alive is this fork's addition, so
# without it `nix-store --gc` and the `echo go > "$fifo"` that follows never
# return.
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

fifo="$TEST_ROOT/gc-during-build.fifo"
rm -f "$fifo"
mkfifo "$fifo"

# A fresh seed forces a rebuild (and fresh CA output content) even on a
# dirty store (e.g. when the store could not be cleared).
seed="$RANDOM$RANDOM$$"

# Start the top-level build in the background. `dep` builds immediately;
# `blocker` parks on the fifo, so `top` stays queued.
nix build -f gc-during-build.nix top --no-link \
    --argstr seed "$seed" --argstr fifo "$fifo" \
    --max-jobs 2 &
buildPid=$!
# shellcheck disable=SC2064
trap "kill $buildPid 2>/dev/null || true" EXIT

# Wait until dep's output is built and registered as valid. The final
# content-addressed path is only known after the build, so find it by
# name and by this run's content.
depOut=""
for _ in $(seq 300); do
    for p in "$NIX_STORE_DIR"/*-gc-during-build-dep; do
        [[ -e "$p/content" ]] || continue
        if grep -qx "dep-$seed" "$p/content" 2>/dev/null \
            && nix path-info "$p" >/dev/null 2>&1; then
            depOut=$p
            break 2
        fi
    done
    sleep 0.2
done
[[ -n "$depOut" ]]
nix path-info "$depOut"

# Run the GC while the build is still in flight. Without a temp root on
# the freshly built (dynamic) output path, this used to delete it.
nix-store --gc

# The dep output must have survived the GC.
nix path-info "$depOut"
[[ -e "$depOut/content" ]]

# Unblock the rest of the build; it must now succeed.
echo go > "$fifo"
wait $buildPid
trap - EXIT

# And the final output must actually exist and contain both parts.
topOut=$(nix build -f gc-during-build.nix top --no-link --print-out-paths \
    --argstr seed "$seed" --argstr fifo "$fifo")
grep -q "dep-$seed" "$topOut/content"
grep -q "blocker-$seed" "$topOut/content"
