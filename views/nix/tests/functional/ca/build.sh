#!/usr/bin/env bash

source common.sh

drv=$(nix-instantiate ./content-addressed.nix -A rootCA --arg seed 1)^out
nix derivation show "$drv" --arg seed 1

buildAttr () {
    local derivationPath=$1
    local seedValue=$2
    shift; shift
    local args=("./content-addressed.nix" "-A" "$derivationPath" --arg seed "$seedValue" "--no-out-link")
    args+=("$@")
    nix-build "${args[@]}"
}

testDeterministicCA () {
    [[ $(buildAttr rootCA 1) = $(buildAttr rootCA 2) ]]
}

testCutoffFor () {
    local out1 out2
    out1=$(buildAttr "$1" 1)
    # The seed only changes the root derivation, and not it's output, so the
    # dependent derivations should only need to be built once.
    buildAttr rootCA 2
    out2=$(buildAttr "$1" 2 -j0)
    test "$out1" == "$out2"
}

testCutoff () {
    # Don't directly build dependentCA, that way we'll make sure we don't rely on
    # dependent derivations always being already built.
    #testDerivation dependentCA
    testCutoffFor transitivelyDependentCA
    testCutoffFor dependentNonCA
    testCutoffFor dependentFixedOutput
}

testGC () {
    nix-instantiate ./content-addressed.nix -A rootCA --arg seed 5
    nix-collect-garbage --option keep-derivations true
    clearStore
    buildAttr rootCA 1 --out-link "$TEST_ROOT"/rootCA
    nix-collect-garbage
    buildAttr rootCA 1 -j0
}

testNixCommand () {
    clearStore
    nix build --file ./content-addressed.nix --no-link
}

testFailureDiagnostics () {
    clearStore

    local output status
    output=$(nix build -L --file ./failure-diagnostics.nix resolvedFailure --no-link 2>&1) && status=0 || status=$?
    test "$status" = 1
    grepQuiet "CA_FAILURE_FIRST" <<< "$output"
    test "$(grep -c "error: Cannot build '.*-ca-failing-first.drv'" <<< "$output")" = 1
    grepQuiet "ca-failing-first" <<< "$output"
    grepQuiet "exit code 42" <<< "$output"
    grepQuietInverse "build of resolved derivation" <<< "$output"

    clearStore
    output=$(nix build -L --file ./failure-diagnostics.nix failFast --no-link 2>&1) && status=0 || status=$?
    test "$status" = 1
    grepQuiet "CA_FAILURE_FIRST" <<< "$output"
    test "$(grep -c "error: Cannot build '.*-ca-failing-first.drv'" <<< "$output")" = 1
    grepQuiet "ca-failing-first" <<< "$output"
    grepQuiet "exit code 42" <<< "$output"
    grepQuietInverse "Reason: 1 dependency failed" <<< "$output"

    clearStore
    output=$(nix build -L --keep-going --file ./failure-diagnostics.nix keepGoing --no-link \
        --json-log-path "$TEST_ROOT/ca-failure.json" 2>&1) && status=0 || status=$?
    test "$status" = 1
    grepQuiet "CA_FAILURE_FIRST" <<< "$output"
    grepQuiet "CA_FAILURE_SECOND" <<< "$output"
    test "$(grep -c "error: Cannot build '.*-ca-failing-first.drv'" <<< "$output")" = 1
    test "$(grep -c "error: Cannot build '.*-ca-failing-second.drv'" <<< "$output")" = 1
    grepQuiet "ca-failing-first" <<< "$output"
    grepQuiet "ca-failing-second" <<< "$output"
    grepQuiet "CA_FAILURE_FIRST" "$TEST_ROOT/ca-failure.json"
    grepQuiet "CA_FAILURE_SECOND" "$TEST_ROOT/ca-failure.json"
    grepQuietInverse "build of resolved derivation" <<< "$output"
    grepQuietInverse "Reason: 2 dependencies failed" <<< "$output"
}

# Regression test for https://github.com/NixOS/nix/issues/4775
testNormalization () {
    clearStore
    outPath=$(buildAttr rootCA 1)
    test "$(stat -c %Y "$outPath")" -eq 1
}

clearStore
testNormalization
testDeterministicCA
clearStore
testCutoff
testGC
testNixCommand
# testFailureDiagnostics asserts the message shape this fork's `libstore: preserve
# content-addressed leaf failures` produces, and the message is written by
# whichever process runs the build. Under a test daemon that is the daemon, which
# on a release still emits the pre-patch text: run 30636844197, daemon 2.32.4,
# ca/build FAILs at build.sh:73 on `grepQuietInverse 'Reason: 1 dependency
# failed'`. The rest of this file is upstream coverage and keeps running.
if isDaemonNewer "2.34.7"; then
    testFailureDiagnostics
fi
