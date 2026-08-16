#!/usr/bin/env bash

# `readdir` signals end-of-directory and failure identically, by returning
# NULL, and the two are told apart only by whether it set `errno`. The
# collector set `errno` to zero before each call, which is the setup for that
# check, and then never made it. So a read that failed partway through the
# store directory produced a short enumeration with nothing to say it was
# short: the collector deleted what it had seen, reported the round complete,
# and left everything past the failure unexamined. On a schedule, against a
# durable store, that is a disk filling up while every run reports success.
#
# Nothing available to a test makes a real directory read fail on demand --
# permissions are checked at `opendir`, and EIO or ESTALE want hardware or a
# network filesystem misbehaving -- so the fault goes in at the libc entry
# point, which is where the collector's information about it comes from
# anyway. See readdir-fault/readdir-fault.c.

source common.sh

TODO_NixOS

if [[ $(uname) != Linux ]]; then skipTest "readdir interposition through LD_PRELOAD is Linux-specific"; fi
needLocalStore "the collector has to run in the process being injected into"

preload="${_NIX_TEST_BUILD_DIR?}/readdir-fault/libreaddirfault.so"
[[ -e "$preload" ]] \
  || fail "libreaddirfault.so was not built; the suite's meson 'deps' entry is wrong"

storeDir="${NIX_STORE_DIR:-/nix/store}"
linksDir="$storeDir/.links"

clearStore

# The library does nothing at all without `NIX_READDIR_FAULT_DIR`, so this run
# is what makes the rest of the test a statement about the fault rather than
# about preloading a library into the collector.
garbage=$(nix-store --add ./dummy)
LD_PRELOAD="$preload" nix-store --gc
expect 1 nix-store --check-validity "$garbage"

# Now with the store directory failing from its very first entry, so the
# collector sees a store with nothing in it -- which is exactly what a store
# with no garbage in it looks like.
garbage=$(nix-store --add ./dummy)
gcLog="$TEST_ROOT/gc-readdir-fault-store.log"
expectStderr 1 env \
  LD_PRELOAD="$preload" \
  NIX_READDIR_FAULT_DIR="$storeDir" \
  NIX_READDIR_FAULT_AFTER=0 \
  nix-store --gc \
  | sed 's/\x1b\[[0-9;]*m//g' > "$gcLog"
cat "$gcLog"

# Before the fix this exited 0 and printed how much it had freed, having
# enumerated nothing. The garbage stays either way; what changed is whether
# anybody is told the sweep was partial.
grepQuiet "reading directory" "$gcLog"
grepQuiet "$storeDir" "$gcLog"
nix-store --check-validity "$garbage"

# The same hole guarded the second loop, the one that unlinks unused entries in
# `.links` and then reports how much hard linking is saving. A short read there
# both leaves entries behind and makes that figure wrong.
nix-store --optimise
[[ -d "$linksDir" ]]

gcLinksLog="$TEST_ROOT/gc-readdir-fault-links.log"
expectStderr 1 env \
  LD_PRELOAD="$preload" \
  NIX_READDIR_FAULT_DIR="$linksDir" \
  NIX_READDIR_FAULT_AFTER=0 \
  nix-store --gc \
  | sed 's/\x1b\[[0-9;]*m//g' > "$gcLinksLog"
cat "$gcLinksLog"

grepQuiet "reading directory" "$gcLinksLog"
grepQuiet "$linksDir" "$gcLinksLog"

# And with no fault, the collector still works: the checks report a genuine
# failure, not every read.
nix-store --gc
