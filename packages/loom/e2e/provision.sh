#!/usr/bin/env bash
# Provision a plain base-image VM into a loom control VM. Runs INSIDE
# the VM (root). This is the imperative e2e reference for the declarative
# nixosConfigurations.loom template.
#
# Required env:
#   LOOM_SRC_URL - tarball of index/packages/loom
#
# Expects the VM to have been created with the account secrets attached:
#   --secret-file loom_ix_token=loom_ix_token
#   --secret-file anthropic_api_key=anthropic_api_key
set -euo pipefail

: "${LOOM_SRC_URL:?}"

# 1. The ix CLI, through the same public installer every customer uses.
mkdir -p /root/bin
IX_INSTALL_DIR=/root/.local/bin sh -c 'curl -fsSL https://ix.dev/install.sh | sh'
ln -sf /root/.local/bin/ix /root/bin/ix

# 2. CLI credentials, from the attached secret. /run/secrets is tmpfs
#    and does NOT survive a stop/start of a restored fork (measured
#    live), so everything a fork needs at wake time is persisted to
#    DISK - snapshots capture disk, and disk survives cold boots.
mkdir -p /var/lib/loom /root/.config/ix
install -m 600 /run/secrets/anthropic_api_key /var/lib/loom/anthropic_api_key
printf 'token = "%s"\nserver = "https://api.ix.dev"\n' \
  "$(cat /run/secrets/loom_ix_token)" > /root/.config/ix/config.toml
chmod 600 /root/.config/ix/config.toml

# 3. claude, official installer, plus the wrapper the children run:
#    key from disk first (fork wake path), tmpfs as fallback.
curl -fsSL https://claude.ai/install.sh | bash
cat > /root/bin/claude <<'WRAP'
#!/bin/sh
key_file=/var/lib/loom/anthropic_api_key
[ -s "$key_file" ] || key_file=/run/secrets/anthropic_api_key
export ANTHROPIC_API_KEY="$(cat "$key_file")"
exec /root/.local/bin/claude "$@"
WRAP
chmod +x /root/bin/claude

# 4. Elixir (cache.ix.dev substitutes it) and loom itself.
nix profile install nixpkgs#elixir
export PATH=/root/.nix-profile/bin:$PATH
mix local.hex --force
cd /root && rm -rf loom && curl -sf "$LOOM_SRC_URL" | tar xz
cd /root/loom && MIX_ENV=prod mix compile --no-deps-check --warnings-as-errors

# 5. The human launcher: `loom` inside the VM = configured iex.
cat > /root/bin/loom <<'LAUNCH'
#!/bin/sh
export PATH=/root/.nix-profile/bin:/root/bin:/usr/bin:/bin
export HOME=/root MIX_ENV=prod
export LOOM_PARENT_VM="${LOOM_PARENT_VM:-loom-ctl}"
export LOOM_IX_BIN=/root/bin/ix
export LOOM_CLAUDE_BIN=/root/bin/claude
export LOOM_PREFLIGHT="test -s /var/lib/loom/anthropic_api_key && test -x /root/bin/claude"
# Same-node hairpin workaround; drop once guests can dial siblings.
export LOOM_IX_PREFIX="${LOOM_IX_PREFIX:---admin}"
export LOOM_RESTORE_ARGS="${LOOM_RESTORE_ARGS:---on hil-compute-2}"
# The fork is the sandbox; in-guest permission prompts protect nothing.
export LOOM_CLAUDE_ARGS="${LOOM_CLAUDE_ARGS:---dangerously-skip-permissions}"
cd /root/loom && exec iex -S mix run --no-deps-check
LAUNCH
chmod +x /root/bin/loom

echo "loom control VM provisioned"
