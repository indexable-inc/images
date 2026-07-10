set -eu

export HOME="$TMPDIR/home"
mkdir -p "$HOME" upstream work/patches
git config --global user.email test@example.com
git config --global user.name Test

git -C upstream init --quiet
printf 'base\n' >upstream/base.txt
git -C upstream add base.txt
git -C upstream commit --quiet -m base
printf 'upstream\n' >>upstream/base.txt
git -C upstream commit --quiet -am upstream
new=$(git -C upstream rev-parse HEAD)

git clone --quiet upstream scratch
printf 'patch\n' >scratch/patch.txt
git -C scratch add patch.txt
git -C scratch commit --quiet -m 'fixture: add patch'

printf 'stale\n' >work/patches/0001-stale.patch
printf '{"nodes":{"fake-src":{"locked":{"rev":"%s"}}}}\n' "$new" >work/flake.lock
printf '[{"name":"fake","input":"fake-src","url":"%s","patchDir":"patches"}]\n' "$PWD/upstream" >work/mapping.json
printf '{"fork":"fake","old":"unused","new":"%s"}\n' "$new" >scratch/.git/rebase-patches-state.json
mkdir -p scratch/.git/rr-cache/resolution
printf 'before\n' >scratch/.git/rr-cache/resolution/preimage
printf 'after\n' >scratch/.git/rr-cache/resolution/postimage
printf 'transient\n' >scratch/.git/rr-cache/resolution/thisimage

cd work
rebase-patches resume fake "$PWD/../scratch" --mapping mapping.json

test ! -e ../scratch
test ! -e patches/0001-stale.patch
test "$(find patches -maxdepth 1 -name '*.patch' | wc -l | tr -d ' ')" = 1
grep -Fq 'fixture: add patch' patches/0001-fixture-add-patch.patch
grep -Fq "\"base\": \"$new\"" patches/dag.json
test -e patches/rerere/resolution/preimage
test -e patches/rerere/resolution/postimage
test ! -e patches/rerere/resolution/thisimage
touch "$out"
