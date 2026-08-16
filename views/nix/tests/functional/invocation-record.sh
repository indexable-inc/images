#!/usr/bin/env bash

source common.sh

TODO_NixOS

enableFeatures "invocation-records"

clearStore

# Records go under the *client's* state directory, so point that somewhere
# empty and assert on what lands there.
records=$TEST_ROOT/invocations
rm -rf "$records"
export NIX_STATE_HOME=$TEST_ROOT

nix build --file invocation-record.nix --no-link 2> "$TEST_ROOT/stderr"

# The id is printed on stderr, so it never corrupts `--json` output on stdout.
id=$(sed -n 's/^invocation \([0-9a-f]*\)$/\1/p' "$TEST_ROOT/stderr")
[[ -n $id ]]
[[ -d $records/$id ]]

record=$(nix invocation show "$id" --json)

# The exit status is recorded.
[[ $(jq -r '.exitStatus' <<< "$record") == 0 ]]

# Evaluation was measured.
[[ $(jq -r '.eval.cpuTime > 0' <<< "$record") == true ]]
[[ $(jq -r '.eval.nrFunctionCalls > 0' <<< "$record") == true ]]

# Both derivations were built, each with a duration and a place.
[[ $(jq -r '[.work[] | select(.kind == "build")] | length' <<< "$record") == 2 ]]
[[ $(jq -r 'any(.work[]; .path | endswith("invocation-record-dep.drv"))' <<< "$record") == true ]]
[[ $(jq -r '[.work[] | select(.path | endswith("invocation-record-dep.drv"))][0].seconds >= 2' <<< "$record") == true ]]
[[ $(jq -r '[.work[] | select(.path | endswith("invocation-record-dep.drv"))][0].on' <<< "$record") == local ]]

# A prefix of the id resolves, and so does `last`.
nix invocation show "${id:0:8}" > /dev/null
nix invocation show last > /dev/null

# Reading a record does not itself mint one.
before=$(find "$records" -maxdepth 1 -mindepth 1 | wc -l)
nix invocation list > /dev/null
after=$(find "$records" -maxdepth 1 -mindepth 1 | wc -l)
[[ $before == "$after" ]]

# A failed command is recorded too, with its status.
if nix build --impure --expr 'throw "nope"' --no-link 2> "$TEST_ROOT/stderr2"; then false; fi
failed=$(sed -n 's/^invocation \([0-9a-f]*\)$/\1/p' "$TEST_ROOT/stderr2")
[[ $(nix invocation show "$failed" --json | jq -r '.exitStatus') == 1 ]]

# `keep-invocation-records` bounds the directory.
for _ in 1 2 3; do
    nix eval --expr 1 --option keep-invocation-records 2 > /dev/null
done
[[ $(find "$records" -maxdepth 1 -mindepth 1 | wc -l) -le 2 ]]
