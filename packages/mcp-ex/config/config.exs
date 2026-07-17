import Config

# Stdout belongs to the MCP JSON-RPC wire; every log line must go to stderr
# or it would corrupt the protocol stream.
config :logger, :default_handler, config: [type: :standard_error]
