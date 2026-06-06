defmodule SymphonyElixir.MixProject do
  use Mix.Project

  def project do
    [
      app: :symphony_elixir,
      version: "0.2.0",
      elixir: "~> 1.19",
      compilers: [:phoenix_live_view] ++ Mix.compilers(),
      start_permanent: Mix.env() == :prod,
      deps: deps(),
      aliases: aliases(),
      test_coverage: [tool: ExCoveralls],
      dialyzer: [
        plt_add_apps: [:mix, :ex_unit],
        plt_core_path: "priv/plts",
        plt_local_path: "priv/plts"
      ]
    ]
  end

  def cli do
    [
      preferred_envs: [
        coveralls: :test,
        "coveralls.detail": :test,
        "coveralls.post": :test,
        "coveralls.html": :test,
        "coveralls.json": :test
      ]
    ]
  end

  def application do
    [
      mod: {SymphonyElixir.Application, []},
      extra_applications: [:logger]
    ]
  end

  defp deps do
    [
      {:bandit, "~> 1.8"},
      {:phoenix, "~> 1.8.0"},
      {:phoenix_html, "~> 4.2"},
      {:phoenix_live_view, "~> 1.1.0"},
      {:req, "~> 0.5"},
      {:jason, "~> 1.4"},
      {:yaml_elixir, "~> 2.12"},
      # Pure-BEAM markdown render + sanitize for the dashboard. Both are
      # NIF-free (earmark is pure Elixir, html_sanitize_ex rides on the
      # pure-Erlang mochiweb), so the runtime mix build stays portable on
      # NixOS where precompiled dynamically-linked NIFs break.
      {:earmark, "~> 1.4"},
      {:html_sanitize_ex, "~> 1.5"},
      # Phoenix channel client: a runtime worker dials the control plane's
      # /worker socket and serves provision/teardown over it.
      {:slipstream, "~> 1.1"},
      {:lazy_html, ">= 0.1.0", only: :test},
      {:credo, "~> 1.7", only: [:dev, :test], runtime: false},
      {:dialyxir, "~> 1.4", only: [:dev, :test], runtime: false},
      {:sobelow, "~> 0.13", only: [:dev, :test], runtime: false},
      {:mix_audit, "~> 2.1", only: [:dev, :test], runtime: false},
      {:excoveralls, "~> 0.18", only: :test}
    ]
  end

  defp aliases do
    [
      setup: ["deps.get", "compile --warnings-as-errors"],
      build: ["compile --warnings-as-errors"],
      lint: ["credo"],
      quality: [
        "format --check-formatted",
        # Non-strict so credo respects the :low priority the checks carry in
        # .credo.exs; --strict surfaces and fails on those informational
        # refactor/readability suggestions, defeating that config.
        "credo",
        "sobelow --config",
        # decimal 2.x is pinned by ecto and solid (both require ~> 2.0), so the
        # only patched release (3.0.0) is unreachable until they move upstream.
        # https://github.com/advisories/GHSA-rhv4-8758-jx7v
        "deps.audit --ignore-advisory-ids GHSA-rhv4-8758-jx7v",
        "dialyzer"
      ]
    ]
  end
end
