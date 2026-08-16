#!/usr/bin/env bash

source common.sh

if [[ $(uname) != Linux ]]; then
    skipTest "daemon worker signal test requires procfs"
fi

if [[ -z ${NIX_DAEMON_PACKAGE:-} ]]; then
    skipTest "daemon worker signal test requires a test daemon"
fi

# The behavior asserted here is this fork's `fix(libstore): interrupt blocked
# automatic GC`: a worker parked in an automatic GC must still act on SIGTERM. A
# released daemon does not, so the test's 10s wait expires and it reports "daemon
# worker ignored SIGTERM" as a failure when the honest answer is that this daemon
# never had the fix. Run 30636844197, daemon 2.32.4, main/daemon-signal FAIL at
# daemon-signal.sh:66.
#
# Unlike the other guards added alongside this one, needLocalStore is not an option
# here: this test requires a daemon by construction, which is what it is measuring.
requireDaemonNewerThan "2.34.7"

clientPid=
workerPid=

cleanup() {
    if [[ -n $workerPid && -e /proc/$workerPid ]]; then
        kill -KILL "$workerPid" || true
    fi
    if [[ -n $clientPid ]] && kill -0 "$clientPid" 2> /dev/null; then
        kill -KILL "$clientPid" || true
    fi
    killDaemon
}
trap cleanup EXIT

clientLog=$TEST_ROOT/daemon-signal-client.log
expr=$(cat <<EOF
with import ${config_nix}; mkDerivation {
  name = "daemon-signal";
  buildCommand = ''
    echo daemon-signal-ready >&2
    sleep 300
  '';
}
EOF
)

nix build --impure --no-link -L --expr "$expr" > "$clientLog" 2>&1 &
clientPid=$!

for _ in {1..100}; do
    if grepQuiet -F "daemon-signal-ready" "$clientLog"; then
        break
    fi
    kill -0 "$clientPid" || fail "client exited before its build started"
    sleep 0.1
done
grepQuiet -F "daemon-signal-ready" "$clientLog"

workerPids=$(<"/proc/$_NIX_TEST_DAEMON_PID/task/$_NIX_TEST_DAEMON_PID/children")
read -r -a workers <<< "$workerPids"
if [[ ${#workers[@]} -ne 1 ]]; then
    fail "expected one daemon worker, found: ${workers[*]}"
fi
workerPid=${workers[0]}

kill -TERM "$workerPid"
for _ in {1..100}; do
    if [[ ! -e /proc/$workerPid ]]; then
        break
    fi
    sleep 0.1
done

[[ ! -e /proc/$workerPid ]] || fail "daemon worker ignored SIGTERM"
workerPid=
wait "$clientPid" || true
clientPid=
