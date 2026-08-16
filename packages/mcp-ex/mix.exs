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
      # Beyond the app's own needs, carry the standard OTP batteries into
      # the release: agents run arbitrary cells, and a REPL kernel without
      # :inets/:ssl cannot make an HTTPS call (#3798). :xmerl for XML,
      # :runtime_tools for dbg/tracing, :tools for fprof/eprof/cover.
      extra_applications: [
        :logger,
        :crypto,
        :inets,
        :ssl,
        :xmerl,
        :runtime_tools,
        :tools
      ],
      mod: {IxMcp.Application, []}
    ]
  end

  # exqlite is the one runtime dependency: the action log (#3512) appends
  # every tools/call to a local SQLite file, and SQLite needs a NIF. The MCP
  # wire format itself still rides OTP >= 27's built-in JSON module.
  defp deps do
    [
      {:exqlite, "~> 0.39"},
      # Agent compatibility affordance: cells written from habit call
      # Jason.decode!/encode! even though OTP >= 27 ships a built-in JSON
      # module, and an UndefinedFunctionError mid-cell wastes a whole
      # exec round trip. Carrying jason in the release makes those calls
      # just work.
      {:jason, "~> 1.4"},
      # The Fable 5 async-subagents harness (packages/agent-harness-ex,
      # #3700): agents as supervised processes with mailbox-backed Send/Wait
      # semantics. A path dep so the harness rides into the release; the MCP
      # tool surface over it is follow-up work, nothing is exposed yet.
      {:agent_harness, path: "../agent-harness-ex"},
      # The fleet warning engine and the one copy of the BEAM mesh client
      # (ENG-12004 adjacent; extracted from this package so test-ide and the
      # kernel stop keeping parallel copies). Policy stays private: this
      # package only names a policy module via config, never its contents.
      {:fleet_mesh, path: "../fleet-mesh"},
      # Static-analysis gate, test-only so the sandboxed check runs `mix credo`
      # offline where the deps FOD provides it.
      {:credo, "~> 1.7", only: :test, runtime: false},
      # ExSlop rides the same Credo run as a plugin (lib/elixir/credo.exs):
      # checks aimed at LLM failure modes, which agent-written code needs
      # most (#3876). Clone detection is NOT added here: the tree-wide
      # `nix run .#clone` ratchet already scans .ex/.exs.
      {:ex_slop, "~> 0.4", only: :test, runtime: false}
    ]
  end
end
