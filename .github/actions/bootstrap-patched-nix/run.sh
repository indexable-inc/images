#!/usr/bin/env bash
set -euo pipefail

temp_dirs=()

cleanup() {
  local dir
  for dir in "${temp_dirs[@]}"; do
    rm -rf -- "$dir"
  done
}

on_error() {
  echo "::error title=bootstrap-patched-nix::failed at line $1: $2"
}

private_dir() {
  local output_name="$1"
  local prefix="$2"
  local lifetime="$3"
  local dir

  umask 077
  dir="$(mktemp -d -- "${RUNNER_TEMP:?RUNNER_TEMP is required}/${prefix}.XXXXXX")"
  chmod 700 "$dir"
  if [[ ! -d "$dir" || -L "$dir" || "$(stat -c '%a' "$dir")" != 700 ]]; then
    echo "bootstrap-patched-nix: temporary directory is not private: $dir" >&2
    exit 1
  fi
  if [[ "$lifetime" == step ]]; then
    temp_dirs+=("$dir")
  elif [[ "$lifetime" != job ]]; then
    echo "bootstrap-patched-nix: invalid private directory lifetime: $lifetime" >&2
    exit 65
  fi
  printf -v "$output_name" '%s' "$dir"
}

private_temp_dir() {
  private_dir "$1" "$2" step
}

private_job_dir() {
  private_dir "$1" "$2" job
}

trusted_git() {
  GIT_CONFIG_NOSYSTEM=1 \
  GIT_CONFIG_GLOBAL=/dev/null \
  GIT_CONFIG_COUNT=2 \
  GIT_CONFIG_KEY_0=core.hooksPath \
  GIT_CONFIG_VALUE_0=/dev/null \
  GIT_CONFIG_KEY_1=core.fsmonitor \
  GIT_CONFIG_VALUE_1=false \
    git "$@"
}

assert_clean_repo() {
  local repo="$1"
  local expected="$2"
  local dangerous

  if [[ "$(trusted_git -C "$repo" rev-parse HEAD)" != "$expected" ]]; then
    echo "bootstrap-patched-nix: repository revision mismatch in $repo" >&2
    exit 1
  fi
  if [[ -n "$(trusted_git -C "$repo" status --porcelain=v1 --untracked-files=all)" ]]; then
    echo "bootstrap-patched-nix: repository is not clean: $repo" >&2
    exit 1
  fi
  dangerous="$(trusted_git -C "$repo" config --local --get-regexp \
    '^(core\.(hooksPath|fsmonitor)|url\..*\.insteadOf|filter\..*\.(clean|smudge|process|required))$' || true)"
  if [[ -n "$dangerous" ]]; then
    echo "bootstrap-patched-nix: executable local Git config is forbidden in $repo" >&2
    exit 1
  fi
}

main() {
  local probe source_repository bootstrap_rev
  local source_root source_repo source_tree src src_commit job_root out_link client_root client out short

  # The self-hosted runner image normally already carries nix-ix. This avoids
  # fetching and archiving the 40 MiB bootstrap source on every five-minute
  # healthy escalation tick while retaining the pinned fallback for recovery.
  if probe="$(nix --extra-experimental-features nix-command \
      eval --raw --expr 'toString 25_565' 2>/dev/null)" && [[ "$probe" == 25565 ]]; then
    echo "::notice title=bootstrap-patched-nix::compatible preinstalled $(nix --version) reused"
    return
  fi

  source_repository="$(BOOTSTRAP_LOCK_FIELD=repository \
    nix --extra-experimental-features nix-command \
      eval --impure --raw --file "${BOOTSTRAP_LOCK_READER:?BOOTSTRAP_LOCK_READER is required}")"
  bootstrap_rev="$(BOOTSTRAP_LOCK_FIELD=revision \
    nix --extra-experimental-features nix-command \
      eval --impure --raw --file "$BOOTSTRAP_LOCK_READER")"
  if [[ ! "$source_repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] ||
     [[ ! "$bootstrap_rev" =~ ^[0-9a-f]{40}$ ]]; then
    echo "::error title=bootstrap-patched-nix::invalid bootstrap lock"
    exit 65
  fi

  private_temp_dir source_root nix-ix-bootstrap-source
  source_repo="$source_root/repository"
  mkdir -- "$source_repo"
  trusted_git init -q --template= "$source_repo"
  trusted_git -C "$source_repo" remote add origin \
    "https://github.com/$source_repository.git"
  trusted_git -C "$source_repo" fetch --no-tags --depth 1 origin \
    "+$bootstrap_rev:refs/remotes/origin/bootstrap"
  trusted_git -C "$source_repo" checkout -q --detach refs/remotes/origin/bootstrap
  assert_clean_repo "$source_repo" "$bootstrap_rev"
  source_tree="$(trusted_git -C "$source_repo" rev-parse "$bootstrap_rev^{tree}")"

  # The pinned tree becomes a fresh single-commit repository because that is
  # the one flake-ref shape proven across the runner fleet.
  private_temp_dir src nix-ix-bootstrap-tree
  trusted_git -C "$source_repo" archive "$bootstrap_rev" | tar -x -C "$src"
  trusted_git -C "$src" -c init.defaultBranch=bootstrap init -q --template=
  trusted_git -C "$src" add -A
  trusted_git -C "$src" \
    -c user.email=bootstrap@ix.dev -c user.name=bootstrap \
    -c commit.gpgsign=false \
    commit -q -m "nix-ix bootstrap @ $bootstrap_rev"
  src_commit="$(trusted_git -C "$src" rev-parse HEAD)"
  assert_clean_repo "$src" "$src_commit"
  if [[ "$(trusted_git -C "$src" rev-parse 'HEAD^{tree}')" != "$source_tree" ]]; then
    echo "::error title=bootstrap-patched-nix::synthetic bootstrap tree differs from the pinned revision"
    exit 1
  fi

  # Keep the out-link until RUNNER_TEMP is removed at job teardown. PATH is
  # not a GC root, so deleting this link at action exit races concurrent GC.
  private_job_dir job_root nix-ix-bootstrap-gc-root
  out_link="$job_root/result"
  if ! out="$(nix build \
      --extra-experimental-features "nix-command flakes" \
      --out-link "$out_link" \
      "git+file://$src#nix-ix" 2>&1)"; then
    printf '%s\n' "$out" | tail -n 40
    short="$(printf '%s' "$out" | tail -c 600 | tr '\n' ' ')"
    echo "::error title=bootstrap-patched-nix::nix build failed: $short"
    exit 1
  fi
  client_root="$(readlink -f "$out_link")"
  client="$client_root/bin/nix"
  if [[ "$($client --extra-experimental-features nix-command \
      eval --raw --expr 'toString 25_565')" != 25565 ]]; then
    echo "::error title=bootstrap-patched-nix::built client does not accept underscore digit separators"
    exit 1
  fi
  echo "$client_root/bin" >> "${GITHUB_PATH:?GITHUB_PATH is required}"
  echo "::notice title=bootstrap-patched-nix::$($client --version) on PATH"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  trap 'on_error "$LINENO" "$BASH_COMMAND"' ERR
  trap cleanup EXIT
  main "$@"
fi
