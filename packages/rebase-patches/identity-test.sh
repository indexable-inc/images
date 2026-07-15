set -eu

root="$TMPDIR/rebase-patches-identity-test"
home="$root/home"
upstream="$root/upstream"
series="$root/series"
work="$root/work"
mkdir -p "$home" "$upstream" "$work/patches"

commit() {
  repo="$1"
  shift
  git -C "$repo" \
    -c user.name=Fixture \
    -c user.email=fixture@example.com \
    -c commit.gpgsign=false \
    commit "$@"
}

git -C "$upstream" init --quiet
printf 'base\n' >"$upstream/content.txt"
git -C "$upstream" add content.txt
commit "$upstream" --quiet -m 'fixture: add base'
old=$(git -C "$upstream" rev-parse HEAD)

git clone --quiet "$upstream" "$series"
printf 'patch\n' >"$series/content.txt"
git -C "$series" add content.txt
commit "$series" --quiet \
  -m 'fixture: change content' \
  -m 'Exercise scratch repository identity during rebase.'
git -C "$series" format-patch \
  --zero-commit --no-signature --no-stat -N \
  -o "$work/patches" "$old..HEAD"

printf 'upstream\n' >"$upstream/content.txt"
git -C "$upstream" add content.txt
commit "$upstream" --quiet -m 'fixture: move upstream content'
new=$(git -C "$upstream" rev-parse HEAD)

git -C "$work" init --quiet
printf '{"nodes":{"fake-src":{"locked":{"rev":"%s"}}}}\n' "$old" >"$work/flake.lock"
printf '[{"name":"fake","input":"fake-src","url":"%s","patchDir":"patches"}]\n' \
  "$upstream" >"$work/mapping.json"
git -C "$work" add flake.lock mapping.json patches
commit "$work" --quiet -m 'fixture: pin old upstream'
printf '{"nodes":{"fake-src":{"locked":{"rev":"%s"}}}}\n' "$new" >"$work/flake.lock"

cat >"$root/global.gitconfig" <<'EOF'
[user]
	useConfigOnly = true
EOF
export HOME="$home"
export GIT_CONFIG_GLOBAL="$root/global.gitconfig"
export GIT_CONFIG_NOSYSTEM=1
unset GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL

cd "$work"
if rebase-patches fake --mapping mapping.json >"$root/rebase.log" 2>&1; then
  echo 'expected the fixture rebase to stop on a conflict' >&2
  exit 1
fi
if ! grep -Fq 'unresolved conflicts in [content.txt]' "$root/rebase.log"; then
  cat "$root/rebase.log" >&2
  exit 1
fi

scratch_paths=("$TMPDIR"/rebase-patches-fake.*)
test "${#scratch_paths[@]}" = 1
scratch="${scratch_paths[0]}"
test "$(git -C "$scratch" config --local --get user.name)" = rebase-patches
test "$(git -C "$scratch" config --local --get user.email)" = rebase-patches@indexable.dev
git -C "$scratch" config --unset-all user.name
git -C "$scratch" config --unset-all user.email
printf 'merged\n' >"$scratch/content.txt"
git -C "$scratch" add content.txt

rebase-patches resume fake "$scratch" --mapping mapping.json

test ! -e "$scratch"
test "$(find patches -maxdepth 1 -name '*.patch' | wc -l | tr -d ' ')" = 1
grep -Fq 'fixture: change content' patches/0001-fixture-change-content.patch
grep -Fq '+merged' patches/0001-fixture-change-content.patch
grep -Fq "\"base\": \"$new\"" patches/dag.json
touch "$out"
