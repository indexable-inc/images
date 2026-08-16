defmodule IxMcp.Stdlib do
  @moduledoc """
  The kernel's GROWN standard library: modules that earned their way in.

  A cell can already `defmodule` anything it likes, and that module lives as
  long as the BEAM does. What it cannot do is survive a restart, be found by
  the next session, or be trusted by a third one. So a useful recipe gets
  retyped from memory instead, and retyping is where it breaks. This module
  is the promotion path from one to the other.

  ## The ladder

    1. SCRATCH. Any `defmodule` in an exec cell. Free, immediate, and gone
       on restart. Nothing here is needed for it, which is the point: the
       cheap rung must stay cheap.
    2. RESIDENT. A module under `lib/ix_mcp/stdlib/` that landed through the
       CI queue. It is compiled with the kernel, so the gate that guards
       everything else guards it too, and it is aliased into every cell with
       no wiring: dropping a file in that directory is the whole promotion.
    3. FITNESS. Every resident call recorded through `observe/3` becomes a
       row in the RLM event log, so `fitness/1` can say which residents are
       load-bearing and which are dead weight. A stdlib that cannot be
       pruned grows monotonically, and then nobody reads it.
    4. PROVENANCE. Every resident's `@moduledoc` carries a `## Provenance`
       section naming the incident or need that created it (`provenance/1`,
       gated two-sided by the test suite). A resident whose reason nobody
       can state is a resident nobody can retire.

  ## Why `lib/ix_mcp/stdlib/` and not `priv/`

  A `priv/stdlib/*.ex` tree read with `Code.compile_file/1` at boot was the
  obvious shape and it is the wrong one twice over: it moves compilation to
  the boot path, so a syntax error in a resident stops the kernel starting
  rather than failing a build, and it puts those files OUTSIDE
  `mix compile --warnings-as-errors`, `mix format --check-formatted`,
  `mix credo --strict` and `mix test`, which is exactly the gate a resident
  is supposed to have passed. Under `lib/` the compiler is the gate, and
  discovery is still free because it reads the application's own module list
  (`Application.spec/2`) rather than a registry somebody has to remember to
  edit.

  ## Static at boot; hot reload is follow-up

  `prelude/0` is evaluated when a workspace is created, so a resident that
  landed after this kernel started is not in it: picking up a new resident
  needs a kernel restart today. Reloading residents when ix main moves
  (recompile, `:code.load_binary/3`, re-alias live workspaces) is deliberate
  follow-up work and not part of this landing.

  ## Provenance

  2026-08-12. Four lanes needed the same forge landing recipe on one day,
  each retyped it from memory or from another lane's transcript, and every
  copy grew a different fault (`IxMcp.Stdlib.Forge` records them). The
  faults were not in anybody's understanding; they were in the copying. A
  kernel whose useful procedures live only in transcripts guarantees that
  outcome, so the procedures need somewhere to live.
  """

  alias IxMcp.EventLog

  require Logger

  @prefix ["Elixir", "IxMcp", "Stdlib"]
  @provenance_heading "## Provenance"

  @typedoc """
  One resident function's record, as `fitness/1` folds it out of the event
  log. `outcomes` counts the leading atom of each call's result, so a lander
  that returned `{:passed, _}` twice and `{:failed, _}` once reads as such.
  """
  @type fitness :: %{
          module: String.t(),
          function: String.t(),
          calls: non_neg_integer(),
          outcomes: %{optional(String.t()) => non_neg_integer()},
          ms_total: non_neg_integer()
        }

  @doc """
  The resident modules, sorted.

  Exactly one segment under `IxMcp.Stdlib`: `IxMcp.Stdlib.Forge` is a
  resident, `IxMcp.Stdlib.Forge.Helper` is that resident's own business and
  is neither listed nor aliased. Read off the application's module list, so
  a file added to the directory is a resident with no registry to update.
  """
  @spec modules() :: [module()]
  def modules do
    :ix_mcp
    |> Application.spec(:modules)
    |> List.wrap()
    |> Enum.filter(&resident?/1)
    |> Enum.sort()
  end

  @doc """
  The `alias` and `require` fragment every cell's environment starts with, or
  `""` when there are no residents.

  Both, not just the alias. A resident that exports MACROS is unusable from a
  cell with an alias alone: `Sh.mutate "..." do ... end` parses as an ordinary
  function call, and the markers in its body die as undefined variables. The
  first resident exported none, so an alias-only prelude looked complete --
  this was a gap nobody had reached, not a decision. `require` is inert for a
  resident that exports only functions, so every resident gets both and no
  resident has to remember to ask.

  Deterministic order, because a prelude that shuffles makes two kernels
  disagree about which module a bare name means.
  """
  @spec prelude() :: String.t()
  def prelude do
    modules()
    |> Enum.reject(&shadows_core?/1)
    |> Enum.map_join("; ", &"alias #{inspect(&1)}; require #{inspect(&1)}")
  end

  # Residents are aliased AFTER the core aliases, so a resident whose last
  # segment collides with a core one would silently rebind that name in every
  # cell: an `IxMcp.Stdlib.Jobs` would make `Jobs` mean something new everywhere,
  # and no test of either module would notice. A collision is a naming decision
  # for a human, so the resident is left un-aliased and says so out loud rather
  # than quietly winning.
  @doc false
  @spec shadows_core?(module()) :: boolean()
  def shadows_core?(module) do
    last = module |> Module.split() |> List.last()

    if last in IxMcp.Workspace.core_aliases() do
      Logger.warning(
        "stdlib resident #{inspect(module)} would shadow the core alias #{last} in every cell; " <>
          "it stays reachable by its full name. Rename it."
      )

      true
    else
      false
    end
  end

  @doc """
  Run `fun`, record the call, and return its result untouched.

  The outcome recorded is the result's leading atom (`{:passed, _}` records
  `"passed"`, a bare `:ok` records `"ok"`), because a resident's own
  vocabulary is more useful than a generic ok/error flattening. A raise is
  recorded as `"raise"` and then re-raised with its original stacktrace: an
  observer that swallows is worse than no observer.

      Stdlib.observe(__MODULE__, :land, fn -> do_land(change, opts) end)
  """
  # Written without a `when result: term()` parametric tail on purpose: the
  # repo's own `astlog-rules/elixir.astlog` public-def-needs-spec rule matches
  # `(arguments (binary_operator left: (call ...)))`, and a `when` clause puts
  # another binary_operator on top, so a parametric spec reads to the gate as NO
  # spec at all. Verified 2026-08-12: zero `@spec ... when` forms exist under any
  # gated `lib/`, so this is a blind spot nobody had hit rather than a rule I am
  # working around.
  @spec observe(module(), atom(), (-> term())) :: term()
  def observe(module, function, fun)
      when is_atom(module) and is_atom(function) and is_function(fun, 0) do
    started = System.monotonic_time(:millisecond)

    try do
      result = fun.()
      record(module, function, outcome(result), started)
      result
    catch
      kind, reason ->
        record(module, function, "raise", started)
        :erlang.raise(kind, reason, __STACKTRACE__)
    end
  end

  @doc """
  What the residents have actually been used for, newest events included,
  busiest first.

  Options are `IxMcp.EventLog.fold/3`'s: `:after` to resume from a cursor,
  `:batch` for rows per read.
  """
  @spec fitness(keyword()) :: [fitness()]
  def fitness(opts \\ []) do
    %{}
    |> EventLog.fold(&tally/2, Keyword.put(opts, :kind, :stdlib_call))
    |> Map.values()
    |> Enum.sort_by(& &1.calls, :desc)
  end

  @doc """
  The `## Provenance` prose from `module`'s `@moduledoc`.

  `{:error, :missing}` covers both a resident with no such section and one
  whose doc chunk is not readable, because a provenance nobody can read is
  not a provenance. The gate on this is a test, which runs against a build
  that keeps its doc chunks; a stripped release answering `:missing` is
  expected and is not a failure.
  """
  @spec provenance(module()) :: {:ok, String.t()} | {:error, :missing}
  def provenance(module) when is_atom(module) do
    with {:ok, doc} <- moduledoc(module),
         [_before, section] <- String.split(doc, @provenance_heading, parts: 2),
         prose = String.trim(section),
         true <- prose != "" do
      {:ok, prose}
    else
      _absent -> {:error, :missing}
    end
  end

  @doc "The heading a resident's `@moduledoc` must carry."
  @spec provenance_heading() :: String.t()
  def provenance_heading, do: @provenance_heading

  # ── internals ─────────────────────────────────────────────────────────

  @spec resident?(module()) :: boolean()
  defp resident?(module) do
    case String.split(Atom.to_string(module), ".") do
      @prefix ++ [_one] -> true
      _other -> false
    end
  end

  @spec moduledoc(module()) :: {:ok, String.t()} | :error
  defp moduledoc(module) do
    case Code.fetch_docs(module) do
      {:docs_v1, _anno, _lang, _format, %{"en" => doc}, _meta, _docs} when is_binary(doc) ->
        {:ok, doc}

      _unreadable ->
        :error
    end
  end

  # One row per call. A resident's calls are seconds-to-minutes operations,
  # not a hot loop, so the row is free relative to the work it describes --
  # and it rides `rlm_events` rather than a table of its own, because a
  # second event store would disagree with the first about what happened.
  #
  # Recording is subordinate to the call, always: a broken event log must not
  # be able to change what the caller sees. Found by live fire -- on a kernel
  # whose event log was absent, a raising call came back as the RECORDER's
  # error with the original one gone, which is an observer editing history.
  @spec record(module(), atom(), String.t(), integer()) :: :ok
  defp record(module, function, outcome, started) do
    EventLog.append(%{
      kind: :stdlib_call,
      payload: %{
        module: inspect(module),
        function: Atom.to_string(function),
        outcome: outcome,
        ms: System.monotonic_time(:millisecond) - started
      }
    })

    :ok
  catch
    kind, reason ->
      Logger.warning(
        "stdlib fitness not recorded for #{inspect(module)}.#{function}: " <>
          "#{Exception.format(kind, reason)}"
      )

      :ok
  end

  @spec outcome(term()) :: String.t()
  defp outcome(result) when is_atom(result), do: Atom.to_string(result)

  defp outcome(result) when is_tuple(result) and tuple_size(result) > 0 do
    case elem(result, 0) do
      head when is_atom(head) -> Atom.to_string(head)
      _other -> "value"
    end
  end

  defp outcome(_other), do: "value"

  @spec tally(EventLog.event(), map()) :: map()
  defp tally(%{payload: payload}, acc) do
    with {:ok, module} <- Map.fetch(payload, "module"),
         {:ok, function} <- Map.fetch(payload, "function") do
      key = {module, function}

      entry =
        Map.get_lazy(acc, key, fn ->
          %{
            module: module,
            function: function,
            calls: 0,
            outcomes: %{},
            ms_total: 0
          }
        end)

      outcome = Map.get(payload, "outcome", "value")
      ms = Map.get(payload, "ms", 0)

      Map.put(acc, key, %{
        entry
        | calls: entry.calls + 1,
          outcomes: Map.update(entry.outcomes, outcome, 1, &(&1 + 1)),
          ms_total: entry.ms_total + if(is_integer(ms), do: ms, else: 0)
      })
    else
      # A row this build cannot read is counted by nobody rather than
      # crashing the fold: the log is append-only and older rows outlive
      # every reader's idea of the payload shape.
      _unreadable -> acc
    end
  end
end
