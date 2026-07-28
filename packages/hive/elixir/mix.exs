defmodule Hive.MixProject do
  use Mix.Project

  def project do
    [
      app: :hive,
      version: "0.1.0",
      elixir: "~> 1.18",
      start_permanent: Mix.env() == :prod,
      deps: deps()
    ]
  end

  # Run "mix help compile.app" to learn about applications.
  def application do
    [
      extra_applications: [:logger],
      mod: {Hive.Application, []}
    ]
  end

  # Run "mix help deps" to learn about dependencies.
  defp deps do
    [
      # Static-analysis gate, test-only so the `hive` launcher still runs `mix`
      # offline in :dev with no deps; the sandboxed check runs in :test where
      # credo is fetched. runtime: false keeps it out of the released app.
      {:credo, "~> 1.7", only: :test, runtime: false}
    ]
  end
end
