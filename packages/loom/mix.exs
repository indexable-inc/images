defmodule Loom.MixProject do
  use Mix.Project

  def project do
    [
      app: :loom,
      version: "0.1.0",
      elixir: "~> 1.18",
      start_permanent: Mix.env() == :prod,
      elixirc_options: [warnings_as_errors: true],
      elixirc_paths: elixirc_paths(Mix.env()),
      deps: deps()
    ]
  end

  def application do
    [
      extra_applications: [:logger],
      mod: {Loom.Application, []}
    ]
  end

  defp elixirc_paths(:test), do: ["lib", "test/support"]
  defp elixirc_paths(_env), do: ["lib"]

  defp deps do
    [
      # Same static-analysis lane as agent-harness-ex: test-only, so the
      # library carries zero runtime deps.
      {:credo, "~> 1.7", only: :test, runtime: false},
      {:ex_slop, "~> 0.4", only: :test, runtime: false}
    ]
  end
end
