#!/bin/sh
set -eu

key_file=/var/lib/loom/anthropic_api_key
if [ ! -s "$key_file" ]; then
  key_file=/run/secrets/anthropic_api_key
fi

export ANTHROPIC_API_KEY="$(cat "$key_file")"
exec @claude@ "$@"
