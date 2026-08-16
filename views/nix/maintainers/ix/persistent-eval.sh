#!/usr/bin/env bash
#
# What a second evaluation costs when the first one's evaluator is still alive.
#
# Measured on dev-compute-1 (32 cores, 125 GiB) against ix main 96e14957a479
# and this fork at 6fad38528, evaluating
# `nixosConfigurations.<host>.config.system.build.toplevel.drvPath`, four
# requests to one `nix eval-persistent` process:
#
#   request                        wall     cpu   share of cold cpu
#   host1, cold                   32.0s   29.2s   100%
#   host1 again, tree untouched    0.1s    0.1s     0.4%
#   host3, tree untouched          7.9s    7.3s    25.0%
#   host1, after a 1 char edit    30.7s   33.0s   112.7%
#
# Read those four rows together, because the third and fourth are the finding.
# A second host costs a quarter of the first, so three quarters of an
# evaluation is genuinely shared work that a live evaluator already reuses. A
# one character edit to one host's inventory entry returns that host to full
# price and slightly past it. The sharing is not destroyed by the edited bytes,
# which the read set instrumentation puts at 4.3% of attributed cpu; it is
# destroyed by identity. Everything reached through `self` hangs off a new
# accessor, a new `Expr` arena and new `Env` chains, so values whose inputs are
# untouched are unreachable under any key the evaluator holds. Closing that gap
# is what boundary retention is for, and it is what this measurement says the
# prize is: the distance from 112.7% to somewhere near 25.0%.
#
# Reusing evaluated files is not the lever, and this is the measurement that
# settles it. `evalFile` accounting for the same four requests, on
# dev-compute-4 against the same ix revision:
#
#   request                        cpu   evalFile calls   already cached
#   host1, cold                  25.7s           32,333           26,616
#   host1 again, tree untouched   0.1s                3                2
#   host3, tree untouched         6.3s           11,921           11,911
#   host1, after a 1 char edit   28.6s           32,278           31,592
#
# The last row is the point. A warm request after an edit is already answered
# from the file cache 31,592 times out of 32,278, so 97.9% of file evaluation
# is reused with no new machinery, and the request still costs more than the
# cold one. Only 686 files are re-evaluated. Even pretending file evaluation
# were the whole of cold's 25.7s, 686 of 5,717 files bound the remaining
# opportunity at 3.1s of a 28.6s run, and file evaluation is nowhere near the
# whole of it.
#
# So naming a file so its name survives an edit, and letting the file cache
# hit across one, cannot move this. It was tried: keyed on the tree's identity
# without its version, the view of the tree the path is reached through, the
# path within it and the file's content hash, a second evaluation reached that
# key 4 times. The work is in applications rather than files. `pkgs` and the
# module fixpoint are function applications whose arguments derive from the
# edited tree, Nix memoises no application anywhere, and no file level naming
# reaches them.
#
# Off this command nothing moves. Five alternated pairs of a plain `nix eval`,
# this fork against 261bbd0c8, gave a median cpu delta of -0.12%, with
# `nrThunks` 33,435,214 and `nrAvoided` 33,572,769 in all ten runs, so both
# binaries did the same evaluation and the spread is the machine.
#
# The fourth row also has to be checked, not just timed. Before the two cache
# fixes this script exercises, it answered in 11ms with the pre-edit derivation
# path, which reads as a spectacular speedup and is a wrong answer. The script
# therefore compares every drvPath against a plain `nix eval` in a fresh
# process and fails on any disagreement.
#
#   ./persistent-eval.sh --flake /path/to/ix --host hil-compute-1 --other hil-compute-3
#
# The edit is applied to the host's inventory entry and reverted at the end;
# run it against a checkout with nothing uncommitted that you care about.

set -euo pipefail

flake=
host=hil-compute-1
other=hil-compute-3
nix=nix
retain=

while [ $# -gt 0 ]; do
    case "$1" in
        --flake) flake="$2"; shift 2 ;;
        --host) host="$2"; shift 2 ;;
        --other) other="$2"; shift 2 ;;
        --nix) nix="$2"; shift 2 ;;
        --retain) retain=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 1 ;;
    esac
done

[ -n "$flake" ] || { echo "--flake is required" >&2; exit 1; }

inventory="$flake/nix/inventory/nodes/$host.nix"
[ -f "$inventory" ] || { echo "no inventory entry at $inventory" >&2; exit 1; }

attr() { echo ".#nixosConfigurations.$1.config.system.build.toplevel.drvPath"; }

work=$(mktemp -d)
cleanup() {
    git -C "$flake" checkout -- "${inventory#"$flake"/}" 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT

# The edit has to change something the host's own configuration reads, or the
# derivation does not move and the comparison proves nothing.
original=$(grep -oE 'publicIpv4 = "[0-9.]+"' "$inventory" | head -1)
[ -n "$original" ] || { echo "no publicIpv4 in $inventory to edit" >&2; exit 1; }
lastOctet=${original##*.}
lastOctet=${lastOctet%\"}
edited="${original%.*}.$((lastOctet + 1))\""

applyEdit() {
    sed -i "s|$original|$edited|" "$inventory"
    grep -qF "$edited" "$inventory" || { echo "edit did not apply" >&2; exit 1; }
}
revertEdit() {
    git -C "$flake" checkout -- "${inventory#"$flake"/}"
    grep -qF "$original" "$inventory" || { echo "revert did not apply" >&2; exit 1; }
}

revertEdit

# The answers a fresh process gives, which are what the persistent evaluator
# has to agree with.
cd "$flake"
expectedCold=$("$nix" eval --raw "$(attr "$host")")
expectedOther=$("$nix" eval --raw "$(attr "$other")")
applyEdit
expectedEdited=$("$nix" eval --raw "$(attr "$host")")
revertEdit

[ "$expectedCold" != "$expectedEdited" ] || {
    echo "the edit did not move the derivation, so this measures nothing" >&2
    exit 1
}

fifo="$work/requests"
results="$work/results"
mkfifo "$fifo"
: > "$results"

"$nix" eval-persistent --interactive ${retain:+--retain} < "$fifo" > "$results" 2> "$work/stderr" &
evaluator=$!
exec 3>"$fifo"

waitFor() {
    local want=$1 waited=0
    while [ "$(wc -l < "$results")" -lt "$want" ]; do
        kill -0 "$evaluator" 2>/dev/null || {
            echo "evaluator exited before result $want" >&2
            tail -20 "$work/stderr" >&2
            exit 1
        }
        sleep 1
        waited=$((waited + 1))
        [ "$waited" -lt 900 ] || { echo "timed out waiting for result $want" >&2; exit 1; }
    done
}

attr "$host" >&3;  waitFor 1
attr "$host" >&3;  waitFor 2
attr "$other" >&3; waitFor 3
applyEdit
attr "$host" >&3;  waitFor 4

exec 3>&-
wait "$evaluator"
revertEdit

python3 - "$results" "$expectedCold" "$expectedOther" "$expectedEdited" <<'PY'
import json
import sys

results, cold, other, edited = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
rows = [json.loads(line) for line in open(results)]
labels = [
    ("host, cold", cold),
    ("host again, tree untouched", cold),
    ("other host, tree untouched", other),
    ("host, after a one character edit", edited),
]

print(f"{'request':<36}{'wall':>9}{'cpu':>9}{'of cold':>9}")
coldCpu = rows[0]["cpuMs"]
wrong = []
for (label, expected), row in zip(labels, rows):
    print(
        f"{label:<36}{row['wallMs'] / 1000:>8.2f}s{row['cpuMs'] / 1000:>8.2f}s"
        f"{100 * row['cpuMs'] / coldCpu:>8.1f}%"
    )
    if row["value"] != expected:
        wrong.append((label, expected, row["value"]))

if wrong:
    print()
    for label, expected, got in wrong:
        print(f"WRONG ANSWER at '{label}':\n  expected {expected}\n  got      {got}")
    sys.exit(1)

print("\nevery request agreed with a fresh process")
PY
