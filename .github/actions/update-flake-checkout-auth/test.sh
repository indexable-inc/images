#!/usr/bin/env bash
set -euo pipefail

tools="${1:?tool closure is required}"
action_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$action_dir/../../.." && pwd)"
export PATH="$tools/bin"

for executable in \
  base64 bash cat chmod cp dirname env git grep ln mkdir mktemp mv readlink rm sed stat tail tr; do
  if [[ ! -x "$tools/bin/$executable" ]]; then
    printf 'workflow tool closure omits %s\n' "$executable" >&2
    exit 1
  fi
done

scratch="$(mktemp -d)"
scratch="$(cd -- "$scratch" && pwd -P)"
trap 'rm -rf "$scratch"' EXIT

canonical_state="$scratch/var/lib/private/ix-ci-job"
logical_state="$scratch/var/lib/ix-ci-job"
workspace_suffix="_work/repository/repository"
canonical_workspace="$canonical_state/$workspace_suffix"
logical_workspace="$logical_state/$workspace_suffix"
github_env="$scratch/github-env"
previous_config="$scratch/previous.gitconfig"
runner_temp="$scratch/runner-temp"

mkdir -p "$canonical_workspace" "$(dirname "$logical_state")" "$runner_temp"
ln -s private/ix-ci-job "$logical_state"
git config --file "$previous_config" user.name preserved

GITHUB_WORKSPACE="$logical_workspace" \
GITHUB_ENV="$github_env" \
GITHUB_SERVER_URL=https://github.com \
RUNNER_TEMP="$runner_temp" \
CHECKOUT_TOKEN=test-token \
  bash "$action_dir/run.sh" prepare >"$scratch/prepare.stdout"

value_from_env() {
  local key="$1"
  sed -n "s/^${key}=//p" "$github_env" | tail -n 1
}

config_count="$(value_from_env GIT_CONFIG_COUNT)"
config_global="$(value_from_env GIT_CONFIG_GLOBAL)"
config_added="$(value_from_env UPDATE_FLAKE_CHECKOUT_AUTH_COUNT)"
expected_header="AUTHORIZATION: basic $(printf 'x-access-token:test-token' | base64 | tr -d '\n')"
if [[ "$config_count" != 2 || "$config_added" != 2 ||
      "$(value_from_env GIT_CONFIG_KEY_0)" != core.hooksPath ||
      "$(value_from_env GIT_CONFIG_VALUE_0)" != /dev/null ||
      "$(value_from_env GIT_CONFIG_KEY_1)" != core.fsmonitor ||
      "$(value_from_env GIT_CONFIG_VALUE_1)" != false ]] ||
   grep -F "$expected_header" "$github_env" > "$scratch/job-env-token"; then
  printf 'aliased workspace did not produce isolated command-scoped authentication\n' >&2
  exit 1
fi
if [[ ! -f "$config_global" || -L "$config_global" || "$(stat -c '%a' "$config_global")" != 600 ]]; then
  printf 'checkout helper did not create a private global config\n' >&2
  exit 1
fi

git init -q "$canonical_workspace"
# Mirror checkout@v7's mismatched local include. It must remain inactive when
# Git resolves the repository through the canonical DynamicUser path.
checkout_config="$scratch/checkout-credentials.config"
git config --file "$checkout_config" http.https://github.com/.extraheader \
  'AUTHORIZATION: basic duplicate'
git -C "$canonical_workspace" config \
  "includeIf.gitdir:$logical_workspace/.git.path" "$checkout_config"
isolated_git() {
  GIT_CONFIG_GLOBAL="$config_global" \
  GIT_CONFIG_NOSYSTEM=1 \
  GIT_CONFIG_COUNT="$config_count" \
  GIT_CONFIG_KEY_0="$(value_from_env GIT_CONFIG_KEY_0)" \
  GIT_CONFIG_VALUE_0="$(value_from_env GIT_CONFIG_VALUE_0)" \
  GIT_CONFIG_KEY_1="$(value_from_env GIT_CONFIG_KEY_1)" \
  GIT_CONFIG_VALUE_1="$(value_from_env GIT_CONFIG_VALUE_1)" \
    git "$@"
}
for workspace in "$logical_workspace" "$canonical_workspace"; do
  headers="$(isolated_git -C "$workspace" config --get-all http.https://github.com/.extraheader)"
  if [[ "$(grep -Fc "$expected_header" <<<"$headers")" != 1 ||
        "$(isolated_git -C "$workspace" config --get core.hooksPath)" != /dev/null ||
        "$(isolated_git -C "$workspace" config --get core.fsmonitor)" != false ]]; then
    printf 'authentication is unavailable through workspace spelling %s\n' "$workspace" >&2
    exit 1
  fi
  if isolated_git -C "$workspace" config --get user.name > "$scratch/untrusted-global"; then
    printf 'prior global config survived checkout isolation through %s\n' "$workspace" >&2
    exit 1
  fi
done

# Apply the exact cleanup environment a later workflow step receives. The
# token slot must be blank and the prior command-config count restored.
GITHUB_ENV="$github_env" \
RUNNER_TEMP="$runner_temp" \
GIT_CONFIG_GLOBAL="$config_global" \
GIT_CONFIG_COUNT="$config_count" \
UPDATE_FLAKE_CHECKOUT_AUTH_COUNT="$config_added" \
  bash "$action_dir/run.sh" cleanup
if [[ "$(value_from_env GIT_CONFIG_COUNT)" != 0 ||
      -n "$(value_from_env GIT_CONFIG_VALUE_0)" ||
      -s "$config_global" ]]; then
  printf 'cleanup did not scrub command-scoped checkout authentication\n' >&2
  exit 1
fi

# The private global file remains a valid target for updater-style global Git
# writes after the credential slots are scrubbed.
GIT_CONFIG_GLOBAL="$config_global" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_COUNT=0 \
  git config --global --add safe.directory "$canonical_workspace"
if [[ "$(GIT_CONFIG_GLOBAL="$config_global" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_COUNT=0 git config --global --get safe.directory)" != "$canonical_workspace" ]]; then
  printf 'checkout authentication leaked into a later global Git write\n' >&2
  exit 1
fi

# Inherited command config can execute remote helpers before checkout has a
# chance to clean the repository, so the helper rejects it rather than merging.
prior_env="$scratch/prior-env"
if GITHUB_WORKSPACE="$logical_workspace" \
  GITHUB_ENV="$prior_env" \
  GITHUB_SERVER_URL=https://github.com \
  RUNNER_TEMP="$runner_temp" \
  GIT_CONFIG_COUNT=1 \
  GIT_CONFIG_KEY_0=url.file:///hostile.insteadOf \
  GIT_CONFIG_VALUE_0=https://github.com/ \
  CHECKOUT_TOKEN=test-token \
    bash "$action_dir/run.sh" prepare >"$scratch/prior-prepare.stdout" 2>"$scratch/prior-prepare.stderr"; then
  printf 'prepare accepted inherited command-scoped Git config\n' >&2
  exit 1
fi
grep -F 'inherited command-scoped Git config is not trusted' "$scratch/prior-prepare.stderr" >/dev/null

# Reproduce the workflow-owner trust bootstrap with a workspace containing a
# hostile prior repo. Fetch happens in a fresh private temp repo; only the
# attested exact tree may replace the aliased workspace.
owner_source="$scratch/workflow-owner-source"
owner_workspace="$logical_state/_work/workflow-owner/workflow-owner"
git init -q "$owner_source"
printf 'exact owner\n' > "$owner_source/payload"
git -C "$owner_source" add payload
git -C "$owner_source" -c user.name=test -c user.email=test@example.com -c commit.gpgsign=false \
  commit -q -m owner
owner_sha="$(git -C "$owner_source" rev-parse HEAD)"
mkdir -p "$owner_workspace"
owner_runner_temp="$logical_state/_work/_temp"
mkdir -p "$owner_runner_temp"
git init -q "$owner_workspace"
git -C "$owner_workspace" remote add origin marker::stale
hostile_hooks="$scratch/hostile-hooks"
mkdir -p "$hostile_hooks" "$owner_workspace/.update-flake-workflow-actions"
printf '#!/usr/bin/env bash\nprintf hook > %q\n' "$scratch/hook-ran" > "$hostile_hooks/post-checkout"
chmod +x "$hostile_hooks/post-checkout"
git -C "$owner_workspace" config core.hooksPath "$hostile_hooks"
git -C "$owner_workspace" config core.fsmonitor "$scratch/fsmonitor-ran"
git -C "$owner_workspace" config url.marker::.insteadOf "file://$owner_source"
printf 'hostile\n' > "$owner_workspace/.update-flake-workflow-actions/payload"

owner_root="$(mktemp -d -- "$runner_temp/update-flake-owner.XXXXXX")"
chmod 700 "$owner_root"
owner_repo="$owner_root/repository"
mkdir "$owner_repo"
owner_auth="$owner_root/git-auth"
: > "$owner_auth"
chmod 600 "$owner_auth"
trusted_owner_git() {
  GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$owner_auth" GIT_CONFIG_COUNT=0 \
    git -c core.hooksPath=/dev/null -c core.fsmonitor=false "$@"
}
trusted_owner_git init -q --template= "$owner_repo"
trusted_owner_git -C "$owner_repo" remote add origin "file://$owner_source"
{
  printf '[http "https://github.com/"]\n'
  printf '\textraheader = %s\n' "$expected_header"
} > "$owner_auth"
trusted_owner_git -C "$owner_repo" fetch -q --no-tags --depth 1 origin \
  "+$owner_sha:refs/remotes/origin/workflow-owner"
: > "$owner_auth"
trusted_owner_git -C "$owner_repo" checkout -q --detach refs/remotes/origin/workflow-owner
if [[ "$(trusted_owner_git -C "$owner_repo" rev-parse HEAD)" != "$owner_sha" ||
      -n "$(trusted_owner_git -C "$owner_repo" status --porcelain=v1 --untracked-files=all)" ]]; then
  printf 'temporary workflow-owner checkout failed attestation\n' >&2
  exit 1
fi
GITHUB_WORKSPACE="$owner_workspace" RUNNER_TEMP="$owner_runner_temp" \
  bash "$repo_root/.github/scripts/update-flake-reset-workspace.sh"
cp -R -- "$owner_repo"/. "$owner_workspace"/
if [[ "$(trusted_owner_git -C "$owner_workspace" rev-parse HEAD)" != "$owner_sha" ||
      -n "$(trusted_owner_git -C "$owner_workspace" status --porcelain=v1 --untracked-files=all)" ]]; then
  printf 'aliased workflow-owner fetch did not materialize the exact revision\n' >&2
  exit 1
fi
if [[ -e "$scratch/hook-ran" || -e "$scratch/fsmonitor-ran" ||
      -e "$owner_workspace/.update-flake-workflow-actions" || ! -L "$logical_state" ]] ||
   trusted_owner_git -C "$owner_workspace" config --get-all http.https://github.com/.extraheader > "$scratch/persisted-owner-auth" ||
   trusted_owner_git -C "$owner_workspace" config --local --get-regexp '^(core\.(hooksPath|fsmonitor)|url\..*\.insteadOf)$' > "$scratch/persisted-owner-config"; then
  printf 'hostile owner state or command-scoped credential survived attestation\n' >&2
  exit 1
fi
rm -rf -- "$owner_root"

# A stale workspace leaf must be unlinked, never traversed. An intermediate
# alias may resolve only inside the runner's own _work tree.
outside="$scratch/outside-workspace"
leaf_parent="$logical_state/_work/leaf-repro"
leaf_workspace="$leaf_parent/repository"
mkdir -p "$outside" "$leaf_parent"
printf 'preserve\n' > "$outside/sentinel"
ln -s "$outside" "$leaf_workspace"
GITHUB_WORKSPACE="$leaf_workspace" RUNNER_TEMP="$owner_runner_temp" \
  bash "$repo_root/.github/scripts/update-flake-reset-workspace.sh"
if [[ ! -f "$outside/sentinel" || -L "$leaf_workspace" || ! -d "$leaf_workspace" ]]; then
  printf 'workspace reset followed a stale leaf symlink\n' >&2
  exit 1
fi

escaped_parent="$logical_state/_work/escaped-parent"
ln -s "$outside" "$escaped_parent"
if GITHUB_WORKSPACE="$escaped_parent/repository" RUNNER_TEMP="$owner_runner_temp" \
  bash "$repo_root/.github/scripts/update-flake-reset-workspace.sh" \
    > "$scratch/escaped-reset.stdout" 2> "$scratch/escaped-reset.stderr"; then
  printf 'workspace reset accepted an ancestor escaping the runner work root\n' >&2
  exit 1
fi
grep -F 'workspace parent escapes the runner work root' "$scratch/escaped-reset.stdout" >/dev/null
[[ -f "$outside/sentinel" ]]

plain_workspace="$scratch/plain/workspace"
plain_env="$scratch/plain-env"
mkdir -p "$plain_workspace"
GITHUB_WORKSPACE="$plain_workspace" \
GITHUB_ENV="$plain_env" \
GITHUB_SERVER_URL=https://github.com \
RUNNER_TEMP="$runner_temp" \
CHECKOUT_TOKEN='' \
  bash "$action_dir/run.sh" prepare
if [[ "$(sed -n 's/^GIT_CONFIG_COUNT=//p' "$plain_env" | tail -n1)" != 2 ||
      "$(sed -n 's/^UPDATE_FLAKE_CHECKOUT_AUTH_COUNT=//p' "$plain_env" | tail -n1)" != 2 ]]; then
  printf 'canonical workspace did not receive isolated non-auth Git config\n' >&2
  exit 1
fi

if GITHUB_WORKSPACE="$logical_workspace" \
  GITHUB_ENV="$scratch/missing-token-env" \
  GITHUB_SERVER_URL=https://github.com \
  RUNNER_TEMP="$runner_temp" \
  CHECKOUT_TOKEN='' \
    bash "$action_dir/run.sh" prepare >"$scratch/missing-token.stdout" 2>"$scratch/missing-token.stderr"; then
  printf 'aliased workspace accepted an empty checkout token\n' >&2
  exit 1
fi
grep -F 'CHECKOUT_TOKEN is required' "$scratch/missing-token.stderr" >/dev/null

auth_symlink="$scratch/auth-workspace-symlink"
ln -s "$outside" "$auth_symlink"
if GITHUB_WORKSPACE="$auth_symlink" \
  GITHUB_ENV="$scratch/symlink-auth-env" \
  GITHUB_SERVER_URL=https://github.com \
  RUNNER_TEMP="$runner_temp" \
  CHECKOUT_TOKEN=test-token \
    bash "$action_dir/run.sh" prepare > "$scratch/symlink-auth.stdout" 2> "$scratch/symlink-auth.stderr"; then
  printf 'checkout authentication followed a workspace leaf symlink\n' >&2
  exit 1
fi
grep -F 'workspace leaf must not be a symlink' "$scratch/symlink-auth.stderr" >/dev/null
[[ -f "$outside/sentinel" ]]

fake_base64_bin="$scratch/fake-base64-bin"
mkdir "$fake_base64_bin"
cat > "$fake_base64_bin/base64" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s' "${CHECKOUT_TOKEN-}" > "$FAKE_BASE64_ENV"
exec "$REAL_BASE64" "$@"
EOF
chmod +x "$fake_base64_bin/base64"
probe_env="$scratch/probe-env"
FAKE_BASE64_ENV="$scratch/base64-env" \
REAL_BASE64="$tools/bin/base64" \
PATH="$fake_base64_bin:$PATH" \
GITHUB_WORKSPACE="$logical_workspace" \
GITHUB_ENV="$probe_env" \
GITHUB_SERVER_URL=https://github.com \
RUNNER_TEMP="$runner_temp" \
CHECKOUT_TOKEN=process-env-sentinel \
  bash "$action_dir/run.sh" prepare > "$scratch/probe-prepare.stdout"
if [[ -s "$scratch/base64-env" ]] || grep -F process-env-sentinel "$probe_env" > "$scratch/probe-job-env"; then
  printf 'checkout token escaped into a child or job-wide environment\n' >&2
  exit 1
fi
probe_global="$(sed -n 's/^GIT_CONFIG_GLOBAL=//p' "$probe_env" | tail -n 1)"
GITHUB_ENV="$probe_env" RUNNER_TEMP="$runner_temp" \
GIT_CONFIG_GLOBAL="$probe_global" UPDATE_FLAKE_CHECKOUT_AUTH_COUNT=2 \
  bash "$action_dir/run.sh" cleanup
[[ ! -s "$probe_global" ]]

workflow="$repo_root/.github/workflows/update-flake-lock.yml"
wrapper="$repo_root/.github/workflows/update-flake-lock-index.yml"
if grep -Eq 'indexable-inc/index/.github/actions/(update-flake-checkout-auth|install-nix|bootstrap-patched-nix)@' "$workflow"; then
  printf 'credential-bearing reusable-workflow action is fetched from a mutable ref\n' >&2
  exit 1
fi
# These are literal GitHub expressions; the shell must not expand them.
# shellcheck disable=SC2016
workflow_repository='WORKFLOW_REPOSITORY: ${{ job.workflow_repository }}'
# shellcheck disable=SC2016
workflow_revision='WORKFLOW_SHA: ${{ job.workflow_sha }}'
# shellcheck disable=SC2016
workflow_owner_token='CHECKOUT_TOKEN: ${{ secrets.workflow-owner-read-token || github.token }}'
if [[ "$(grep -Fc 'name: Materialize exact workflow owner' "$workflow")" != 1 ||
      "$(grep -Fc "$workflow_revision" "$workflow")" != 1 ]]; then
  printf 'update job does not resolve its workflow owner at the exact called revision\n' >&2
  exit 1
fi
if grep -Eq '&materialize-workflow-owner|\*materialize-workflow-owner' "$workflow"; then
  printf 'watcher retained a duplicate workflow-owner checkout\n' >&2
  exit 1
fi
if [[ "$(grep -Fc "$workflow_owner_token" "$workflow")" != 1 ]] ||
   grep -Eq 'workflow-owner-token|Require cross-repository workflow-owner token' "$workflow"; then
  printf 'workflow-owner read auth is coupled to the caller PR App\n' >&2
  exit 1
fi
if [[ "$(grep -Fc "$workflow_repository" "$workflow")" != 1 ]] ||
   grep -F 'name: Checkout workflow owner' "$workflow" > "$scratch/owner-checkout"; then
  printf 'workflow owner is not fetched through the alias-safe bootstrap path\n' >&2
  exit 1
fi
# This is a literal GitHub expression; the shell must not expand it.
# shellcheck disable=SC2016
if grep -Eq 'ubuntu-|runner-label \|\||id-token:|^  (schedule|workflow_dispatch):' "$workflow" ||
   [[ "$(grep -Fc 'runs-on: ${{ inputs.runner-label }}' "$workflow")" != 2 ]] ||
   grep -F 'runs-on: [self-hosted,' "$workflow" > "$scratch/runner-label-subset" ||
   ! grep -F 'required: true' "$workflow" > "$scratch/required-inputs" ||
   ! grep -F "runner-label: \${{ format('ix-ci-run-" "$wrapper" > "$scratch/wrapper-label"; then
  printf 'flake updater lacks the exact JIT runner-label contract or its self-hosted wrapper\n' >&2
  exit 1
fi
# These are literal GitHub expressions; the shell must not expand them.
# shellcheck disable=SC2016
if [[ "$(grep -Fc 'github-token: ${{ github.token }}' "$workflow")" != 1 ||
      "$(grep -Fc 'SLACK_BOT_TOKEN: ${{ secrets.slack-bot-token }}' "$workflow")" != 1 ]] ||
   grep -F 'GH_TOKEN:' "$workflow" > "$scratch/gh-token-env" ||
   grep -F -- '-H "Authorization: Bearer ${SLACK_BOT_TOKEN}"' "$workflow" > "$scratch/bearer-argv" ||
   grep -F 'GIT_CONFIG_VALUE_1="AUTHORIZATION:' "$workflow" > "$scratch/git-auth-env"; then
  printf 'API credentials are not scoped to the final escalation step\n' >&2
  exit 1
fi
# shellcheck disable=SC2016
grep -F 'if: ${{ always() }}' "$workflow" >/dev/null
# shellcheck disable=SC2016
grep -F 'group: update-flake-lock-${{ github.repository }}' "$workflow" >/dev/null
# shellcheck disable=SC2016
grep -F "const priorBody = issue?.state === 'open' ? (issue.body || '') : '';" "$workflow" >/dev/null
grep -F 'uses: actions/github-script@3a2844b7e9c422d3c10d287c895573f7108da1b3' "$workflow" >/dev/null
if [[ "$(grep -Fc 'name: Bootstrap patched Nix client' "$workflow")" != 1 ]] ||
   grep -Eq 'update-flake-workflow-tools|update-flake-slack-post|curl ' "$workflow"; then
  printf 'API watcher retained the duplicate workflow-owner toolchain\n' >&2
  exit 1
fi
grep -F "['success', 'failure', 'cancelled', 'skipped']" "$workflow" >/dev/null
# shellcheck disable=SC2016
grep -F 'bash "$reset_script"' "$workflow" >/dev/null
# shellcheck disable=SC2016
reset_command='run: bash "$UPDATE_FLAKE_ACTIONS/reset-workspace.sh"'
grep -F "$reset_command" "$workflow" >/dev/null
# shellcheck disable=SC2016
prepare_command='run: bash "$UPDATE_FLAKE_ACTIONS/update-flake-checkout-auth/run.sh" prepare'
grep -F "$prepare_command" "$workflow" >/dev/null
# shellcheck disable=SC2016
cleanup_command='run: bash "$UPDATE_FLAKE_ACTIONS/update-flake-checkout-auth/run.sh" cleanup'
grep -F "$cleanup_command" "$workflow" >/dev/null
