#!/usr/bin/env bash
set -e
sid=$(jq -r '.session_id')
echo "{\"hookSpecificOutput\": {\"hookEventName\": \"SessionStart\", \"additionalContext\": \"Session ID: $sid\"}}"
