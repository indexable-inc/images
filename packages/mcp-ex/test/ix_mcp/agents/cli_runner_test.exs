defmodule IxMcp.Agents.CliRunnerTest do
  # async: false: exercises the application's shared harness and ledger.
  use ExUnit.Case, async: false

  alias IxMcp.Agents

  @fixtures Path.expand("../../fixtures", __DIR__)

  defp stub!(tmp, fixture) do
    path = Path.join(tmp, "stub-#{fixture}")
    File.write!(path, "#!/bin/sh\nexec cat #{Path.join(@fixtures, fixture)}\n")
    File.chmod!(path, 0o755)
    path
  end

  @tag :tmp_dir
  test "claude child: recorded stream to final, events, graph", %{tmp_dir: tmp} do
    bin = stub!(tmp, "claude-oneshot.ndjson")
    {:ok, id} = Agents.spawn("say ok", backend: :claude, bin: bin, name: "rt-claude")

    assert {:ok, "ok"} = Agents.await(id, 10_000)
    assert {:ok, final} = Agents.report() |> Map.fetch("rt-claude")
    assert final == {:ok, "ok"}

    kinds = id |> Agents.events() |> Enum.map(& &1.kind)
    assert :init in kinds
    assert :result in kinds
    assert :text in kinds

    assert %{nodes: nodes, edges: edges} = Agents.graph()
    assert Enum.any?(nodes, &(&1["id"] == "rt-claude" and &1["state"] == "done"))
    assert ["lead", "rt-claude"] in edges
  end

  @tag :tmp_dir
  test "codex child: a failed turn surfaces as an error to the lead", %{tmp_dir: tmp} do
    bin = stub!(tmp, "codex-oneshot.jsonl")
    {:ok, id} = Agents.spawn("say ok", backend: :codex, bin: bin, name: "rt-codex")

    assert {:error, message} = Agents.await(id, 10_000)
    assert message =~ "out of credits"

    assert %{nodes: nodes, edges: _} = Agents.graph()
    assert Enum.any?(nodes, &(&1["id"] == "rt-codex" and &1["state"] == "blocked"))
  end
end
