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

  # exqlite is the one runtime dependency: the action log (#3512) appends
  # every tools/call to a local SQLite file, and SQLite needs a NIF. The MCP
  # wire format itself still rides OTP >= 27's built-in JSON module.
  defp deps do
    [
      {:exqlite, "~> 0.39"},
      # The Fable 5 async-subagents harness (packages/agent-harness-ex,
      # #3700): agents as supervised processes with mailbox-backed Send/Wait
      # semantics. A path dep so the harness rides into the release; the MCP
      # tool surface over it is follow-up work, nothing is exposed yet.
      {:agent_harness, path: "../agent-harness-ex"},
      # Static-analysis gate, test-only so the sandboxed check runs `mix credo`
      # offline where the deps FOD provides it.
      {:credo, "~> 1.7", only: :test, runtime: false}
    ]
  end
end
