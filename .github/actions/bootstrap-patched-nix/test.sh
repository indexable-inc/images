#!/usr/bin/env bash
set -euo pipefail

tools="${1:?tool closure is required}"
action_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
export PATH="$tools/bin"

# shellcheck disable=SC1091
source "$action_dir/run.sh"

scratch="$(mktemp -d)"
scratch="$(cd "$scratch" && pwd -P)"
trap 'rm -rf -- "$scratch"' EXIT
export RUNNER_TEMP="$scratch/runner-temp"
mkdir "$RUNNER_TEMP"

# Seed the fixed paths the old action reused, plus executable global Git
# configuration. Trusted helpers must neither reuse nor execute any of it.
mkdir -p "$RUNNER_TEMP/nix-ix-bootstrap-source/.git" \
  "$RUNNER_TEMP/nix-ix-bootstrap/.git" "$scratch/template/hooks" "$scratch/bin"
printf 'stale\n' > "$RUNNER_TEMP/nix-ix-bootstrap-source/stale"
printf '#!/usr/bin/env bash\nprintf hook > %q\n' "$scratch/hook-ran" > "$scratch/template/hooks/post-checkout"
printf '#!/usr/bin/env bash\nprintf fsmonitor > %q\n' "$scratch/fsmonitor-ran" > "$scratch/fsmonitor"
printf '#!/usr/bin/env bash\nprintf remote > %q\nexit 1\n' "$scratch/remote-helper-ran" > "$scratch/bin/git-remote-marker"
chmod +x "$scratch/template/hooks/post-checkout" "$scratch/fsmonitor" "$scratch/bin/git-remote-marker"
hostile_global="$scratch/hostile.gitconfig"
git config --file "$hostile_global" init.templateDir "$scratch/template"
git config --file "$hostile_global" core.hooksPath "$scratch/template/hooks"
git config --file "$hostile_global" core.fsmonitor "$scratch/fsmonitor"
git config --file "$hostile_global" url.marker::.insteadOf file://

origin="$scratch/origin"
GIT_CONFIG_GLOBAL=/dev/null git init -q --template= "$origin"
printf 'trusted\n' > "$origin/payload"
GIT_CONFIG_GLOBAL=/dev/null git -C "$origin" add payload
GIT_CONFIG_GLOBAL=/dev/null git -C "$origin" -c user.name=test -c user.email=test@example.com -c commit.gpgsign=false \
  commit -q -m trusted
revision="$(GIT_CONFIG_GLOBAL=/dev/null git -C "$origin" rev-parse HEAD)"
export GIT_CONFIG_GLOBAL="$hostile_global"
export PATH="$scratch/bin:$PATH"

# Populated by private_temp_dir from the sourced action implementation.
# shellcheck disable=SC2034
temp_dirs=()
checkout_root=""
private_temp_dir checkout_root bootstrap-test-checkout
checkout="$checkout_root/repository"
mkdir "$checkout"
trusted_git init -q --template= "$checkout"
trusted_git -C "$checkout" remote add origin "file://$origin"
trusted_git -C "$checkout" fetch -q --no-tags --depth 1 origin \
  "+$revision:refs/remotes/origin/bootstrap"
trusted_git -C "$checkout" checkout -q --detach refs/remotes/origin/bootstrap
assert_clean_repo "$checkout" "$revision"

if [[ -e "$scratch/hook-ran" || -e "$scratch/fsmonitor-ran" ||
      -e "$scratch/remote-helper-ran" || -L "$checkout_root" ||
      "$(stat -c '%a' "$checkout_root")" != 700 ]]; then
  printf 'trusted bootstrap checkout executed inherited Git configuration\n' >&2
  exit 1
fi

archive_root=""
private_temp_dir archive_root bootstrap-test-archive
trusted_git -C "$checkout" archive "$revision" | tar -x -C "$archive_root"
if [[ "$(cat "$archive_root/payload")" != trusted ]]; then
  printf 'trusted bootstrap archive did not contain the exact tree\n' >&2
  exit 1
fi
job_root=""
private_job_dir job_root bootstrap-test-job-root
cleanup
if [[ ! -d "$job_root" ]]; then
  printf 'job-lifetime GC root was removed at action exit\n' >&2
  exit 1
fi

# A compatible runner-provided nix-ix is the five-minute hot path: it must
# return before reading the lock, fetching 40 MiB, or touching stale temp dirs.
cat > "$scratch/bin/nix" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  *"eval --raw --expr toString 25_565"*) printf 'benign warning\n' >&2; printf '25565' ;;
  "--version") printf 'nix (nix-ix) test\n' ;;
  *) printf 'unexpected fake nix invocation: %s\n' "$*" >&2; exit 1 ;;
esac
EOF
chmod +x "$scratch/bin/nix"
GITHUB_PATH="$scratch/github-path"
export GITHUB_PATH
SECONDS=0
main
if ((SECONDS >= 5)) || [[ ! -e "$RUNNER_TEMP/nix-ix-bootstrap-source/stale" ]]; then
  printf 'compatible preinstalled Nix did not take the bounded hot path\n' >&2
  exit 1
fi
