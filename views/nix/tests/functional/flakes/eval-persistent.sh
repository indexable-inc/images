#!/usr/bin/env bash

# An evaluator that outlives one evaluation must not answer a later request
# with an earlier request's tree. Two process lifetime caches make it do
# exactly that, and both fail silently: the stale answer is a well formed
# derivation path returned in milliseconds, which is indistinguishable from
# the reuse this command exists to provide unless the value is checked.

source ./common.sh

requireGit

flakeDir="$TEST_ROOT/eval-persistent-flake"

createGitRepo "$flakeDir" ""

cat >"$flakeDir/value.nix" <<'NIX'
"before"
NIX

cat >"$flakeDir/flake.nix" <<'NIX'
{
  outputs = { self }: {
    value = import ./value.nix;
  };
}
NIX

git -C "$flakeDir" add flake.nix value.nix
git -C "$flakeDir" commit -m init

results="$TEST_ROOT/eval-persistent-results"

# Drive the evaluator through a fifo so the tree can be edited between the two
# requests while the same process stays alive.
fifo="$TEST_ROOT/eval-persistent-fifo"
rm -f "$fifo" "$results"
mkfifo "$fifo"

nix eval-persistent --interactive < "$fifo" > "$results" &
evaluatorPid=$!
exec 3>"$fifo"

waitForLines() {
    local want=$1 waited=0
    while [ "$(wc -l < "$results")" -lt "$want" ]; do
        if ! kill -0 "$evaluatorPid" 2>/dev/null; then
            echo "evaluator exited before producing $want results" >&2
            exit 1
        fi
        sleep 1
        waited=$((waited + 1))
        if [ "$waited" -gt 60 ]; then
            echo "timed out waiting for $want results" >&2
            exit 1
        fi
    done
}

echo "$flakeDir#value" >&3
waitForLines 1

echo '"after"' > "$flakeDir/value.nix"

echo "$flakeDir#value" >&3
waitForLines 2

exec 3>&-
wait "$evaluatorPid"

exec 3>&-
rm -f "$fifo"

first=$(sed -n 1p "$results" | jq -r .value)
second=$(sed -n 2p "$results" | jq -r .value)

[[ "$first" = before ]] || { echo "first request answered '$first', expected 'before'" >&2; exit 1; }
[[ "$second" = after ]] || { echo "second request answered '$second', expected 'after'; the evaluator served the pre-edit tree" >&2; exit 1; }

# The file accounting exists to answer whether reuse across evaluations is
# where the time is, and a counter that has quietly stopped counting answers
# it with a zero that reads exactly like "nothing was reused". So require the
# first request to have asked for files at all, and the second to have been
# answered from the cache at least once, which it must be: the two requests
# share every flake input.
firstCalls=$(sed -n 1p "$results" | jq -r .evalFileCalls)
secondHits=$(sed -n 2p "$results" | jq -r .evalFilePathHits)

[[ "$firstCalls" -gt 0 ]] \
    || { echo "first request reported $firstCalls file evaluations; the accounting is not counting" >&2; exit 1; }
[[ "$secondHits" -gt 0 ]] \
    || { echo "second request reused $secondHits already evaluated files, expected at least one" >&2; exit 1; }
