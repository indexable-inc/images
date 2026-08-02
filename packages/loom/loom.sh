#!@shell@
set -eu

key_file=/var/lib/loom/anthropic_api_key
if [ ! -s "$key_file" ]; then
  key_file=/run/secrets/anthropic_api_key
fi

export ANTHROPIC_API_KEY="$(cat "$key_file")"
export IS_SANDBOX=1

# A snapshot restore resumes this process in the child VM. Keep one tiny
# identity watcher beside Claude so the cloned parent session exits there;
# the original session keeps running because its address never changes.
baseline="$(ip -o addr show scope global 2>/dev/null | awk '{print $4}' | sort)"
claude_pid=$$
(
  while kill -0 "$claude_pid" 2>/dev/null; do
    sleep 1
    current="$(ip -o addr show scope global 2>/dev/null | awk '{print $4}' | sort)"
    if [ "$current" != "$baseline" ]; then
      kill -TERM "$claude_pid" 2>/dev/null || true
      exit 0
    fi
  done
) &

exec @claude@ --append-system-prompt-file=@prompt@ "$@"
