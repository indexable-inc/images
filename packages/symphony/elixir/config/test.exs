import Config

# Tests do not need to bind a real HTTP socket. Letting the endpoint
# bind 127.0.0.1:4040 means `mix test` fails whenever a real symphony
# is already running on the same host. Set `server: false` so the
# Bandit adapter is skipped; LiveView and Plug logic that the test
# suite touches still work without a live listener.
config :symphony_elixir, SymphonyElixirWeb.Endpoint, server: false

# Tests start the bits they need from test_helper.exs. The full supervision
# tree depends on SYMPHONY_ROOT and friends being set, which test runners
# should not have to inherit from the host env.
config :symphony_elixir, auto_start: false
