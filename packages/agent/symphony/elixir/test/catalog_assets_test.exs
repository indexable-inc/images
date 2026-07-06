defmodule SymphonyElixir.CatalogAssetsTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.DSL.Parser
  alias SymphonyElixir.Skill

  @root Path.expand("../..", __DIR__)
  @example_workflows_dir Path.join([@root, "workflows", "example", "workflows"])
  @example_skills_dir Path.join([@root, "workflows", "example", "skills"])

  test "all shipped workflow files parse" do
    results =
      @root
      |> Path.join("workflows/*/workflows/*.sym")
      |> Path.wildcard()
      |> Enum.sort()
      |> Enum.map(fn path -> {path, Parser.parse(File.read!(path), file: path)} end)

    refute results == []

    for {path, result} <- results do
      assert {:ok, %{kind: :workflow}} = result, "expected #{path} to parse, got #{inspect(result)}"
    end
  end

  test "all shipped skill files load" do
    assert @root
           |> Path.join("workflows/*/skills/*.md")
           |> Path.wildcard()
           |> Enum.sort()
           |> Enum.map(&Skill.load/1)
           |> Enum.all?(&match?({:ok, %Skill{}}, &1))
  end

  test "indexable pack insights workflow fires daily via cron" do
    source = File.read!(Path.join([@root, "workflows", "indexable", "workflows", "insights.sym"]))
    assert {:ok, workflow} = Parser.parse(source, file: "insights.sym")

    assert workflow.name == "insights"
    # The cron kind and zone are the load-bearing contract: Triggers.Cron
    # selects on them, and the zone keeps 9am Pacific across DST.
    assert %{kind: :cron, schedule: "0 9" <> _, timezone: "America/Los_Angeles"} = workflow.trigger

    binds = for {:bind, name, _expr} <- workflow.statements, do: name
    assert binds == ["insights"]
  end

  test "example workflow pack is safe and manual-only" do
    source = File.read!(Path.join(@example_workflows_dir, "inspect.sym"))
    assert {:ok, workflow} = Parser.parse(source, file: "inspect.sym")
    assert {:ok, skill} = Skill.load(Path.join(@example_skills_dir, "inspect.md"))

    assert workflow.name == "inspect"
    assert workflow.trigger == %{kind: :manual}

    binds = for {:bind, name, _expr} <- workflow.statements, do: name
    assert binds == ["inspect"]

    assert skill.tools == []
  end
end
