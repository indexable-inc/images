#!/usr/bin/env bash
set -euo pipefail

workspace="${GITHUB_WORKSPACE:?GITHUB_WORKSPACE is required}"
runner_temp="${RUNNER_TEMP:?RUNNER_TEMP is required}"

if [[ "$workspace" != /* || "$workspace" == / || "$runner_temp" != /* ]]; then
  echo "::error::runner workspace and temporary directory must be absolute non-root paths"
  exit 65
fi
if [[ ! -d "$runner_temp" || -L "$runner_temp" ]]; then
  echo "::error::runner temporary directory is not a regular directory"
  exit 1
fi

# GitHub places RUNNER_TEMP at <runner-work>/_temp and checkouts below that
# same work root. Resolve the intentional DynamicUser parent alias, then refuse
# any stale intermediate symlink that escapes the runner-owned tree.
runner_temp_canonical="$(cd -- "$runner_temp" && pwd -P)"
runner_work_root="$(dirname -- "$runner_temp_canonical")"
workspace_parent="${workspace%/*}"
if [[ ! -d "$workspace_parent" ]]; then
  echo "::error::runner workspace parent does not exist"
  exit 1
fi
workspace_parent_canonical="$(cd -- "$workspace_parent" && pwd -P)"
case "$workspace_parent_canonical/" in
  "$runner_work_root/"*) ;;
  *)
    echo "::error::runner workspace parent escapes the runner work root"
    exit 1
    ;;
esac

# A legitimate DynamicUser alias is an ancestor of GITHUB_WORKSPACE, never the
# workspace leaf. Unlink a stale leaf instead of globbing through it.
if [[ -L "$workspace" || ( -e "$workspace" && ! -d "$workspace" ) ]]; then
  rm -f -- "$workspace"
fi
if [[ ! -e "$workspace" ]]; then
  mkdir -- "$workspace"
fi
if [[ ! -d "$workspace" || -L "$workspace" ]]; then
  echo "::error::runner workspace is not a regular directory"
  exit 1
fi

canonical_workspace="$(cd -- "$workspace" && pwd -P)"
case "$canonical_workspace/" in
  "$runner_work_root/"*) ;;
  *)
    echo "::error::runner workspace escapes the runner work root"
    exit 1
    ;;
esac
case "$runner_temp_canonical/" in
  "$canonical_workspace/"*)
    echo "::error::runner workspace contains RUNNER_TEMP"
    exit 1
    ;;
esac

shopt -s dotglob nullglob
workspace_entries=("$canonical_workspace"/*)
if ((${#workspace_entries[@]} != 0)); then
  rm -rf -- "${workspace_entries[@]}"
fi
workspace_entries=("$canonical_workspace"/*)
if [[ -L "$workspace" || "$(cd -- "$workspace" && pwd -P)" != "$canonical_workspace" ]] ||
   ((${#workspace_entries[@]} != 0)); then
  echo "::error::runner workspace reset failed attestation"
  exit 1
fi
