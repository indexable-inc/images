#!/usr/bin/env bash
set -euo pipefail

cleanup() {
  local github_env="${GITHUB_ENV:?GITHUB_ENV is required}"
  local added_count="${UPDATE_FLAKE_CHECKOUT_AUTH_COUNT:-}"
  local runner_temp="${RUNNER_TEMP:?RUNNER_TEMP is required}"
  local safe_global="${GIT_CONFIG_GLOBAL:-}"
  local safe_parent runner_temp_canonical
  local slot

  if [[ -z "$added_count" ]]; then
    return
  fi
  if [[ "$added_count" != 2 ]]; then
    printf 'update-flake-checkout-auth: invalid saved config count %q\n' "$added_count" >&2
    exit 65
  fi
  if [[ -z "$safe_global" || ! -f "$safe_global" || -L "$safe_global" ||
        "$(stat -c '%a' "$safe_global")" != 600 ]]; then
    printf 'update-flake-checkout-auth: safe global config cannot be scrubbed\n' >&2
    exit 1
  fi
  safe_parent="$(cd -- "$(dirname -- "$safe_global")" && pwd -P)"
  runner_temp_canonical="$(cd -- "$runner_temp" && pwd -P)"
  if [[ "$safe_parent" != "$runner_temp_canonical" ]]; then
    printf 'update-flake-checkout-auth: safe global config escaped RUNNER_TEMP\n' >&2
    exit 1
  fi

  # The alias-safe bearer lives only in this private file. Scrub it before any
  # updater step runs while retaining a valid global-config target.
  : >"$safe_global"

  {
    printf 'GIT_CONFIG_COUNT=0\n'
    for ((slot = 0; slot < added_count; slot++)); do
      printf 'GIT_CONFIG_KEY_%s=\n' "$slot"
      printf 'GIT_CONFIG_VALUE_%s=\n' "$slot"
    done
    printf 'UPDATE_FLAKE_CHECKOUT_AUTH_COUNT=\n'
  } >>"$github_env"
}

prepare() {
  local workspace="${GITHUB_WORKSPACE:?GITHUB_WORKSPACE is required}"
  local github_env="${GITHUB_ENV:?GITHUB_ENV is required}"
  local runner_temp="${RUNNER_TEMP:?RUNNER_TEMP is required}"
  local server_url="${GITHUB_SERVER_URL:?GITHUB_SERVER_URL is required}"
  local logical_workspace canonical_workspace checkout_token basic_credential safe_global

  if [[ -L "$workspace" ]]; then
    printf 'update-flake-checkout-auth: workspace leaf must not be a symlink\n' >&2
    exit 65
  fi
  mkdir -p -- "$workspace"
  if [[ ! -d "$workspace" || -L "$workspace" ]]; then
    printf 'update-flake-checkout-auth: workspace is not a regular directory\n' >&2
    exit 1
  fi
  logical_workspace="${workspace%/}"
  canonical_workspace="$(cd -- "$workspace" && pwd -P)"
  if [[ "${GIT_CONFIG_COUNT:-0}" != 0 ]]; then
    printf 'update-flake-checkout-auth: inherited command-scoped Git config is not trusted\n' >&2
    exit 65
  fi

  umask 077
  safe_global="$(mktemp "$runner_temp/update-flake-gitconfig.XXXXXX")"
  chmod 600 "$safe_global"
  if [[ ! -f "$safe_global" || -L "$safe_global" || "$(stat -c '%a' "$safe_global")" != 600 ]]; then
    printf 'update-flake-checkout-auth: safe global config is not a regular file\n' >&2
    exit 1
  fi

  # checkout@v7 conditions its credential include on the logical gitdir. A
  # systemd DynamicUser StateDirectory canonicalizes through /var/lib/private,
  # so a private global header is the stable authentication owner. The fresh
  # global config and command overrides also prevent a prior runner user from
  # injecting hooks, fsmonitor commands, or URL rewrites into checkout.
  # https://github.com/actions/checkout/blob/9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0/src/git-auth-helper.ts#L360-L406
  if [[ "$logical_workspace" != "$canonical_workspace" ]]; then
    : "${CHECKOUT_TOKEN:?CHECKOUT_TOKEN is required for an aliased workspace}"
    checkout_token="$CHECKOUT_TOKEN"
    unset CHECKOUT_TOKEN
    basic_credential="$(printf 'x-access-token:%s' "$checkout_token" | base64 | tr -d '\n')"
    unset checkout_token
    printf '::add-mask::%s\n' "$basic_credential"
    if [[ ! "$server_url" =~ ^https://[A-Za-z0-9.-]+(:[0-9]+)?$ ]]; then
      printf 'update-flake-checkout-auth: invalid GitHub server URL\n' >&2
      exit 65
    fi
    {
      printf '[http "%s/"]\n' "${server_url%/}"
      printf '\textraheader = AUTHORIZATION: basic %s\n' "$basic_credential"
    } >"$safe_global"
  fi

  # Only non-secret isolation settings enter the job environment. The bearer
  # stays in the mode-0600 global file until the immediate post-checkout scrub.
  {
    printf 'GIT_CONFIG_GLOBAL=%s\n' "$safe_global"
    printf 'GIT_CONFIG_NOSYSTEM=1\n'
    printf 'GIT_CONFIG_KEY_0=core.hooksPath\n'
    printf 'GIT_CONFIG_VALUE_0=/dev/null\n'
    printf 'GIT_CONFIG_KEY_1=core.fsmonitor\n'
    printf 'GIT_CONFIG_VALUE_1=false\n'
    printf 'GIT_CONFIG_COUNT=2\n'
    printf 'UPDATE_FLAKE_CHECKOUT_AUTH_COUNT=2\n'
  } >>"$github_env"
  unset CHECKOUT_TOKEN checkout_token basic_credential
  printf '::notice title=update-flake-checkout-auth::prepared isolated checkout Git configuration\n'
}

case "${1:-}" in
  cleanup)
    cleanup
    ;;
  prepare)
    prepare
    ;;
  *)
    printf 'update-flake-checkout-auth: expected prepare or cleanup\n' >&2
    exit 64
    ;;
esac
