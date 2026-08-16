#!/usr/bin/env bash

# A builder that writes to stderr and exits at once must still get its output
# into the failure message and into `nix log`, even while other builds are
# starting.
#
# On Darwin it did not. The builder's output travels over a pseudoterminal, and
# XNU destroys whatever the pty still holds shortly after the last slave fd
# closes: ptsclose() allows the master ~0.6s to drain (t_timeout = 60 ticks) and
# ttyclose() then calls ttyflush(FREAD | FWRITE). Nix's worker only polls once
# it has finished starting every runnable child, so under `--max-jobs N` a
# builder that exited early had its entire log flushed away before anything read
# it: an empty logTail, an empty `nix log`, and a bare "builder failed with exit
# code 1" for the operator to work from. Linux keeps the data indefinitely, so
# this only ever bit macOS.

source common.sh

TODO_NixOS

# The assertion below expects exit code 100, which `nix-build` returns for a failed
# build only when IT ran the build. Through a daemon the client gets 1 instead, and
# that is upstream behavior rather than anything this fork changed: measured on
# aarch64-darwin against one live daemon, our 2.34.7+ix client and an unpatched
# nixpkgs 2.35.1 client both exit 1 on the same deliberately failing derivation.
#
# What this test actually covers is fine through a daemon, which is why this is a
# harness declaration and not a bug report. Reproducing the compat lane locally
# (hydraJobs.installTests.aarch64-darwin.againstSelf) shows the daemon delivering
# exactly what the patch is for:
#
#     DIAGNOSTIC-LINE-1
#     DIAGNOSTIC-LINE-2
#     DIAGNOSTIC-LINE-3
#     ...
#     Last 3 log lines:
#     > DIAGNOSTIC-LINE-1
#     > DIAGNOSTIC-LINE-2
#     > DIAGNOSTIC-LINE-3
#
# all three lines, in order, live and in the quoted tail. Only the exit code
# differs, so the test needs the store mode it was written against rather than a
# looser assertion: accepting 1 as well would make it pass for reasons that have
# nothing to do with the pty reader.
#
# Found by run 30633098813, where the compat lane failed at 73/225 in 1.46s.
needLocalStore "nix-build only returns 100 for a build it ran itself; through a daemon a failed build exits 1"

clearStoreIfPossible

# Build the siblings alongside the failing derivation so the scheduler spends
# its time starting children rather than sitting in poll(); that is the state in
# which the output used to disappear.
# 100 is nix-build's exit code for a failed build, not 1.
expectStderr 100 nix-build build-log-fast-exit.nix -A all --no-out-link --max-jobs 8 \
    > "$TEST_ROOT/fast-exit.err"

cat "$TEST_ROOT/fast-exit.err" >&2

# The diagnostic has to survive both as live output and in the "last N log
# lines" the error message quotes.
grepQuiet "DIAGNOSTIC-LINE-1" "$TEST_ROOT/fast-exit.err"
grepQuiet "DIAGNOSTIC-LINE-3" "$TEST_ROOT/fast-exit.err"

# ... and the daemon must have written it to the on-disk log, which is the copy
# `nix log` serves and the one that was coming back empty.
drv=$(nix-instantiate build-log-fast-exit.nix -A failer)
log=$(nix log "$drv")
[[ $log == *DIAGNOSTIC-LINE-1* ]]
[[ $log == *DIAGNOSTIC-LINE-3* ]]

# Every line, and in the order the builder wrote them. Completeness alone is not
# enough: the builder's first output and the rest of its log can reach this file
# by different paths through the setup handshake, so all three lines present in
# the wrong order is a real failure mode, and a quieter one than losing a line.
# Assert the sequence, not just the membership.
[[ $log == *DIAGNOSTIC-LINE-2* ]]
[[ $log == *DIAGNOSTIC-LINE-1*DIAGNOSTIC-LINE-2*DIAGNOSTIC-LINE-3* ]]
grepQuiet "DIAGNOSTIC-LINE-2" "$TEST_ROOT/fast-exit.err"
