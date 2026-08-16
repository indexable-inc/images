#!/usr/bin/env bash

# Regression test: a content-addressed build killed after its builder created
# $out (possible on non-chroot stores, where the builder writes directly at
# the deterministic scratch store path) leaves that path behind as an invalid,
# build-user-owned orphan. The next build of the same derivation computes the
# same scratch path, and its builder used to fail writing $out over the
# non-writable leftover. Nix must detect the invalid orphan before the build
# and clear it. See https://github.com/indexable-inc/index/issues/4112 (the
# diagnosis is https://github.com/indexable-inc/index/issues/2247).

source common.sh

# Detecting and clearing the invalid orphan before the rebuild is this fork's
# `fix(libstore): clear invalid orphan scratch outputs before rebuilding`, and it
# happens in whichever process builds. Under a test daemon that is the daemon, so
# against a release the rebuild still fails on the non-writable leftover: run
# 30636844197, daemon 2.32.4, ca/killed-build-orphan FAIL at
# killed-build-orphan.sh:47.
requireDaemonNewerThan "2.34.7"

clearStoreIfPossible

outRecord="$TEST_ROOT/killed-build-orphan.out-path"
goFlag="$TEST_ROOT/killed-build-orphan.go"
rm -f "$outRecord" "$goFlag"

# A fresh seed forces a rebuild (and a fresh scratch path) even on a
# dirty store.
seed="$RANDOM$RANDOM$$"

buildArgs=(-f killed-build-orphan.nix top --no-link
    --argstr seed "$seed" --argstr outRecord "$outRecord" --argstr goFlag "$goFlag")

# First run: the builder records its scratch output path and fails (the
# go-flag does not exist yet).
expect 1 nix build "${buildArgs[@]}"
scratch=$(head -n 1 "$outRecord")
[[ -n "$scratch" ]]

# Plant the orphan a killed build leaves behind: an unregistered,
# non-writable directory at the scratch path.
rm -rf "$scratch"
mkdir -p "$scratch"
echo leftover > "$scratch/orphan"
chmod -R a-w "$scratch"

# Precondition of the recovery: the planted path must not be a valid
# (registered) store path. Only then may nix remove it.
expect 1 nix-store --check-validity "$scratch"

# Second run: same derivation, so the same scratch path. Nix must clear
# the invalid orphan, say so, and build successfully.
touch "$goFlag"
nix build "${buildArgs[@]}" 2> "$TEST_ROOT/killed-build-orphan.log"
grepQuiet "clearing invalid orphan" "$TEST_ROOT/killed-build-orphan.log"

# The output must be the fresh build's content, untainted by the orphan.
out=$(nix build "${buildArgs[@]}" --print-out-paths)
grep -q "payload-$seed" "$out/c"
[[ ! -e "$out/orphan" ]]
