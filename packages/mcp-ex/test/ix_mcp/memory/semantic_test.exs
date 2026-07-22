defmodule IxMcp.Memory.SemanticTest do
  # Mutates process-global env and drives the singleton port owner, so
  # never async. The weave binary is mocked (test/fixtures/weave_mock.exs):
  # these tests pin the plumbing -- the NDJSON --stdin protocol, the
  # fact join, residency, and respawn keying -- and run everywhere,
  # because the real recall path needs a weave with #339 and its default
  # model is a 2.4GB Hugging Face download; the real-binary round trip
  # lives in IxMcp.Memory.SemanticIntegrationTest below.
  use ExUnit.Case, async: false

  alias IxMcp.Memory

  @mock Path.expand("../../fixtures/weave_mock.exs", __DIR__)
  @env ~w(WEAVE_MEMORY_STORE WEAVE_BIN WEAVE_MOCK_LOG)

  setup do
    previous = Map.new(@env, &{&1, System.get_env(&1)})

    store =
      Path.join(System.tmp_dir!(), "ix-semantic-test-#{System.unique_integer([:positive])}")

    File.mkdir_p!(store)
    log = Path.join(store, "mock.log")
    System.put_env("WEAVE_MEMORY_STORE", store)
    System.put_env("WEAVE_BIN", @mock)
    System.put_env("WEAVE_MOCK_LOG", log)
    stop_port_owner()

    on_exit(fn ->
      stop_port_owner()

      for {key, value} <- previous do
        if value, do: System.put_env(key, value), else: System.delete_env(key)
      end

      File.rm_rf!(store)
    end)

    {:ok, store: store, log: log}
  end

  test "hits join to facts and ride one resident process", %{log: log} do
    assert [alpha, orphan] = Memory.semantic("what broke the build", limit: 2)

    assert alpha.entity == "mem:alpha"
    assert alpha.similarity == 0.91
    assert alpha.desc == "alpha hook"
    assert alpha.type == "project"
    assert alpha.topic == ["nix"]
    assert alpha.id == "blake3:aaaa"
    assert alpha.seq == 3
    assert %DateTime{} = alpha.time

    # A hit without mem/desc keeps its score and document label only.
    assert orphan == %{
             entity: "note:orphan",
             similarity: 0.42,
             desc: "orphan label",
             id: nil,
             seq: nil,
             time: nil,
             type: nil,
             topic: [],
             handle: nil,
             body: nil,
             verified_at: nil
           }

    # limit truncates client-side: the port over-fetches at a fixed -k.
    assert [%{entity: "mem:alpha"}] = Memory.semantic("second question", limit: 1)

    # Multi-line queries flatten to fit the one-query-per-line protocol.
    assert [_, _] = Memory.semantic("third\n  question")

    # One spawn (the positional query), then resident stdin answers.
    assert mock_events(log) == [
             "spawn what broke the build",
             "line second question",
             "line third question"
           ]
  end

  test "a changed store retires the resident process", %{log: log, store: store} do
    assert [_, _] = Memory.semantic("first")

    other = store <> "-b"
    File.mkdir_p!(other)
    on_exit(fn -> File.rm_rf!(other) end)
    System.put_env("WEAVE_MEMORY_STORE", other)

    assert [_, _] = Memory.semantic("second")
    assert mock_events(log) == ["spawn first", "spawn second"]
  end

  test "a blank query is rejected before any spawn", %{log: log} do
    assert_raise ArgumentError, ~r/non-empty/, fn -> Memory.semantic(" \n ") end
    assert mock_events(log) == []
  end

  defp stop_port_owner do
    case Process.whereis(IxMcp.Memory.Semantic) do
      nil -> :ok
      pid -> GenServer.stop(pid)
    end
  end

  defp mock_events(log) do
    case File.read(log) do
      {:ok, content} -> String.split(content, "\n", trim: true)
      {:error, :enoent} -> []
    end
  end
end

weave = System.get_env("WEAVE_BIN") || System.find_executable("weave")

# The real-binary round trip runs only where a weave with semantic recall
# (indexable-inc/weave#339) is installed; like IxMcp.MemoryTest it
# compiles away in the sandboxed nix check, where weave is not staged.
# The fixture embedder (deterministic hash, no model fetch, no GPU) makes
# a throwaway store embeddable without the default model's 2.4GB
# download, so what this asserts is the plumbing -- ranking, joining,
# residency -- not retrieval quality.
if weave && match?({_, 0}, System.cmd(weave, ["recall", "--help"], stderr_to_stdout: true)) do
  defmodule IxMcp.Memory.SemanticIntegrationTest do
    use ExUnit.Case, async: false

    alias IxMcp.Memory

    @weave weave
    @env ~w(WEAVE_MEMORY_STORE WEAVE_BIN WEAVE_EMBED_MODEL)

    setup do
      previous = Map.new(@env, &{&1, System.get_env(&1)})

      store =
        Path.join(System.tmp_dir!(), "ix-semantic-int-#{System.unique_integer([:positive])}")

      {_, 0} = System.cmd(@weave, ["--store", store, "init"])
      System.put_env("WEAVE_MEMORY_STORE", store)
      System.put_env("WEAVE_BIN", @weave)
      System.put_env("WEAVE_EMBED_MODEL", "fixture")
      stop_port_owner()

      on_exit(fn ->
        stop_port_owner()

        for {key, value} <- previous do
          if value, do: System.put_env(key, value), else: System.delete_env(key)
        end

        File.rm_rf!(store)
      end)

      :ok
    end

    test "ranks remembered memories and joins the enriched fields" do
      Memory.remember("gpu-lore", "metal shader debugging notes",
        type: "reference",
        topic: ["gpu"],
        handle: "xcrun metal"
      )

      Memory.remember("nix-lint", "run the repo lint before committing")

      rows = Memory.semantic("debugging shaders on metal")

      # `weave init` seeds demo entities (agent:main, prefab:*), so the
      # store embeds more documents than the two remembered here.
      entities = Enum.map(rows, & &1.entity)
      assert "mem:gpu-lore" in entities
      assert "mem:nix-lint" in entities
      assert Enum.all?(rows, &is_float(&1.similarity))

      similarities = Enum.map(rows, & &1.similarity)
      assert similarities == Enum.sort(similarities, :desc)

      row = Enum.find(rows, &(&1.entity == "mem:gpu-lore"))
      assert row.desc == "metal shader debugging notes"
      assert row.type == "reference"
      assert row.topic == ["gpu"]
      assert row.handle == "xcrun metal"
      assert String.starts_with?(row.id, "blake3:")

      # The second query rides the already-loaded resident process.
      assert [_ | _] = Memory.semantic("lint gate", limit: 1)
    end

    defp stop_port_owner do
      case Process.whereis(IxMcp.Memory.Semantic) do
        nil -> :ok
        pid -> GenServer.stop(pid)
      end
    end
  end
end
