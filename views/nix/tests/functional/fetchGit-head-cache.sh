#!/usr/bin/env bash

source common.sh

requireGit

clearStoreIfPossible

# A rev-pinned fetchGit whose rev is already in the local git cache must
# evaluate without talking to the remote: resolving the default ref costs a
# `git ls-remote --symref <url> HEAD` round trip per input per eval, which
# dominates eval wall time for expressions with many pinned git inputs
# (e.g. Cargo.lock git dependencies vendored at eval time).

repo=$TEST_ROOT/git-head-cache

# Treat the file:// URL as remote so the fetcher takes the cached-repo code
# path instead of reading the local repository directly.
export _NIX_FORCE_HTTP=1

rm -rf "$repo" "$TEST_HOME/.cache/nix"

createGitRepo "$repo"
echo utrecht > "$repo/hello"
git -C "$repo" add hello
git -C "$repo" commit -m 'Bla1'
rev=$(git -C "$repo" rev-parse HEAD)

# Log every git invocation that mentions the remote URL. Reads of the local
# cache repo pass filesystem paths, so a `file://` argument identifies
# exactly the subprocesses that would hit the network for a real remote.
remoteLog=$TEST_ROOT/git-remote-calls.log
: > "$remoteLog"
shimDir=$TEST_ROOT/git-shim
mkdir -p "$shimDir"
realGit=$(type -p git)
cat > "$shimDir/git" <<EOF
#!/bin/sh
for arg in "\$@"; do
    case \$arg in
    file://*) echo "\$*" >> "$remoteLog" ;;
    esac
done
exec "$realGit" "\$@"
EOF
chmod +x "$shimDir/git"
export PATH=$shimDir:$PATH

expectRemoteCalls() {
    local expected="$1"
    local when="$2"
    local actual
    actual=$(grep -c . "$remoteLog" || true)
    if [[ $actual != "$expected" ]]; then
        echo "expected $expected remote git call(s) $when, got $actual:" >&2
        cat "$remoteLog" >&2
        exit 1
    fi
    : > "$remoteLog"
}

# Cold fetch: populates the cache repo (and its cached HEAD) for the URL.
path=$(nix eval --raw --expr "(builtins.fetchGit { url = \"file://$repo\"; rev = \"$rev\"; }).outPath")
: > "$remoteLog"

# Warm rev-pinned fetch: the rev is in the cache, so even with the TTL
# expired (--refresh sets tarball-ttl to 0) there must be no remote call,
# neither a default-ref resolution nor a fetch.
path2=$(nix eval --refresh --raw --expr "(builtins.fetchGit { url = \"file://$repo\"; rev = \"$rev\"; }).outPath")
[[ $path = "$path2" ]]
expectRemoteCalls 0 "for a cached rev-pinned input"

# allRefs only widens where a rev may be found; it must not force a refetch
# of a rev that is already present.
path2=$(nix eval --refresh --raw --expr "(builtins.fetchGit { url = \"file://$repo\"; rev = \"$rev\"; allRefs = true; }).outPath")
[[ $path = "$path2" ]]
expectRemoteCalls 0 "for a cached rev-pinned input with allRefs"

# An unpinned fetch resolves the remote HEAD; make sure the cache repo has
# the corresponding local ref, then start measuring.
path3=$(nix eval --impure --raw --expr "(builtins.fetchGit \"file://$repo\").outPath")
[[ $path = "$path3" ]]
: > "$remoteLog"

headFile=$(ls "$TEST_HOME"/.cache/nix/gitv3/*/HEAD)

# Age the cached HEAD past the TTL while the local ref stays fresh: the next
# eval must re-resolve HEAD over the network (one call), but needs no fetch.
# The TTL is passed explicitly because the sandboxed test env has no
# internet, and offline nix forces tarball-ttl to infinity unless the
# setting is overridden.
touch -d '@1000000000' "$headFile"
path4=$(nix eval --tarball-ttl 3600 --impure --raw --expr "(builtins.fetchGit \"file://$repo\").outPath")
[[ $path = "$path4" ]]
expectRemoteCalls 1 "after the cached HEAD expired"

# That successful HEAD resolution must have refreshed the cached HEAD, so
# within the TTL evals are quiet again. (Previously the cached HEAD was only
# written when a fetch ran, so once everything was cached it stayed expired
# forever and every eval paid the network round trip.)
path4=$(nix eval --tarball-ttl 3600 --impure --raw --expr "(builtins.fetchGit \"file://$repo\").outPath")
[[ $path = "$path4" ]]
expectRemoteCalls 0 "within the TTL after the cached HEAD was refreshed"
