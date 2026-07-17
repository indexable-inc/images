defmodule IxMcp.MixProject do
  use Mix.Project

  def project do
    [
      app: :ix_mcp,
      version: "0.1.0",
      elixir: "~> 1.18",
      start_permanent: Mix.env() == :prod,
      elixirc_options: [warnings_as_errors: true],
      deps: deps()
    ]
  end

  def application do
    [
      extra_applications: [:logger, :crypto],
      mod: {IxMcp.Application, []}
    ]
  end

  # Zero RUNTIME dependencies on purpose: OTP >= 27 ships a JSON module
  # (exposed as `JSON` in Elixir 1.18), so the MCP wire format needs no hex
  # package and the released server carries no Mix deps at all.
  defp deps do
    [
      # Static-analysis gate, test-only so the server runs `mix` offline with
      # no deps; the sandboxed check runs in :test where credo is fetched.
      {:credo, "~> 1.7", only: :test, runtime: false}
    ]
  end
end
