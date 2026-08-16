defmodule IxMcp.StdlibTest do
  use ExUnit.Case, async: false

  alias IxMcp.ActionLog
  alias IxMcp.EventLog
  alias IxMcp.Stdlib
  alias IxMcp.Workspace

  # The negative control for the provenance gate. It is deliberately NOT under
  # `IxMcp.Stdlib.` -- a resident without provenance would fail the gate below,
  # which is the whole point -- so this is a module of the same shape that the
  # checker must reject.
  defmodule NoProvenance do
    @moduledoc "A module whose reason for existing nobody wrote down."
    def noop, do: :ok
  end

  defmodule NoDocAtAll do
    @moduledoc false
    def noop, do: :ok
  end

  setup do
    unique = System.unique_integer([:positive])
    path = Path.join(System.tmp_dir!(), "ix-mcp-stdlib-test-#{unique}.db")
    log = start_supervised!({ActionLog, path: path, name: :"stdlib_log_#{unique}"})
    Application.put_env(:ix_mcp, :action_log_server, log)
    Process.put(:ix_workspace, "test-#{unique}")

    on_exit(fn ->
      Application.delete_env(:ix_mcp, :action_log_server)
      File.rm(path)
    end)

    %{log: log}
  end

  describe "discovery" do
    test "a module under lib/ix_mcp/stdlib is a resident with nothing to register" do
      assert IxMcp.Stdlib.Forge in Stdlib.modules()
    end

    # The registry is not one of the things it registers, and neither is a
    # resident's own private submodule: exactly one segment under the prefix.
    test "the registry itself and any nested helper are not residents" do
      residents = Stdlib.modules()

      refute Stdlib in residents

      for module <- residents do
        assert ["Elixir", "IxMcp", "Stdlib", _one] = String.split(Atom.to_string(module), ".")
      end
    end

    # M4. Residents are aliased AFTER the core aliases, so a resident whose last
    # segment collides with a core one silently REBINDS that name in every cell:
    # an `IxMcp.Stdlib.Jobs` would make `Jobs` mean something else everywhere,
    # and no test of either module would notice. A collision is a naming decision
    # for a human, so the resident stays reachable by its full name and says so.
    test "a resident that would shadow a core alias is left out of the prelude" do
      core = Workspace.core_aliases()

      assert "Jobs" in core, "the core prelude no longer binds Jobs; pick another collision"

      assert Stdlib.shadows_core?(IxMcp.Stdlib.Jobs)
      refute Stdlib.shadows_core?(IxMcp.Stdlib.Forge)

      # ...and the list is checked against what a CELL actually sees, not against
      # the string it was derived from: a name claimed as core but bound nowhere
      # would make this gate refuse a resident for no reason, and a core alias
      # missing from the list would let one shadow it silently.
      Workspace.reset()
      {_binding, env} = Workspace.snapshot()

      bound =
        Enum.map(env.aliases, fn {short, _module} -> short |> Module.split() |> List.last() end)

      for name <- core do
        assert name in bound,
               "#{name} is claimed as a core alias but no cell binds it"
      end
    end

    test "the prelude aliases every resident, in a stable order" do
      prelude = Stdlib.prelude()

      assert prelude =~ "alias IxMcp.Stdlib.Forge"
      assert prelude == Stdlib.prelude()

      for module <- Stdlib.modules() do
        assert prelude =~ "alias #{inspect(module)}"
      end
    end

    # A resident that exports MACROS needs `require` on top of the alias, or a
    # cell's `Resident.macro "..." do ... end` parses as an ordinary function
    # call and dies on its own body's markers. Forge, the first resident,
    # exported no macros, so an alias-only prelude looked complete. Asserted for
    # EVERY resident rather than only the macro-exporting ones, because "does
    # this module export a macro" is not a question the prelude should have to
    # answer -- and `require` is inert where it is not needed.
    test "the prelude requires every resident, not just aliases it" do
      prelude = Stdlib.prelude()

      for module <- Stdlib.modules() do
        assert prelude =~ "require #{inspect(module)}"
      end

      # The END of the wiring, not the middle, matching how the alias gate above
      # is taken all the way to a cell's environment: the string proves the
      # prelude SAYS require, env.requires proves a cell actually got it. A
      # resident whose macros are unusable would pass the string check.
      Workspace.reset()
      {_binding, env} = Workspace.snapshot()

      for module <- Stdlib.modules() do
        assert module in env.requires,
               "#{inspect(module)} is aliased but never required, so its macros do not work in a cell"
      end
    end

    # The end of the wiring, not the middle: a cell's environment must resolve
    # the bare name. Asserting on the alias table proves what a cell would see
    # without needing to evaluate one.
    test "a fresh workspace resolves the bare resident name" do
      Workspace.reset()
      {_binding, env} = Workspace.snapshot()

      assert {Forge, IxMcp.Stdlib.Forge} in env.aliases
    end
  end

  describe "provenance" do
    test "every resident states why it exists" do
      for module <- Stdlib.modules() do
        assert {:ok, prose} = Stdlib.provenance(module),
               "#{inspect(module)} has no #{Stdlib.provenance_heading()} section in its @moduledoc"

        assert String.length(prose) > 40
      end
    end

    # The gate must be able to say no, or its yes means nothing: same shape of
    # module, no provenance section, and no doc at all.
    test "a module without a provenance section is refused" do
      assert {:error, :missing} = Stdlib.provenance(NoProvenance)
      assert {:error, :missing} = Stdlib.provenance(NoDocAtAll)
    end
  end

  describe "fitness" do
    test "a resident call is recorded with its own outcome vocabulary" do
      assert {:passed, :fine} =
               Stdlib.observe(IxMcp.Stdlib.Forge, :land, fn -> {:passed, :fine} end)

      assert {:failed, :nope} =
               Stdlib.observe(IxMcp.Stdlib.Forge, :land, fn -> {:failed, :nope} end)

      assert :ok = Stdlib.observe(IxMcp.Stdlib.Forge, :await_verdict, fn -> :ok end)

      assert [land, await] = Stdlib.fitness()

      assert land.function == "land"
      assert land.calls == 2
      assert land.outcomes == %{"passed" => 1, "failed" => 1}
      assert await.function == "await_verdict"
      assert await.calls == 1
      assert await.outcomes == %{"ok" => 1}
    end

    test "the record rides rlm_events rather than a table of its own" do
      Stdlib.observe(IxMcp.Stdlib.Forge, :land, fn -> :ok end)

      assert [event] = EventLog.events(kind: :stdlib_call, limit: 10)
      assert event.payload["module"] == "IxMcp.Stdlib.Forge"
      assert event.payload["function"] == "land"
      assert is_integer(event.payload["ms"])
    end

    # An observer that swallows is worse than no observer: the raise reaches
    # the caller unchanged, and the call is still counted.
    test "a raising call is recorded and re-raised" do
      assert_raise RuntimeError, "boom", fn ->
        Stdlib.observe(IxMcp.Stdlib.Forge, :land, fn -> raise "boom" end)
      end

      assert [%{calls: 1, outcomes: %{"raise" => 1}}] = Stdlib.fitness()
    end

    test "an unreadable row is skipped rather than crashing the fold" do
      EventLog.append(%{kind: :stdlib_call, payload: %{"nothing" => "useful"}})
      Stdlib.observe(IxMcp.Stdlib.Forge, :land, fn -> :ok end)

      assert [%{function: "land", calls: 1}] = Stdlib.fitness()
    end

    test "no calls is an empty answer, not an error" do
      assert Stdlib.fitness() == []
    end
  end

  describe "the kind vocabulary" do
    test "storage can revive the fitness kind" do
      assert :stdlib_call in ActionLog.rlm_kinds()

      Stdlib.observe(IxMcp.Stdlib.Forge, :land, fn -> :ok end)

      assert [%{kind: :stdlib_call}] = EventLog.events(kind: :stdlib_call, limit: 10)
    end
  end
end
