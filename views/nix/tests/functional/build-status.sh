#!/usr/bin/env bash

source common.sh

# The build status directory is daemon-independent shared state on the
# filesystem, so this test exercises the direct (non-daemon) store: it starts a
# sleeping build and checks that `nix store builds` sees it without connecting
# to a daemon.
TODO_NixOS

# The header above already says this test exercises the DIRECT store: it reads
# the build status directory from the filesystem without connecting to a daemon.
# That was a description, not a guard, so the daemon-compat lanes ran it anyway
# and it blocked there, one of three tests that left run 30626908044 silent for
# 38 minutes before its 90-minute wall. Meson's per-test timeout is disabled by
# nixpkgs' `--timeout-multiplier=0`, so a block costs the whole job.
needLocalStore "the build status directory is read straight off the filesystem, with no daemon in the path"

enableFeatures "build-status-dir"

clearStore

nixExpr=build-status.nix

drvPath=$(nix-instantiate "$nixExpr")
echo "derivation is $drvPath"

# Start the (sleeping) build in the background.
nix build --file "$nixExpr" --no-link &
buildPid=$!

cleanup() {
    kill "$buildPid" 2> /dev/null || true
    wait "$buildPid" 2> /dev/null || true
}
trap cleanup EXIT

# The status file is written exactly when the build starts doing real work, so
# poll `nix store builds` until our build shows up (or time out).
found=
for _ in $(seq 1 100); do
    if nix store builds --json | jq -e --arg drv "$drvPath" \
        'any(.[]; .type == "build" and (.drvPath | endswith("build-status-sleeper.drv")))' > /dev/null; then
        found=1
        break
    fi
    sleep 0.2
done
[[ -n $found ]]

# Grab the entry and assert its shape.
entry=$(nix store builds --json | jq --arg drv "$drvPath" \
    'map(select(.drvPath | endswith("build-status-sleeper.drv"))) | .[0]')
echo "$entry" | jq .

[[ $(echo "$entry" | jq -r '.type') == build ]]
[[ $(echo "$entry" | jq -r '.storePath') == null ]]
[[ $(echo "$entry" | jq -r '.drvPath') == *build-status-sleeper.drv ]]
# The why-chain must be non-empty and end at this derivation.
[[ $(echo "$entry" | jq '.why.chain | length') -ge 1 ]]
[[ $(echo "$entry" | jq -r '.why.chain | last') == *build-status-sleeper.drv ]]
# The worker pid must be set and refer to a live process. The direct local
# store has no daemon peer, so it has no client pid to report.
pid=$(echo "$entry" | jq -r '.pid')
[[ -n $pid && $pid != null ]]
kill -0 "$pid"
[[ $(echo "$entry" | jq -r '.clientPid') == null ]]

# Stop the build; the status directory must drain.
cleanup
trap - EXIT

# The worker removes its status file on exit, so the directory should now be
# empty (`nix store builds` also prunes any stale files as it reads).
empty=
for _ in $(seq 1 100); do
    if [[ $(nix store builds --json | jq 'length') == 0 ]]; then
        empty=1
        break
    fi
    sleep 0.2
done
[[ -n $empty ]]

# A daemon connection records the requesting client pid. This is the stable
# correlation key used by per-derivation liveness monitors when many clients
# owned by the same uid build concurrently.
daemonStarted=
if ! isTestOnNixOS; then
    startDaemon
    daemonStarted=1
    nix build --file "$nixExpr" --no-link &
    buildPid=$!
    found=
    for _ in $(seq 1 100); do
        if nix store builds --json | jq -e --argjson clientPid "$buildPid" \
            'any(.[]; .clientPid == $clientPid)' > /dev/null; then
            found=1
            break
        fi
        sleep 0.2
    done
    [[ -n $found ]]
    cleanup

    # A SIGKILLed client (it cannot say goodbye on the socket) must have the
    # same effect: the daemon worker has to notice the disconnect on its own
    # and abort the goal, its builder, and its status entry, even though the
    # builder is completely silent and no timeout is armed (index#3752).
    nix build --file build-status-liveness.nix --argstr mode disconnect --no-link &
    buildPid=$!
    handlerPid=
    for _ in $(seq 1 100); do
        handlerPid=$(nix store builds --json | jq -r --argjson clientPid "$buildPid" \
            '.[] | select(.clientPid == $clientPid) | .pid' | head -1)
        [[ -n $handlerPid ]] && break
        sleep 0.2
    done
    [[ -n $handlerPid ]]
    kill -9 "$buildPid"
    wait "$buildPid" || true
    drained=
    for _ in $(seq 1 100); do
        if ! kill -0 "$handlerPid" 2> /dev/null; then
            drained=1
            break
        fi
        sleep 0.2
    done
    [[ -n $drained ]]
    # Its status entry must be gone with it.
    emptied=
    for _ in $(seq 1 100); do
        if nix store builds --json | jq -e --argjson clientPid "$buildPid" \
            'all(.[]; .clientPid != $clientPid)' > /dev/null; then
            emptied=1
            break
        fi
        sleep 0.2
    done
    [[ -n $emptied ]]

    # Disconnecting the client must cancel the daemon-owned goal and its
    # builder, not merely release the caller.
    nix build --file build-status-liveness.nix --argstr mode disconnect --no-link &
    buildPid=$!
    handlerPid=
    for _ in $(seq 1 100); do
        handlerPid=$(nix store builds --json | jq -r --argjson clientPid "$buildPid" \
            '.[] | select(.clientPid == $clientPid) | .pid' | head -1)
        [[ -n $handlerPid ]] && break
        sleep 0.2
    done
    [[ -n $handlerPid ]]
    kill "$buildPid"
    wait "$buildPid" || true
    drained=
    for _ in $(seq 1 100); do
        if ! kill -0 "$handlerPid" 2> /dev/null; then
            drained=1
            break
        fi
        sleep 0.2
    done
    [[ -n $drained ]]
fi

# The no-progress deadline reads per-process activity metrics from /proc, so
# startLiveness() refuses to arm it anywhere but Linux; only assert the
# deadline behaviour there.
if [[ $(uname) == Linux ]]; then
    # A silent idle builder expires, while a silent CPU-active builder continues.
    if nix build --file build-status-liveness.nix --argstr mode silent --no-link \
        --option max-no-progress-time 1 \
        --option big-parallel-max-no-progress-time 4 2> liveness.log; then
        fail "silent builder should hit max-no-progress-time"
    fi
    grepQuiet "timed out after 1 seconds" liveness.log
    grepQuiet "uninterruptible-processes=0" liveness.log

    nix build --file build-status-liveness.nix --argstr mode active --no-link \
        --option max-no-progress-time 1 \
        --option big-parallel-max-no-progress-time 4

    # `big-parallel` gets its explicit longer deadline.
    nix build --file build-status-liveness.nix --argstr mode silent \
        --arg bigParallel true --no-link \
        --option system-features big-parallel \
        --option max-no-progress-time 1 \
        --option big-parallel-max-no-progress-time 4
else
    # Non-Linux platforms must refuse the option loudly rather than build
    # without a deadline.
    if nix build --file build-status-liveness.nix --argstr mode active --no-link \
        --option max-no-progress-time 1 \
        --option big-parallel-max-no-progress-time 4 2> liveness.log; then
        fail "max-no-progress-time should be refused without Linux process metrics"
    fi
    grepQuiet "max-no-progress-time requires Linux process activity metrics" liveness.log
fi

[[ -z $daemonStarted ]] || killDaemon

# Staleness: an entry whose writer survives only as a zombie is a corpse and
# must not be reported (kill(pid, 0) still succeeds for zombies, which kept 33
# phantom builds "in flight" for 10.5 hours in the motivating incident).
statusDir=$TEST_ROOT/var/nix/status
mkdir -p "$statusDir"

# A process that leaves a never-reaped child: the inner sleep exits while its
# parent has exec'd into a long sleep and never calls wait(2).
zombiePidFile=$TEST_ROOT/zombie.pid
sh -c 'sh -c "exit 0" & echo $! > '"$zombiePidFile"'; exec sleep 60' &
keeperPid=$!
zombiePid=
for _ in $(seq 1 100); do
    [[ -s $zombiePidFile ]] && zombiePid=$(cat "$zombiePidFile") && break
    sleep 0.1
done
[[ -n $zombiePid ]]
# Read the state from /proc where available (the Linux sandbox has no full
# ps(1)); fall back to ps -o state= elsewhere (Darwin).
processState() {
    if [[ -r /proc/$1/stat ]]; then
        local stat
        stat=$(< "/proc/$1/stat")
        stat=${stat##*) }
        echo "${stat%% *}"
    else
        ps -o state= -p "$1" 2> /dev/null | tr -d ' '
    fi
}
isZombie=
state=
for _ in $(seq 1 100); do
    state=$(processState "$zombiePid" || true)
    if [[ $state == Z* ]]; then
        isZombie=1
        break
    fi
    sleep 0.1
done
if [[ -z $state ]]; then
    # macOS's seatbelt hides other processes from proc_info/sysctl inside the
    # sandboxed test run -- for the `ps` probe here and equally for the
    # KERN_PROC probe inside `nix store builds` -- so the zombie scenario is
    # unobservable: skip it rather than assert on what cannot be seen.
    echo "skipping zombie staleness check: process states not observable here"
else
    [[ -n $isZombie ]]

    cat > "$statusDir/zzzz-fake-zombie.drv-$zombiePid.json" << EOF
{"type": "build", "drvPath": "$NIX_STORE_DIR/zzzz-fake-zombie.drv", "pid": $zombiePid}
EOF
    nix store builds --json | jq -e --argjson pid "$zombiePid" 'all(.[]; .pid != $pid)'
    [[ ! -e "$statusDir/zzzz-fake-zombie.drv-$zombiePid.json" ]]
fi
kill "$keeperPid" 2> /dev/null || true

# Staleness: an entry that claims a liveness lock nobody holds is a corpse,
# even if its recorded pid is alive (pids get reused).
cat > "$statusDir/zzzz-fake-unlocked.drv-$$.json" << EOF
{"type": "build", "drvPath": "$NIX_STORE_DIR/zzzz-fake-unlocked.drv", "pid": $$, "livenessLock": true}
EOF
nix store builds --json | jq -e 'all(.[]; .drvPath | endswith("zzzz-fake-unlocked.drv") | not)'
[[ ! -e "$statusDir/zzzz-fake-unlocked.drv-$$.json" ]]

echo "build-status.sh: OK"
