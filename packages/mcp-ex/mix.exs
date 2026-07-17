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

  # Zero runtime dependencies on purpose: OTP >= 27 ships a JSON module
  # (exposed as `JSON` in Elixir 1.18), so the MCP wire format needs no hex
  # package and the Nix build needs no Mix-deps fixed-output derivation.
  defp deps do
    []
  end
end
