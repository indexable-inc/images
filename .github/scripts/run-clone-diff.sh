#!/usr/bin/env bash
set -euo pipefail

: "${EVENT_BASE_SHA:?EVENT_BASE_SHA is required}"
if [[ "${GITHUB_EVENT_NAME}" == pull_request ]]; then
  # index#3038: the event base can predate the synthetic merge that checkout
  # regenerates after main advances. HEAD^1 is the checked-out merge's base.
  base_sha="$(git rev-parse --verify HEAD^1)"
else
  base_sha="${EVENT_BASE_SHA}"
fi
nix run .#clone -- . --diff "${base_sha}" > /dev/null
