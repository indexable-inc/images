#!/usr/bin/env bash

# The catch-all backend refusal is counted, and names the command (ENG-12711).
#
# `EvalState::requireBackendCanServe()` is where every command not wired to the
# Rust backend lands -- `nix flake *`, `nix develop`, `nix print-dev-env`,
# `nix-build`. It used to throw without recording anything, so the journal line
# the fleet census reads was absent for by far the largest population of
# refusals: a ClickHouse query grouping by token reported zero for it, and zero
# refusals and a clean evaluation read identically.
#
# `nix build` used to be the example here and is now served, so the census case
# below uses a command that still refuses. The two `nix build` assertions that
# replaced it are in section 4, where they check the message's claim rather
# than the refusal.
#
# The bug's whole shape is an assertion whose passing state is an absence, so
# the test asserts presence of named things rather than absence of failure:
#
#   1. the refusal emits `token=command-unsupported`,
#   2. its `detail=` is the refusing command and nothing else, so the histogram
#      can be ordered instead of being one row for the entire unwired surface,
#   3. the detail actually varies with the command -- checked across a nested
#      `nix` subcommand and a legacy entry point, because a hard-coded string
#      would pass a single-command test, and
#   4. every command the refusal message claims IS served really is, since a
#      message that sends the user to a command that also refuses is worse than
#      no message. (The other direction -- a newly served command missing from
#      the message -- is not guarded here; nothing enumerates the served set.)

source common.sh

rustArm=$'extra-experimental-features = rust-eval\neval-backend = rust\n'

# Same probe as rust-eval-path-to-store.sh: the binary may have been built
# without the Rust evaluator at all (-Dnix:rust-eval=disabled is the default),
# and only an answered evaluation is evidence that the arm exists.
if [[ "$(NIX_CONFIG=$rustArm nix-instantiate --eval --strict -E 1 2>&1)" != 1 ]]; then
    skipTest "this nix was built without the rust evaluator"
fi

drv='derivation { name = "eng12711"; builder = "/bin/sh"; system = "x86_64-linux"; }'

# The census line, as journald sees it. The `<4>` is the syslog priority that
# makes it selectable by severity; asserting it here is what stops the prefix
# being dropped by someone tidying the output, which would leave the line in
# the journal at `info` where no census query looks for it.
censusLine() { # TOKEN DETAIL
    printf '<4>rust-eval refusal token=%s detail=%s' "$1" "$2"
}

# 1 + 2. A top-level `nix` subcommand that evaluates an installable and is not
# wired. `-F` and `-x`: the detail must be the whole rest of the line, since a
# prose prefix or suffix is what makes a histogram row ungroupable.
err=$TEST_ROOT/print-dev-env.err
expectStderr 1 env NIX_CONFIG="$rustArm" nix print-dev-env --impure --expr "$drv" > "$err"
grepQuiet -Fx "$(censusLine command-unsupported 'nix print-dev-env')" "$err"
grepQuiet -F 'rust-eval unimplemented: nix print-dev-env' "$err"

# The advice survives, and points at a backend rather than only naming the
# problem. This is the most-hit refusal in the fleet; a bare token here would
# leave every user of an unwired command with nowhere to go.
grepQuiet -F "eval-backend = cpp" "$err"

# 3a. A nested `nix` subcommand: the walk has to descend, or every `nix flake
# *` files under `nix flake`.
mkdir -p "$TEST_ROOT/flake"
echo '{ outputs = { self }: { }; }' > "$TEST_ROOT/flake/flake.nix"
err=$TEST_ROOT/flake.err
expectStderr 1 env NIX_CONFIG="$rustArm" nix flake metadata "$TEST_ROOT/flake" > "$err"
grepQuiet -Fx "$(censusLine command-unsupported 'nix flake metadata')" "$err"

# 3b. A legacy entry point, which never reaches the `nix` multi-command at all
# and is named from `argv[0]`. Both paths have to produce a name, because a
# refusal filed under the empty string is the unattributable row again.
err=$TEST_ROOT/nix-build.err
expectStderr 1 env NIX_CONFIG="$rustArm" nix-build --dry-run -E "$drv" > "$err"
grepQuiet -Fx "$(censusLine command-unsupported 'nix-build')" "$err"

# 4. The served commands the message names. Each is run, not trusted.
echo 1 > "$TEST_ROOT/one.nix"
[[ "$(NIX_CONFIG=$rustArm nix eval --expr 1)" == 1 ]]
[[ "$(NIX_CONFIG=$rustArm nix eval --file "$TEST_ROOT/one.nix")" == 1 ]]
# `--strict` is part of the claim, not incidental: without it `nix-instantiate
# --eval` refuses with `command-lazy-print`, so a message naming the bare form
# would walk the user into a second refusal. This assertion is what caught the
# first draft of that message.
[[ "$(NIX_CONFIG=$rustArm nix-instantiate --eval --strict -E 1)" == 1 ]]
# `nix build`, both source shapes the message names. `--dry-run` because the
# claim under test is that the command is served, not that this machine can
# build for `x86_64-linux`; the evaluation and the `.drv` write both happen
# either way.
echo "$drv" > "$TEST_ROOT/drv.nix"
NIX_CONFIG=$rustArm nix build --dry-run --impure --expr "$drv"
NIX_CONFIG=$rustArm nix build --dry-run --impure --file "$TEST_ROOT/drv.nix"
# And the `.drv` is really in the store afterwards, which is the whole
# difference between `nix build` being served and `nix eval` being served: a
# computed path is not a store object, and every gate that compares printed
# paths is blind to which one it has (ENG-12799).
builtDrv=$(NIX_CONFIG=$rustArm nix eval --raw --impure --expr "($drv).drvPath")
[[ -f "$builtDrv" ]] || { echo "the rust arm reported $builtDrv and did not write it"; exit 1; }

# And a served command emits no refusal at all, so the greps above are matching
# this mechanism rather than something the harness prints on every invocation.
err=$TEST_ROOT/served.err
NIX_CONFIG=$rustArm nix eval --expr 1 2> "$err" > /dev/null
grepQuietInverse -F 'rust-eval refusal' "$err"

echo "rust-eval-refusal-token: ok"
