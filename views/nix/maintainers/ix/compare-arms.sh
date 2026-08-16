#!/usr/bin/env bash
#
# One scorer for the two-arm comparisons in this directory, because the same
# mistake keeps getting rewritten by hand.
#
# Every gate here runs one expression under two evaluator arms and asks
# whether they agree. The tempting way to write that is `rc_a == rc_b &&
# cmp -s a b`, and it is wrong in a way that is invisible while it is wrong:
# two arms that both produced nothing satisfy it perfectly. A binary that is
# still linking, a probe expression that neither arm can parse, a corpus glob
# that matched no files, a nixpkgs attribute that both arms refuse -- each of
# those scores as agreement, and the run reports a clean pass.
#
# That is not hypothetical. In one week it was written four times by one
# author: `nixpkgs-frontier.sh` counted two timed-out arms as AGREE until a
# TIMEOUT verdict was added; `lang-diff.sh` grew a zero-pairs refusal after a
# `--only` glob matched nothing; an ad-hoc toXML harness reported SAME on
# sixteen rows while the binary it was calling did not exist yet; and the same
# harness reported BOTH-FAIL as a pass for a row where the two arms failed
# with *different* errors, which is a divergence. Each was found by accident,
# late, and each fix was local to the script that had it.
#
#   . "$(dirname "$0")/compare-arms.sh"
#
# The three functions are the three shapes. None of them is clever; the point
# is that they exist once, so a new gate inherits the fix instead of
# rediscovering the bug.

# Score one row. Sets ARMS_VERDICT to one of four values.
#
#   match      both succeeded, byte-identical, and said something
#   empty      both succeeded and said NOTHING -- suspicious, see below
#   fail-both  both failed the same way; whether that is agreement is the
#              caller's call, and the bar is the error class, not the code
#   differ     anything else, including two arms that failed differently
#
# `match` requires agreement AND evidence. Dropping the non-empty condition is
# the whole bug this file exists for, so it is not a flag and there is no way
# to ask for the weaker test.
#
# `empty` is separated from `match` because the two mean opposite things to a
# reader. "Both arms said /nix/store/abc" is a measurement; "both arms exited
# 0 and neither printed anything" is a measurement that did not happen, and a
# gate that treats them alike reports its loudest pass exactly when it is
# broken.
#
# `fail-both` is separated from `empty` because a gate with negative cases has
# rows that are *supposed* to fail on both arms, and folding those into
# `empty` would call the intended outcome suspicious. Migrating
# `search-path-gate.sh` is what found this: two of its nineteen rows are
# `tryEval` probes of malformed input, they exit non-zero on both arms by
# design, and a scorer that only asked "is stdout empty" failed them. The
# distinction is the exit code, not the output.
#
# Two arms that fail *differently* land in `differ`, deliberately: a different
# error is a semantic divergence, and CLAUDE.md's parity bar wants those
# approved or fixed rather than bucketed. Callers wanting the weaker
# error-class comparison have `error-class.sh` for it.
arms_score() { # CPP_OUT CPP_RC RUST_OUT RUST_RC
  local cpp_out=$1 cpp_rc=$2 rust_out=$3 rust_rc=$4
  # ARMS_VERDICT is the out-parameter: set here, read by the caller. shellcheck
  # cannot see across the `.` so it reports it unused, which is the one thing
  # it is definitely not.
  # shellcheck disable=SC2034
  if [ "$cpp_rc" != "$rust_rc" ] || ! cmp -s "$cpp_out" "$rust_out"; then
    ARMS_VERDICT=differ
  elif [ "$cpp_rc" != 0 ]; then
    ARMS_VERDICT=fail-both
  elif [ -s "$cpp_out" ]; then
    ARMS_VERDICT=match
  else
    ARMS_VERDICT=empty
  fi
}

# Refuse a run that compared nothing.
#
# A zero here is never a pass. It means the corpus directory moved, the glob
# matched no files, or the attribute list came back empty -- and every
# "mismatch == 0" check downstream is satisfied by it.
arms_require_rows() { # COUNT WHAT
  local count=$1 what=$2
  if [ "$count" -eq 0 ]; then
    echo "compare-arms: no $what to compare, so this run measured nothing." >&2
    echo "  A zero here satisfies every mismatch check downstream, which is why" >&2
    echo "  it is a refusal and not a pass. Check the corpus path or the filter." >&2
    exit 2
  fi
}

# Refuse to score anything until each arm has been seen to evaluate.
#
# `nix config show eval-backend` reports the *setting* on a binary compiled
# without the backend, so a gate that reads settings passes while measuring a
# stub -- one lang-diff run scored mismatch=249 that way. This asks the binary
# instead, and it is also what catches the plainer failure of calling a path
# that is not executable yet, which is how an ad-hoc harness came to report
# sixteen identical rows against a binary mid-link.
#
# `$1` is a shell function taking an arm name and echoing what that arm
# evaluated `1` to, so each caller keeps its own NIX_CONFIG construction.
arms_probe() { # PROBE_FN ARM...
  local fn=$1
  shift
  local arm got
  for arm in "$@"; do
    got=$("$fn" "$arm" 2>&1 | tail -1)
    if [ "$got" != "1" ]; then
      echo "compare-arms: arm '$arm' cannot evaluate the probe expression '1';" >&2
      echo "  refusing to score a corpus against it. Got: $got" >&2
      exit 2
    fi
  done
}
