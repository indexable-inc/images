#!/usr/bin/env bash

source common.sh

# shellcheck disable=SC1111
needLocalStore "“min-free” and “max-free” are daemon options"

TODO_NixOS

clearStore

# shellcheck disable=SC2034
garbage1=$(nix store add-path --name garbage1 ./nar-access.sh)
# shellcheck disable=SC2034
garbage2=$(nix store add-path --name garbage2 ./nar-access.sh)
# shellcheck disable=SC2034
garbage3=$(nix store add-path --name garbage3 ./nar-access.sh)

ls -l "$garbage3"
POSIXLY_CORRECT=1 du "$garbage3"

fake_free=$TEST_ROOT/fake-free
export _NIX_TEST_FREE_SPACE_FILE=$fake_free
echo 1100 > "$fake_free"

fifoLock=$TEST_ROOT/fifoLock
mkfifo "$fifoLock"

expr=$(cat <<EOF
with import ${config_nix}; mkDerivation {
  name = "gc-A";
  buildCommand = ''
    set -x
    [[ \$(ls \$NIX_STORE/*-garbage? | wc -l) = 3 ]]

    mkdir \$out
    echo foo > \$out/bar

    # Pretend that we run out of space
    echo 100 > ${fake_free}.tmp1
    mv ${fake_free}.tmp1 $fake_free

    # Wait for the GC to run
    for i in {1..20}; do
        echo ''\${i}...
        if [[ \$(ls \$NIX_STORE/*-garbage? | wc -l) = 1 ]]; then
            exit 0
        fi
        sleep 1
    done
    exit 1
  '';
}
EOF
)

expr2=$(cat <<EOF
with import ${config_nix}; mkDerivation {
  name = "gc-B";
  buildCommand = ''
    set -x
    mkdir \$out
    echo foo > \$out/bar

    # Wait for the first build to finish
    cat "$fifoLock"
  '';
}
EOF
)

nix build --impure -v -o "$TEST_ROOT"/result-A -L --expr "$expr" \
    --min-free 1K --max-free 2K --min-free-check-interval 1 &
pid1=$!

nix build --impure -v -o "$TEST_ROOT"/result-B -L --expr "$expr2" \
    --min-free 1K --max-free 2K --min-free-check-interval 1 &
pid2=$!

# Once the first build is done, unblock the second one.
# If the first build fails, we need to postpone the failure to still allow
# the second one to finish
wait "$pid1" || FIRSTBUILDSTATUS=$?
echo "unlock" > "$fifoLock"
( exit "${FIRSTBUILDSTATUS:-0}" )
wait "$pid2"

[[ foo = $(cat "$TEST_ROOT"/result-A/bar) ]]
[[ foo = $(cat "$TEST_ROOT"/result-B/bar) ]]

# Two processes can both decide to auto-GC before either acquires the global
# lock. Once the first collector restores free space, the queued process must
# recheck the threshold instead of starting another collection from its stale
# decision.
clearStore
echo 3000 > "$fake_free"
garbage=$(nix store add-path --name convoy-garbage ./nar-access.sh)
test -e "$garbage"

echo 100 > "$fake_free"
gcLock=$TEST_ROOT/gc-lock.fifo
mkfifo "$gcLock"
firstInput=$TEST_ROOT/convoy-first
secondInput=$TEST_ROOT/convoy-second
echo first > "$firstInput"
echo second > "$secondInput"
firstResult=$TEST_ROOT/convoy-first.result
secondResult=$TEST_ROOT/convoy-second.result
firstLog=$TEST_ROOT/convoy-first.log
secondLog=$TEST_ROOT/convoy-second.log

_NIX_TEST_GC_SYNC_2=$gcLock nix store add-path --name convoy-first "$firstInput" -v \
    --min-free 1K --max-free 2K --min-free-check-interval 0 \
    > "$firstResult" 2> "$firstLog" &
firstPid=$!

# Opening the write end proves that the first collector holds gc.lock and is
# blocked at _NIX_TEST_GC_SYNC_2. Keep the descriptor open while the second
# process reaches the lock queue.
exec 3> "$gcLock"
nix store add-path --name convoy-second "$secondInput" -v \
    --min-free 1K --max-free 2K --min-free-check-interval 0 \
    > "$secondResult" 2> "$secondLog" 3>&- &
secondPid=$!

for _ in {1..100}; do
    if grepQuiet -F "waiting for the big garbage collector lock" "$secondLog"; then
        break
    fi
    kill -0 "$secondPid" || fail "second auto-GC exited before waiting for gc.lock"
    sleep 0.1
done
grepQuiet -F "waiting for the big garbage collector lock" "$secondLog"

echo 3000 > "$fake_free.tmp"
mv "$fake_free.tmp" "$fake_free"
exec 3>&-

wait "$firstPid"
wait "$secondPid"
test -e "$(cat "$firstResult")"
test -e "$(cat "$secondResult")"
grepQuiet -F "skipping auto-GC because" "$secondLog"
grepQuietInverse -F "running auto-GC to free" "$secondLog"

# A synchronous auto-GC caller waits on a detached collector thread. When the
# process is interrupted, the collector must leave a blocked gc.lock wait so it
# can fulfill that future and let the caller release any locks it already owns.
clearStore
echo 3000 > "$fake_free"
interruptGarbage=$(nix store add-path --name interrupt-garbage ./nar-access.sh)
test -e "$interruptGarbage"

echo 100 > "$fake_free"
interruptLock=$TEST_ROOT/interrupt-gc-lock.fifo
mkfifo "$interruptLock"
holderInput=$TEST_ROOT/interrupt-holder
victimInput=$TEST_ROOT/interrupt-victim
echo holder > "$holderInput"
echo victim > "$victimInput"
holderLog=$TEST_ROOT/interrupt-holder.log
victimLog=$TEST_ROOT/interrupt-victim.log

_NIX_TEST_GC_SYNC_2=$interruptLock nix store add-path --name interrupt-holder "$holderInput" -v \
    --min-free 1K --max-free 2K --min-free-check-interval 0 \
    > "$TEST_ROOT/interrupt-holder.result" 2> "$holderLog" &
holderPid=$!

# Opening the writer proves the holder has acquired gc.lock and reached the
# synchronization point inside collectGarbage().
exec 4> "$interruptLock"
nix store add-path --name interrupt-victim "$victimInput" -v \
    --min-free 1K --max-free 2K --min-free-check-interval 0 \
    > "$TEST_ROOT/interrupt-victim.result" 2> "$victimLog" &
victimPid=$!

for _ in {1..100}; do
    if grepQuiet -F "waiting for the big garbage collector lock" "$victimLog"; then
        break
    fi
    kill -0 "$victimPid" || fail "victim exited before waiting for gc.lock"
    sleep 0.1
done
grepQuiet -F "waiting for the big garbage collector lock" "$victimLog"

kill -TERM "$victimPid"
for _ in {1..100}; do
    if ! kill -0 "$victimPid" 2> /dev/null; then
        break
    fi
    sleep 0.1
done

if kill -0 "$victimPid" 2> /dev/null; then
    kill -KILL "$victimPid"
    wait "$victimPid" || true
    exec 4>&-
    wait "$holderPid"
    fail "interrupted auto-GC stayed blocked on gc.lock"
fi

if wait "$victimPid"; then
    fail "interrupted auto-GC exited successfully"
fi
exec 4>&-
wait "$holderPid"
