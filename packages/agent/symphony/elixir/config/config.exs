import Config

# Zone-aware DateTime math (the cron trigger's `tz "..."` evaluation) resolves
# IANA zones through time_zone_info's bundled database. Its default
# `update: :disabled` keeps lookups fully offline, which the sandboxed nix
# check requires.
config :elixir, :time_zone_database, TimeZoneInfo.TimeZoneDatabase

config :phoenix, :json_library, Jason

config :symphony_elixir, SymphonyElixirWeb.Endpoint,
  adapter: Bandit.PhoenixAdapter,
  url: [host: "localhost"],
  render_errors: [
    formats: [html: SymphonyElixirWeb.ErrorHTML, json: SymphonyElixirWeb.ErrorJSON],
    layout: false
  ],
  pubsub_server: SymphonyElixir.PubSub,
  live_view: [signing_salt: "symphony-live-view"],
  check_origin: false,
  server: true,
  http: [ip: {127, 0, 0, 1}, port: 4040]

import_config "#{config_env()}.exs"
