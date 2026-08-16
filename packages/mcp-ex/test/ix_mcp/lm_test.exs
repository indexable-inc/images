defmodule IxMcp.LMTest do
  use ExUnit.Case, async: false

  import ExUnit.CaptureLog

  alias IxMcp.ActionLog
  alias IxMcp.Ctx
  alias IxMcp.EventLog
  alias IxMcp.LM
  alias IxMcp.LM.Budget
  alias IxMcp.LM.Stub

  setup do
    unique = System.unique_integer([:positive])
    path = Path.join(System.tmp_dir!(), "ix-mcp-rlm-test-#{unique}.db")
    cas = Path.join(System.tmp_dir!(), "ix-mcp-rlm-cas-#{unique}")
    log = start_supervised!({ActionLog, path: path, name: :"rlm_log_#{unique}"})

    # Isolation on three axes: its own database, its own CAS, and its own
    # workspace, so neither the developer's real ledger nor a sibling test's
    # budget window is touched.
    Application.put_env(:ix_mcp, :action_log_server, log)
    Application.put_env(:ix_mcp, :rlm_cas, cas)
    Application.put_env(:ix_mcp, :lm_backend, Stub)
    Application.put_env(:ix_mcp, :lm_budget, calls: 100, tokens: 1_000_000)
    Process.put(:ix_workspace, "test-#{unique}")
    Stub.reset()
    Budget.reset()

    on_exit(fn ->
      Application.delete_env(:ix_mcp, :action_log_server)
      Application.delete_env(:ix_mcp, :rlm_cas)
      Application.delete_env(:ix_mcp, :lm_backend)
      Application.delete_env(:ix_mcp, :lm_budget)
      Application.delete_env(:ix_mcp, :lm_stub)
      File.rm(path)
      File.rm_rf(cas)
    end)

    %{log: log, cas: cas, unique: unique}
  end

  defp kinds, do: EventLog.events(limit: 100) |> Enum.map(& &1.kind)

  describe "memoization" do
    test "the same question over the same bytes is answered from the log" do
      handle = Ctx.binary("alpha\nbeta\ngamma\n")

      assert {:ok, first} = LM.ask("what is here?", ctx: handle)
      assert Stub.calls() == 1

      assert {:ok, second} = LM.ask("what is here?", ctx: handle)
      # The instrument that makes this mean something: the provider was not
      # reached again, and the answer is the SAME one rather than a fresh one
      # that happens to match.
      assert Stub.calls() == 1
      assert second == first

      assert kinds() == [:lm_ask, :lm_result, :lm_cache_hit]
    end

    test "a different prompt, context or model is a different derivation" do
      handle = Ctx.binary("alpha\n")
      other = Ctx.binary("omega\n")

      assert {:ok, _} = LM.ask("q", ctx: handle)
      assert {:ok, _} = LM.ask("q different", ctx: handle)
      assert {:ok, _} = LM.ask("q", ctx: other)
      assert {:ok, _} = LM.ask("q", ctx: handle, model: "claude-sonnet-4-5")

      assert Stub.calls() == 4
    end

    test "a slice of the same content keys differently from the whole" do
      whole = Ctx.binary("alpha\nbeta\n")
      slice = Ctx.slice(whole, lines: 1..1)

      refute LM.cache_key("q", ctx: whole) == LM.cache_key("q", ctx: slice)
    end

    test "cache: false skips the read but still records for the next caller" do
      handle = Ctx.binary("alpha\n")

      assert {:ok, _} = LM.ask("q", ctx: handle)
      assert {:ok, _} = LM.ask("q", ctx: handle, cache: false)
      assert Stub.calls() == 2

      # Opting out of a hit is not opting out of being useful.
      assert {:ok, _} = LM.ask("q", ctx: handle)
      assert Stub.calls() == 2
    end

    test "a result too big to inline round-trips through the CAS" do
      big = String.duplicate("y", EventLog.inline_limit() * 2)
      Application.put_env(:ix_mcp, :lm_stub, fn _request -> big end)

      assert {:ok, ^big} = LM.ask("q")

      cached = EventLog.cached(LM.cache_key("q"))
      assert cached.content_id != nil
      assert cached.payload["spilled"]
      assert cached.payload["text"] == big

      assert {:ok, ^big} = LM.ask("q")
      assert Stub.calls() == 1
    end

    test "a spilled payload whose blob is gone is a miss, not a wrong answer" do
      big = String.duplicate("z", EventLog.inline_limit() * 2)
      Application.put_env(:ix_mcp, :lm_stub, fn _request -> big end)

      assert {:ok, ^big} = LM.ask("q")
      File.rm_rf!(Application.get_env(:ix_mcp, :rlm_cas))

      assert EventLog.cached(LM.cache_key("q")) == nil
      assert {:ok, ^big} = LM.ask("q")
      assert Stub.calls() == 2
    end
  end

  describe "budget" do
    test "an exhausted call budget refuses instead of calling" do
      Application.put_env(:ix_mcp, :lm_budget, calls: 2, tokens: 1_000_000)

      assert {:ok, _} = LM.ask("one")
      assert {:ok, _} = LM.ask("two")
      assert LM.ask("three") == {:error, :budget_exhausted}

      # Fail CLOSED: the third call never reached the provider, and the log
      # says so rather than the run looking two-thirds successful.
      assert Stub.calls() == 2
      assert :lm_budget_refused in kinds()
    end

    test "an exhausted token budget refuses too" do
      Application.put_env(:ix_mcp, :lm_budget, calls: 100, tokens: 10)

      assert LM.ask(String.duplicate("q", 400)) == {:error, :budget_exhausted}
      assert Stub.calls() == 0
    end

    test "spend is trued up from the estimate to what the provider reported" do
      assert {:ok, _} = LM.ask("q", max_tokens: 100_000)
      state = LM.budget()

      # max_tokens was pre-charged; settling replaced it with the real cost, so
      # a conservative estimate does not eat the window.
      assert state.calls == 1
      assert state.tokens < 1_000
      assert state.tokens_left > 999_000
    end

    test "budgets are per workspace" do
      Application.put_env(:ix_mcp, :lm_budget, calls: 1, tokens: 1_000_000)

      assert {:ok, _} = LM.ask("q")
      assert LM.ask("q2") == {:error, :budget_exhausted}

      Process.put(:ix_workspace, "another-workspace-#{System.unique_integer([:positive])}")
      Budget.reset()
      assert {:ok, _} = LM.ask("q2")
    end

    test "a fan-out meters against the caller's workspace, not the default" do
      handles = Enum.map(1..6, &Ctx.binary("chunk #{&1}\n"))

      results = LM.map(handles, fn _handle -> "summarize" end, concurrency: 3)

      assert [_, _, _, _, _, _] = results
      assert Enum.all?(results, &match?({:ok, _}, &1))
      # Tasks do not inherit the process dictionary; without propagation these
      # six calls would be metered under a different workspace entirely.
      assert LM.budget().calls == 6
    end

    test "a fan-out that exhausts the budget reports errors, not fewer answers" do
      Application.put_env(:ix_mcp, :lm_budget, calls: 3, tokens: 1_000_000)
      handles = Enum.map(1..6, &Ctx.binary("chunk #{&1}\n"))

      results = LM.map(handles, fn _handle -> "summarize" end, concurrency: 1)

      assert [_, _, _, _, _, _] = results
      assert Enum.count(results, &match?({:ok, _}, &1)) == 3
      assert Enum.count(results, &(&1 == {:error, :budget_exhausted})) == 3
    end
  end

  describe "map/3" do
    test "results line up with the inputs and each handle becomes its own context" do
      test_pid = self()

      Application.put_env(:ix_mcp, :lm_stub, fn request ->
        send(test_pid, {:prompt, request.prompt})
        "answer for " <> List.last(Regex.run(~r/chunk (\d+)/, request.prompt))
      end)

      handles = Enum.map(1..5, &Ctx.binary("chunk #{&1}\n"))
      results = LM.map(handles, fn _handle -> "which chunk is this?" end, concurrency: 5)

      assert results == Enum.map(1..5, &{:ok, "answer for #{&1}"})

      prompts = for _ <- 1..5, do: receive(do: ({:prompt, prompt} -> prompt))
      assert Enum.all?(prompts, &(&1 =~ ~r/<context id="[0-9a-f]{64}:0\+8"/))
      assert Enum.all?(prompts, &(&1 =~ "which chunk is this?"))
    end

    test "prompt_fun may return per-item options" do
      handles = [Ctx.binary("a\n"), Ctx.binary("bb\nb\n")]

      results = LM.map(handles, fn handle -> {"q", [model: "model-#{handle.lines}"]} end)

      assert Enum.all?(results, &match?({:ok, _}, &1))

      models =
        EventLog.events(kind: :lm_ask, limit: 10)
        |> Enum.map(& &1.payload["model"])
        |> Enum.sort()

      assert models == ["model-1", "model-2"]
    end
  end

  describe "errors" do
    test "mode: :rlm refuses rather than silently doing the shallow thing" do
      assert LM.ask("q", mode: :rlm) == {:error, :rlm_mode_unimplemented}
      assert Stub.calls() == 0
      assert kinds() == []
    end

    test "an unknown mode is an error, not a default" do
      assert LM.ask("q", mode: :deep) == {:error, {:unknown_mode, :deep}}
    end

    test "a provider error is returned and logged, and is not cached" do
      Application.put_env(:ix_mcp, :lm_stub, fn _request -> {:error, :boom} end)

      assert LM.ask("q") == {:error, :boom}
      assert :lm_error in kinds()
      refute :lm_result in kinds()

      # A failure must never be memoized as an answer.
      assert EventLog.cached(LM.cache_key("q")) == nil
    end
  end

  describe "event log" do
    test "the kinds the log declares are exactly the kinds storage can revive" do
      {:ok, types} = Code.Typespec.fetch_types(EventLog)
      declared = for {:type, {:kind, spec, _args}} <- types, atom <- type_atoms(spec), do: atom

      # Two-sided: a kind added to EventLog's @type without teaching storage
      # would come back as a string, and a kind storage revives that the log no
      # longer declares is dead vocabulary. Either direction fails here.
      assert declared != []
      assert Enum.sort(declared) == Enum.sort(ActionLog.rlm_kinds())
    end

    test "a kind this build does not know reads back as a string, not a crash" do
      assert EventLog.append(%{kind: :lm_from_a_newer_build, payload: %{}}) > 0

      [event] = EventLog.events(kind: :lm_from_a_newer_build, limit: 10)

      assert event.kind == "lm_from_a_newer_build"
    end

    test "an lm_ask row carries the accounting a spend audit needs" do
      handle = Ctx.binary("alpha\n")
      assert {:ok, _} = LM.ask("q", ctx: handle)

      [ask | _rest] = EventLog.events(kind: :lm_ask, limit: 10)

      assert ask.payload["ctx_ids"] == [Ctx.key(handle)]
      assert ask.payload["tokens_in"] > 0
      assert ask.payload["prompt_hash"] =~ ~r/^[0-9a-f]{64}$/
      assert ask.payload["latency_ms"] >= 0
      assert ask.workspace == to_string(IxMcp.Workspace.current())
      assert {:ok, %DateTime{}, 0} = DateTime.from_iso8601(ask.ts)
    end

    test "fold/3 walks the whole log in batches and resumes from a cursor" do
      for i <- 1..5, do: {:ok, _} = LM.ask("q#{i}")

      assert EventLog.fold(0, fn _event, acc -> acc + 1 end, batch: 2) == 10

      [first | _rest] = EventLog.events(limit: 1)
      assert EventLog.fold(0, fn _event, acc -> acc + 1 end, after: first.seq, batch: 2) == 9
    end

    test "kinds can be filtered" do
      for i <- 1..3, do: {:ok, _} = LM.ask("q#{i}")

      assert [_, _, _] = EventLog.events(kind: :lm_ask, limit: 10)
      assert [_, _, _, _, _, _] = EventLog.events(kind: [:lm_ask, :lm_result], limit: 10)
    end

    test "a degraded log records nothing and claims no cached result", %{unique: unique} do
      # A file belonging to a newer server puts the log in :disabled (#3539).
      # Every RLM answer there must be the fail-closed one: 0 for "not
      # recorded", nil for the cache probe, [] for a read. A cache that
      # guessed here would answer a whole analysis from results it does not
      # have, and a spend audit would under-report by however much ran.
      path = Path.join(System.tmp_dir!(), "rlm-future-#{System.unique_integer([:positive])}.db")
      {:ok, conn} = Exqlite.Sqlite3.open(path)
      :ok = Exqlite.Sqlite3.execute(conn, "PRAGMA user_version = 9000")
      :ok = Exqlite.Sqlite3.close(conn)
      on_exit(fn -> File.rm(path) end)

      {log, output} =
        with_log(fn ->
          # Its own child id: the setup already holds an ActionLog under the
          # module's default spec id, and two children cannot share one.
          start_supervised!({ActionLog, path: path, name: :"rlm_disabled_#{unique}"},
            id: :rlm_disabled
          )
        end)

      assert output =~ "user_version 9000"

      Application.put_env(:ix_mcp, :action_log_server, log)

      assert EventLog.append(%{kind: :lm_ask, payload: %{}, text: "x"}) == 0
      assert EventLog.events(limit: 10) == []
      assert EventLog.cached(LM.cache_key("q")) == nil

      # And LM still works: a call that cannot be memoized is still a call.
      assert {:ok, _} = LM.ask("q")
      assert Stub.calls() == 1
    end
  end

  test "cache_key/2 is stable, and public so a cache can be inspected" do
    handle = Ctx.binary("alpha\n")

    assert LM.cache_key("q", ctx: handle) == LM.cache_key("q", ctx: handle)
    assert LM.cache_key("q", ctx: handle) =~ ~r/^[0-9a-f]{64}$/
  end

  # Erlang type forms: a union of atom literals, which is what `@type kind` is.
  defp type_atoms({:atom, _line, atom}), do: [atom]
  defp type_atoms({:type, _line, :union, members}), do: Enum.flat_map(members, &type_atoms/1)
  defp type_atoms(_other), do: []
end
