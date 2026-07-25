#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workflow="${repo_root}/.github/workflows/update-flake-lock.yml"
real_git="$(command -v git)"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp:?}"' EXIT

for tool in git jq yq; do
  command -v "$tool" >/dev/null || {
    echo "test-update-flake-lock-direct: missing ${tool}" >&2
    exit 1
  }
done

git_init() {
  "$real_git" init --quiet --initial-branch=main "$1"
  "$real_git" -C "$1" config user.name test
  "$real_git" -C "$1" config user.email test@example.com
}

# Two source commits: the parent starts at old_source while index/main already
# points at new_source.
git_init "${tmp}/index-seed"
printf 'old\n' >"${tmp}/index-seed/version"
"$real_git" -C "${tmp}/index-seed" add version
"$real_git" -C "${tmp}/index-seed" commit --quiet -m old
old_source="$("$real_git" -C "${tmp}/index-seed" rev-parse HEAD)"
"$real_git" clone --quiet --bare "${tmp}/index-seed" "${tmp}/index.git"
printf 'new\n' >"${tmp}/index-seed/version"
"$real_git" -C "${tmp}/index-seed" commit --quiet -am new
new_source="$("$real_git" -C "${tmp}/index-seed" rev-parse HEAD)"
"$real_git" -C "${tmp}/index-seed" push --quiet "${tmp}/index.git" main

# Minimal caller repository with the same path-input lock shape as ix.
git_init "${tmp}/ix-seed"
"$real_git" -c protocol.file.allow=always -C "${tmp}/ix-seed" \
  submodule add --quiet "${tmp}/index.git" index
"$real_git" -C "${tmp}/ix-seed/index" checkout --quiet "$old_source"
"$real_git" -C "${tmp}/ix-seed" config -f .gitmodules submodule.index.branch main
old_timestamp="$("$real_git" -C "${tmp}/ix-seed/index" show -s --format=%ct "$old_source")"
jq -n \
  --arg rev "$old_source" \
  --argjson timestamp "$old_timestamp" \
  '{
    nodes: {
      root: {inputs: {index: "index"}},
      index: {
        locked: {
          lastModified: $timestamp,
          path: "./index",
          rev: $rev,
          type: "path"
        }
      }
    },
    root: "root"
  }' >"${tmp}/ix-seed/flake.lock"
"$real_git" -C "${tmp}/ix-seed" add .gitmodules index flake.lock
"$real_git" -C "${tmp}/ix-seed" commit --quiet -m initial
"$real_git" clone --quiet --bare "${tmp}/ix-seed" "${tmp}/ix.git"
"$real_git" clone --quiet "${tmp}/ix.git" "${tmp}/worker"

# The workflow re-stamps the lock itself; a no-op nix fixture proves that
# behavior without evaluating a real flake or touching the network.
mkdir -p "${tmp}/fake-bin"
printf '#!/usr/bin/env bash\nexit 0\n' >"${tmp}/fake-bin/nix"
chmod +x "${tmp}/fake-bin/nix"
yq -r '.jobs.update-flake-lock.steps[]
  | select(.name == "Bump submodules").run' "$workflow" >"${tmp}/worker.sh"
bash -n "${tmp}/worker.sh"

run_worker() {
  (
    cd "${tmp}/worker"
    DIRECT_PUSH=true \
    GH_TOKEN=test \
    GITHUB_REPOSITORY=test/ix \
    GIT_CONFIG_GLOBAL="${tmp}/gitconfig" \
    SUBMODULE_PATHS=index \
    TRIGGER_USER='' \
    UPDATE_REMOTE_URL="${tmp}/ix.git" \
    PATH="${tmp}/fake-bin:${PATH}" \
      bash "${tmp}/worker.sh"
  )
}

"$real_git" config --file "${tmp}/gitconfig" protocol.file.allow always
run_worker

actual="$("$real_git" --git-dir="${tmp}/ix.git" rev-parse main:index)"
locked="$("$real_git" --git-dir="${tmp}/ix.git" show main:flake.lock | jq -r '.nodes.index.locked.rev')"
if [ "$actual" != "$new_source" ] || [ "$locked" != "$new_source" ]; then
  echo "direct update did not move both the gitlink and lock to index/main" >&2
  exit 1
fi

# A current pin is a true no-op.
before="$("$real_git" --git-dir="${tmp}/ix.git" rev-parse main)"
run_worker
after="$("$real_git" --git-dir="${tmp}/ix.git" rev-parse main)"
if [ "$before" != "$after" ]; then
  echo "current-pin run created an unexpected commit" >&2
  exit 1
fi

# Advance index again, then inject one unrelated ix/main commit immediately
# before the worker's first push. The first push must lose the race; the
# second attempt must rebuild on the new tip and preserve the unrelated file.
printf 'newer\n' >"${tmp}/index-seed/version"
"$real_git" -C "${tmp}/index-seed" commit --quiet -am newer
newest_source="$("$real_git" -C "${tmp}/index-seed" rev-parse HEAD)"
"$real_git" -C "${tmp}/index-seed" push --quiet "${tmp}/index.git" main

"$real_git" clone --quiet "${tmp}/ix.git" "${tmp}/racer"
"$real_git" -C "${tmp}/racer" config user.name racer
"$real_git" -C "${tmp}/racer" config user.email racer@example.com
printf 'preserve me\n' >"${tmp}/racer/race.txt"
"$real_git" -C "${tmp}/racer" add race.txt
"$real_git" -C "${tmp}/racer" commit --quiet -m race

# The single quotes deliberately preserve the wrapper's runtime expansions.
# shellcheck disable=SC2016
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -euo pipefail' \
  'if [ "${1:-}" = push ] && [ "${2:-}" = origin ] &&' \
  '   [ "${3:-}" = HEAD:refs/heads/main ] && [ ! -e "$RACE_MARKER" ]; then' \
  '  : >"$RACE_MARKER"' \
  '  "$REAL_GIT" -C "$RACE_WORK" push --quiet origin main' \
  'fi' \
  'exec "$REAL_GIT" "$@"' >"${tmp}/fake-bin/git"
chmod +x "${tmp}/fake-bin/git"

(
  export RACE_MARKER="${tmp}/race-fired"
  export RACE_WORK="${tmp}/racer"
  export REAL_GIT="$real_git"
  run_worker
)

actual="$("$real_git" --git-dir="${tmp}/ix.git" rev-parse main:index)"
locked="$("$real_git" --git-dir="${tmp}/ix.git" show main:flake.lock | jq -r '.nodes.index.locked.rev')"
race_file="$("$real_git" --git-dir="${tmp}/ix.git" show main:race.txt)"
if [ "$actual" != "$newest_source" ] || [ "$locked" != "$newest_source" ]; then
  echo "race retry did not converge to the newest index/main" >&2
  exit 1
fi
if [ "$race_file" != "preserve me" ]; then
  echo "race retry lost the concurrent ix/main commit" >&2
  exit 1
fi

echo "test-update-flake-lock-direct: PASS"
