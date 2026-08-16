#!/usr/bin/env bash

# `eval-backend = shadow` compares a FLAKE, and `nix build` reaches the census
# at all.
#
# What this pins is a measurement, not a feature. Before it, shadow described
# only `--expr` and `--file`: a flake installable set neither, so
# `describeShadowSubject` turned it away and counted `unservable-shape`, and
# `nix build` had no shadow wiring whatsoever. Measured on this tree, with a
# binary whose `rust` backend served both commands perfectly well:
#
#   nix eval --expr '1 + 1'              attempts 1
#   nix eval nixpkgs#hello.name          attempts 0, unservable-shape 1
#   nix build --dry-run nixpkgs#hello    attempts 0, and every skip 0 as well
#
# The third line is the one that matters. Zero attempts *and* zero skips is a
# census that cannot tell "nothing to compare" from "this command was never
# wired up", and every `darwin-rebuild` and `home-manager switch` goes through
# it -- so the harness reported green while comparing nothing, for ever.
#
# Hence the assertions below are about the counters and not about exit codes.
# A clean exit proves nothing here; `attempts: 0` is the failure.

source common.sh

requireDaemonNewerThan "2.4pre20210625"

clearStoreIfPossible

shadowArm=$'extra-experimental-features = rust-eval flakes nix-command\neval-backend = shadow\n'
cppArm=$'extra-experimental-features = rust-eval flakes nix-command\neval-backend = cpp\n'

# As in the sibling tests: `nix config show` reports eval-backend on a binary
# compiled without the Rust evaluator, so ask by evaluating. See CLAUDE.md,
# "A setting is not a capability".
if [[ "$(NIX_CONFIG=$shadowArm nix-instantiate --eval --strict -E 1 2>&1)" != 1 ]]; then
    skipTest "this nix was built without the rust evaluator"
fi

work=$TEST_ROOT/shadow-flake
rm -rf "$work"
mkdir -p "$work"

# `builtins.currentSystem` is not available under a flake's pure eval, so the
# system is baked in here instead.
system=$(nix-instantiate --eval --strict -E builtins.currentSystem | tr -d '"')

cat > "$work/flake.nix" <<EOF
{
  outputs = { self }: {
    answer = 1 + 1;
    traced = builtins.trace "SHADOW-TRACE" "traced-value";
    drv = derivation {
      name = "shadow-flake-drv";
      system = "$system";
      builder = "/bin/sh";
      args = [ "-c" "echo hi > \\\$out" ];
    };
  };
}
EOF

flake="path:$work"
stats=$work/stats.json

shadow() { # SUBCOMMAND ARGS... -> runs under the shadow arm, writes $stats
    rm -f "$stats"
    NIX_CONFIG="$shadowArm" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$stats" \
        nix "$@" > "$work/out" 2> "$work/err"
}

field() { # JQ-PATH
    jq -r "$1" < "$stats"
}

# Every attempt reaches a verdict and every skip has a name, so the two
# numbers together account for the invocation. Checked after each case
# because a hole is exactly what this file exists to catch.
accountedFor() { # LABEL EXPECTED-EVALUATIONS
    local label=$1 expected=$2 attempts skips unaccounted
    attempts=$(field '.shadow.attempts')
    skips=$(field '[.shadow.skipped[]] | add')
    unaccounted=$(field '.shadow.unaccounted')
    [[ $unaccounted -eq 0 ]] || {
        echo "$label: $unaccounted attempt(s) reached no verdict" >&2
        exit 1
    }
    [[ $((attempts + skips)) -ge $expected ]] || {
        echo "$label: $expected evaluation(s) happened but the census saw $attempts attempt(s)" \
             "and $skips skip(s) -- an evaluation that is neither is one nobody can see" >&2
        jq -c '.shadow | {attempts, skipped}' < "$stats" >&2
        exit 1
    }
}

# 1. A flake installable through `nix eval`. THE regression: this used to be
#    `attempts 0, unservable-shape 1`.
shadow eval "$flake#answer"
[[ "$(cat "$work/out")" == 2 ]] || { echo "nix eval served the wrong answer" >&2; exit 1; }
[[ $(field '.shadow.attempts') -ge 1 ]] || {
    echo "a flake installable was not shadowed at all" >&2
    jq -c '.shadow | {attempts, skipped}' < "$stats" >&2
    exit 1
}
[[ $(field '.shadow.verdicts.agreed') -ge 1 ]] || {
    echo "the two arms did not agree about a flake output" >&2
    jq -c '.shadow | {verdicts, divergences}' < "$stats" >&2
    exit 1
}
[[ $(field '.shadow.skipped["unservable-shape"]') -eq 0 ]] || {
    echo "a flake installable is still being written off as an unservable shape" >&2
    exit 1
}
accountedFor "nix eval flake" 1

# 2. `nix build --dry-run`, which used to reach the census at all: zero
#    attempts AND zero skips.
shadow build --dry-run --no-link "$flake#drv"
[[ $(field '.shadow.attempts') -ge 1 ]] || {
    echo "nix build did not reach the shadow census" >&2
    jq -c '.shadow | {attempts, skipped}' < "$stats" >&2
    exit 1
}
[[ $(field '.shadow.verdicts.agreed') -ge 1 ]] || {
    echo "the two arms disagree about a derivation nix build would build" >&2
    jq -c '.shadow | {verdicts, divergences}' < "$stats" >&2
    exit 1
}
accountedFor "nix build --dry-run" 1

# 3. A real build, not a dry run. `darwin-rebuild` and `home-manager` take
#    this path and not the one above, so covering only `--dry-run` would leave
#    the workload this exists for uncompared.
shadow build --no-link "$flake#drv"
[[ $(field '.shadow.attempts') -ge 1 ]] || {
    echo "a real nix build did not reach the shadow census" >&2
    jq -c '.shadow | {attempts, skipped}' < "$stats" >&2
    exit 1
}
accountedFor "nix build" 1

# 4. `nix build --expr`, which has no flake and reads its source directly.
shadow build --dry-run --no-link --impure --expr \
    "derivation { name = \"shadow-expr-drv\"; system = \"$system\"; builder = \"/bin/sh\"; args = [ \"-c\" \"echo hi > \\\$out\" ]; }"
[[ $(field '.shadow.attempts') -ge 1 ]] || {
    echo "nix build --expr did not reach the shadow census" >&2
    jq -c '.shadow | {attempts, skipped}' < "$stats" >&2
    exit 1
}
accountedFor "nix build --expr" 1

# 5. Two installables get two comparisons, each against its own answer.
#    A crossed pairing would report two mismatches rather than two agreements,
#    which is why this asserts the verdict and not just the count.
shadow build --dry-run --no-link "$flake#drv" "$flake#drv"
[[ $(field '.shadow.attempts') -eq 2 ]] || {
    echo "two installables produced $(field '.shadow.attempts') comparison(s)" >&2
    exit 1
}
[[ $(field '.shadow.verdicts.agreed') -eq 2 ]] || {
    echo "two installables were compared against the wrong answers" >&2
    jq -c '.shadow | {verdicts, divergences}' < "$stats" >&2
    exit 1
}
accountedFor "two installables" 2

# 6. What cannot be compared is SKIPPED BY NAME, never silently dropped.
#    `^out` reconciles an explicit output selection against
#    `meta.outputsToInstall`, which this does not compare.
shadow build --dry-run --no-link "$flake#drv^out"
[[ $(field '.shadow.attempts') -eq 0 ]] || {
    echo "an output selection was compared, which this backend does not cover" >&2
    exit 1
}
[[ $(field '.shadow.skipped["unservable-shape"]') -eq 1 ]] || {
    echo "an output selection vanished from the census instead of being skipped by name" >&2
    jq -c '.shadow.skipped' < "$stats" >&2
    exit 1
}
accountedFor "output selection" 1

# 7. A C++ arm that FAILS is compared, not written off. Two arms failing the
#    same way is exactly as much of a comparison as two arms agreeing, and on
#    somebody iterating on a configuration the failures are most of the
#    interesting cases.
shadow eval "$flake#nosuchattribute" || true
[[ $(field '.shadow.verdicts["agreed-failure"]') -ge 1 ]] || {
    echo "a flake whose attribute is missing was not compared as a failure" >&2
    jq -c '.shadow | {attempts, verdicts, skipped}' < "$stats" >&2
    exit 1
}
accountedFor "missing flake attribute" 1

# 8. And a C++ arm that never got as far as an evaluation says so, rather than
#    looking like a run with nothing to compare. There is no lock for a flake
#    that could not be fetched, and this describes a flake out of its lock, so
#    there is nothing to hand the Rust arm -- which has to be a named row and
#    not a silence.
shadow build --dry-run --no-link "path:$work/nonexistent-flake#drv" || true
[[ $(field '[.shadow.skipped[]] | add') -ge 1 ]] || {
    echo "a nix build that never evaluated left no trace in the census" >&2
    jq -c '.shadow | {attempts, skipped}' < "$stats" >&2
    exit 1
}

# 9. The served arm's bytes do not move. This is the constraint the whole mode
#    rests on: under shadow the user's result must be what `cpp` produced,
#    stderr included.
#
#    `builtins.trace` is the case that used to break it. Both arms evaluate
#    the expression, so both reach the trace, and the Rust arm's copy went to
#    the same stderr -- one deprecation notice from nixpkgs printed twice
#    because a measurement was switched on. Measured on the binary before this
#    change: `nix eval --expr 'builtins.trace "X" 1'` printed `trace: X` twice
#    under shadow and once under cpp.
for arm in cpp shadow; do
    config=$cppArm
    [[ $arm == shadow ]] && config=$shadowArm
    NIX_CONFIG="$config" nix eval "$flake#traced" > "$work/out.$arm" 2> "$work/err.$arm"
done
diff "$work/out.cpp" "$work/out.shadow" || {
    echo "shadow changed what the served arm printed on stdout" >&2
    exit 1
}
diff "$work/err.cpp" "$work/err.shadow" || {
    echo "shadow changed what the served arm printed on stderr" >&2
    exit 1
}
[[ $(grep -c "SHADOW-TRACE" < "$work/err.shadow") -eq 1 ]] || {
    echo "the shadow arm printed its own copy of a builtins.trace line" >&2
    cat "$work/err.shadow" >&2
    exit 1
}

# 10. The vocabulary the census reports is a denominator, so every reason has a
#    row whether or not it occurred. A rename that drops one would otherwise
#    make a query silently stop matching.
for name in reentrant budget cpp-failed-before-eval unservable-shape \
            non-value-installable flake-unservable cpp-answer-shape backend-absent; do
    jq -e --arg n "$name" '.shadow.skipped | has($n)' < "$stats" > /dev/null || {
        echo "the census lost the skip reason '$name'" >&2
        exit 1
    }
done
for name in agreed agreed-failure agreed-failure-text-differs refused mismatched crashed timed-out; do
    jq -e --arg n "$name" '.shadow.verdicts | has($n)' < "$stats" > /dev/null || {
        echo "the census lost the verdict '$name'" >&2
        exit 1
    }
done

echo "rust-eval-shadow-flake: ok"
