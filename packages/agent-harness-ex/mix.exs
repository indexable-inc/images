defmodule AgentHarness.MixProject do
  use Mix.Project

  def project do
    [
      app: :agent_harness,
      version: "0.1.0",
      elixir: "~> 1.18",
      start_permanent: Mix.env() == :prod,
      elixirc_options: [warnings_as_errors: true],
      deps: deps()
    ]
  end

  # A library: no application module. The host (ix-mcp-ex) places
  # `AgentHarness` under its own supervision tree via child_spec/1.
  def application do
    [extra_applications: [:logger]]
  end

  defp deps do
    [
      # Static-analysis gate, test-only so the library carries zero runtime
      # deps; the sandboxed check runs `mix credo` in :test where the deps
      # FOD provides it.
      {:credo, "~> 1.7", only: :test, runtime: false},
      # ExSlop rides the same Credo run as a plugin (lib/elixir/credo.exs):
      # checks aimed at LLM failure modes, which agent-written code needs
      # most (#3876). Clone detection is NOT added here: the tree-wide
      # `nix run .#clone` ratchet already scans .ex/.exs.
      {:ex_slop, "~> 0.4", only: :test, runtime: false}
    ]
  end
end
