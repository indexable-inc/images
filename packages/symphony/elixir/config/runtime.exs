import Config

# Runtime config. Boot reads these from the environment so a single binary can
# move between hosts without recompiling.

secret_key_base =
  System.get_env("SYMPHONY_SECRET_KEY_BASE") ||
    Base.encode64(:crypto.strong_rand_bytes(48), padding: false)

config :symphony_elixir, SymphonyElixirWeb.Endpoint, secret_key_base: secret_key_base

if config_env() != :test do
  port_string = System.get_env("SYMPHONY_HTTP_PORT", "4040")

  port =
    case Integer.parse(port_string) do
      {value, ""} when value >= 0 -> value
      _ -> raise "SYMPHONY_HTTP_PORT must be a non-negative integer, got #{inspect(port_string)}"
    end

  config :symphony_elixir, SymphonyElixirWeb.Endpoint, http: [ip: {127, 0, 0, 1}, port: port]
end
