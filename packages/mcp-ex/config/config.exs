import Config

# Stdout belongs to the MCP JSON-RPC wire; every log line must go to stderr
# or it would corrupt the protocol stream.
config :logger, :default_handler, config: [type: :standard_error]

# Build exqlite's NIF from its vendored sqlite3 source instead of downloading
# a precompiled artefact: the sandboxed nix builds have no network, and a cc
# build from the hex tarball is reproducible everywhere else too.
config :elixir_make, :force_build, exqlite: true

# Tests keep the action log in memory: the sandboxed check has no writable
# HOME, and no test should touch the operator's real log file.
if config_env() == :test do
  config :ix_mcp, actions_db: ":memory:"
end
