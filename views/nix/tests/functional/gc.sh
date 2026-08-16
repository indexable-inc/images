#!/usr/bin/env bash

source common.sh

TODO_NixOS

clearStore

# A newer Nix may share this store and use an opaque, per-instance temporary
# root filename. GC must treat the filename as diagnostic data rather than
# assuming that every non-hidden entry is a decimal PID.
#
# That tolerance is this fork's (`fix(libstore): accept opaque temporary root
# filenames`) and it lives in whichever process performs the GC. Under a test
# daemon that process is the DAEMON, so a released daemon still calls stoi() on
# the filename and `nix-store --gc` dies with `error: stoi` before any assertion
# is reached: run 30636844197, daemon 2.32.4, main/gc and ca/gc both FAIL at
# gc.sh:15. Guarded here rather than at the top of the file on purpose, so
# everything else in this test, which is upstream coverage that works against any
# daemon, keeps running in the compat lanes. ca/gc.sh sources this file, so this
# covers that variant too.
if isDaemonNewer "2.34.7"; then
    mkdir -p "$NIX_STATE_DIR/temproots"
    futureTempRoot="$NIX_STATE_DIR/temproots/temproots-123-456"
    touch "$futureTempRoot"
    nix-store --gc --print-roots
    test ! -e "$futureTempRoot"
fi

drvPath=$(nix-instantiate dependencies.nix)
outPath=$(nix-store -rvv "$drvPath")

# Set a GC root.
rm -f "$NIX_STATE_DIR/gcroots/foo"
ln -sf "$outPath" "$NIX_STATE_DIR/gcroots/foo"

[ "$(nix-store -q --roots "$outPath")" = "$NIX_STATE_DIR/gcroots/foo -> $outPath" ]

nix-store --gc --print-roots | grep "$outPath"
nix-store --gc --print-live | grep "$outPath"
nix-store --gc --print-dead | grep "$drvPath"
if nix-store --gc --print-dead | grep -E "$outPath"$; then false; fi

nix-store --gc --print-dead

inUse=$(readLink "$outPath/reference-to-input-2")
if nix-store --delete "$inUse"; then false; fi
test -e "$inUse"

if nix-store --delete "$outPath"; then false; fi
test -e "$outPath"

for i in "$NIX_STORE_DIR"/*; do
    if [[ $i =~ /trash ]]; then continue; fi # compat with old daemon
    touch "$i.lock"
    touch "$i.chroot"
done

nix-collect-garbage

# Check that the root and its dependencies haven't been deleted.
cat "$outPath/foobar"
cat "$outPath/reference-to-input-2/bar"

# Check that the derivation has been GC'd.
if test -e "$drvPath"; then false; fi

rm "$NIX_STATE_DIR/gcroots/foo"

nix-collect-garbage

# Check that the output has been GC'd.
if test -e "$outPath/foobar"; then false; fi

# Check that the store is empty.
rmdir "$NIX_STORE_DIR/.links"
rmdir "$NIX_STORE_DIR"
