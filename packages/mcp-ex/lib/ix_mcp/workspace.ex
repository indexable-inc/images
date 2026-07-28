defmodule IxMcp.Workspace do
  @moduledoc """
  Owns the shared evaluation context: the binding and `Macro.Env` every cell
  sees. A cell evaluates in its own process against a snapshot and merges its
  resulting context back on success -- last writer wins per variable, exactly
  the race concurrent Python cells already had on the shared namespace, minus
  the ability to freeze each other.

  One kernel instance is one MCP connection is one session row, but not one
  agent: a Claude Code subagent runs on its parent's connection, so the
  parent and every subagent it fans out share this binding map (#3967). No
  caller identity reaches the kernel (a `tools/call` carries only a per-call
  `claudecode/toolUseId`), so bindings cannot be partitioned by agent. What
  the kernel can do is say who wrote what: every write records its cell's
  job, intent and value type, and a write over another cell's variable is
  reported to both sides. The clobbering cell is told what it replaced; a
  later cell that mentions a variable whose type another cell changed is
  told before it uses it, which is where the silent corruption used to land.

  Every merge is checkpointed into `IxMcp.Checkpoint` (an ETS table owned by a
  different process), so killing or restarting this server -- the analog of a
  kernel restart -- loses nothing: `init/1` restores the last checkpoint.
  """

  use GenServer

  alias IxMcp.Evaluator

  # `Kernel` would shadow Elixir's; cells reach trace/restart as `Ix`.
  @prelude "alias IxMcp.Jobs; alias IxMcp.Api; alias IxMcp.Fleet; " <>
             "alias IxMcp.Read; alias IxMcp.Edit; alias IxMcp.PrWatch; alias IxMcp.Tui; " <>
             "alias IxMcp.TuiLocal; alias IxMcp.Gmail; alias IxMcp.Imsg; alias IxMcp.Contacts; " <>
             "alias IxMcp.Dashboard; " <>
             "alias IxMcp.Kernel, as: Ix; alias IxMcp.Agents; alias IxMcp.Memory; " <>
             "alias IxMcp.Ask; alias IxMcp.Cmd; alias IxMcp.Issues; alias IxMcp.Sessions; " <>
             "alias IxMcp.Serve; " <>
             "alias IxMcp.Requests"

  @typedoc "Who is writing: the cell's job, its intent, and its session row."
  @type writer :: %{
          job: String.t() | nil,
          intent: String.t() | nil,
          session_id: integer() | nil,
          session: String.t() | nil
        }

  @typedoc "What a recorded write knows about itself."
  @type owner :: %{
          job: String.t() | nil,
          intent: String.t() | nil,
          session_id: integer() | nil,
          session: String.t() | nil,
          at: DateTime.t(),
          tag: atom(),
          shape: String.t()
        }

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @doc "The current {binding, env} snapshot a cell should evaluate against."
  @spec snapshot() :: {Code.binding(), Macro.Env.t()}
  def snapshot do
    GenServer.call(__MODULE__, :snapshot)
  end

  @doc """
  Open a cell: the {binding, env} it evaluates against, plus what it should
  be told before it runs.

  One call, not a snapshot followed by a question, so the warnings describe
  exactly the values the cell is handed. The warnings come before evaluation
  because a cell that raises never merges, and the cell holding a clobbered
  value is usually the cell that raises. Nothing is recorded here; a cell
  that fails claims nothing.
  """
  @spec begin_cell(Evaluator.refs(), writer()) ::
          {Code.binding(), Macro.Env.t(), [String.t()]}
  def begin_cell(refs, writer) do
    GenServer.call(__MODULE__, {:begin_cell, refs, writer})
  end

  @doc """
  Merge a finished cell's resulting context back into the shared state, and
  record it as the owner of the variables it wrote and the modules it
  declared. Returns a diagnostic line for every variable this cell took over
  from another cell.
  """
  @spec merge(Code.binding(), Macro.Env.t(), Evaluator.refs(), writer()) :: [String.t()]
  def merge(binding, env, refs, writer) do
    GenServer.call(__MODULE__, {:merge, binding, env, refs, writer})
  end

  @doc "Names bound right now (for introspection / api surface)."
  @spec names() :: [atom()]
  def names do
    {binding, _env} = snapshot()
    binding |> Keyword.keys() |> Enum.sort()
  end

  @doc """
  Every bound name with the cell that bound it: name, value shape, and the
  job, intent, session and time of the write. This is how a cell finds out
  that the `body` it is holding came from somebody else's work.
  """
  @spec owners() :: [map()]
  def owners do
    GenServer.call(__MODULE__, :owners)
  end

  @doc "Drop all bindings and start from the prelude env again."
  @spec reset() :: :ok
  def reset do
    GenServer.call(__MODULE__, :reset)
  end

  @impl true
  def init(_) do
    state =
      case IxMcp.Checkpoint.fetch() do
        {:ok, binding, env} -> %{binding: binding, env: env}
        :empty -> fresh()
      end

    {:ok, Map.merge(state, IxMcp.Checkpoint.fetch_provenance())}
  end

  @impl true
  def handle_call(:snapshot, _from, state) do
    {:reply, {state.binding, state.env}, state}
  end

  # Driven off the binding, not off the provenance map: a name restored from
  # a checkpoint written before this existed has a value and no owner, and
  # leaving it out would report an empty workspace that is not empty.
  def handle_call(:owners, _from, state) do
    rows =
      state.binding
      |> Enum.sort_by(fn {name, _value} -> name end)
      |> Enum.map(fn {name, value} -> row(name, value, Map.get(state.owners, name)) end)

    {:reply, rows, state}
  end

  def handle_call({:begin_cell, refs, writer}, _from, state) do
    now = DateTime.utc_now()

    warnings =
      Enum.flat_map(Map.get(refs, :vars, []), &contested_warning(state, &1, writer, now)) ++
        Enum.flat_map(Map.get(refs, :modules, []), &module_warning(state, &1, writer, now))

    {:reply, {state.binding, state.env, warnings}, state}
  end

  def handle_call({:merge, binding, env, refs, writer}, _from, state) do
    now = DateTime.utc_now()

    {warnings, owners, contested} =
      Enum.reduce(binding, {[], state.owners, state.contested}, fn {name, value}, acc ->
        record_write(state, name, value, writer, now, acc)
      end)

    modules =
      Enum.reduce(Map.get(refs, :modules, []), state.modules, fn module, modules ->
        Map.put(modules, module, definition(writer, now))
      end)

    # Variables the cell bound or rebound win; variables it never touched
    # (pruned from its returned binding) keep their current values.
    merged = Keyword.merge(state.binding, binding)

    state = %{
      state
      | binding: merged,
        env: env,
        owners: owners,
        contested: contested,
        modules: modules
    }

    IxMcp.Checkpoint.store(state.binding, state.env)
    checkpoint_provenance(state)
    {:reply, Enum.reverse(warnings), state}
  end

  def handle_call(:reset, _from, _state) do
    state = fresh()
    IxMcp.Checkpoint.store(state.binding, state.env)
    checkpoint_provenance(state)
    {:reply, :ok, state}
  end

  # A name the cell merely read comes back from `prune_binding` holding the
  # very term it was given, so an identical value is not a write and must not
  # take ownership away from the cell that produced it.
  defp record_write(_state, name, _value, _writer, _now, acc) when not is_atom(name), do: acc

  defp record_write(state, name, value, writer, now, {warnings, owners, contested}) do
    case Keyword.fetch(state.binding, name) do
      {:ok, previous} when previous === value ->
        {warnings, owners, contested}

      previous ->
        mine = describe(value, writer, now)
        taken_over = takeover(Map.get(state.owners, name), writer)

        {warnings, contested} =
          case {previous, taken_over} do
            {{:ok, was}, %{} = theirs} ->
              rebind(name, was, theirs, value, mine, warnings, contested)

            _first_binding_or_own_variable ->
              {warnings, settle(contested, name, mine)}
          end

        {warnings, Map.put(owners, name, mine), contested}
    end
  end

  defp rebind(name, was, theirs, value, mine, warnings, contested) do
    warning =
      "#{severity(theirs, mine)}: shared binding: `#{name}` was bound #{ago(theirs.at, mine.at)} " <>
        "by #{origin(theirs, mine)} as #{shape(was, theirs)}; this cell rebinds it as " <>
        "#{shape(value, mine)}. #{tail(theirs, mine)}"

    contested =
      cond do
        theirs.tag == mine.tag -> Map.delete(contested, name)
        restores?(contested, name, mine) -> Map.delete(contested, name)
        true -> Map.put(contested, name, %{was: theirs, now: mine})
      end

    {[warning | warnings], contested}
  end

  defp restores?(contested, name, mine) do
    match?({:ok, %{was: %{tag: tag}}} when tag == mine.tag, Map.fetch(contested, name))
  end

  # A write that puts the name back to the type it was contested away from is
  # the repair, not a second incident: clearing here is what stops the cell
  # that fixed `body` from being reported as the cell that broke it.
  defp settle(contested, name, mine) do
    case Map.fetch(contested, name) do
      {:ok, %{was: %{tag: tag}}} when tag == mine.tag -> Map.delete(contested, name)
      {:ok, _still_contested} -> contested
      :error -> contested
    end
  end

  # The read side of a clobber: reported to any later cell whose source
  # mentions the name, until somebody binds it again. A type change is the
  # one that raises (`body` went from a String to a list of lines and the
  # next cell's `<>` blew up on it, #3967), so it is the one worth stopping
  # a cell over; a same-typed takeover is reported to the writer only.
  defp contested_warning(state, name, reader, now) do
    case Map.fetch(state.contested, name) do
      {:ok, %{was: was, now: took}} ->
        [
          "warning: shared binding: `#{name}` changed type under this workspace: " <>
            "#{origin(took, reader)} rebound it #{ago(took.at, now)} from #{was.shape} to " <>
            "#{took.shape}, over the value #{origin(was, reader)} bound #{ago(was.at, now)}. " <>
            "Check it before using it."
        ]

      :error ->
        []
    end
  end

  # Modules are global to the BEAM no matter whose cell defines them, so
  # redefinition stays legal and stays warned about -- the compiler's own
  # "redefining module Page" says nothing about who had it first. Reported
  # before the cell runs, recorded only when it succeeds.
  defp module_warning(state, module, writer, now) do
    case takeover(Map.get(state.modules, module), writer) do
      %{} = theirs ->
        [
          "warning: shared module: #{inspect(module)} was defined #{ago(theirs.at, now)} by " <>
            "#{origin(theirs, writer)}; this cell redefines it for every agent on this " <>
            "kernel, because modules are global to the BEAM."
        ]

      nil ->
        []
    end
  end

  defp definition(writer, now) do
    %{
      job: writer.job,
      intent: writer.intent,
      session_id: writer.session_id,
      session: writer.session,
      at: now,
      tag: :module,
      shape: "a module"
    }
  end

  defp row(name, value, nil) do
    %{
      name: name,
      shape: shape_of(value),
      job: nil,
      intent: nil,
      session_id: nil,
      session: nil,
      at: nil
    }
  end

  defp row(name, _value, owner) do
    %{
      name: name,
      shape: owner.shape,
      job: owner.job,
      intent: owner.intent,
      session_id: owner.session_id,
      session: owner.session,
      at: owner.at
    }
  end

  # A cell rewriting its own variable is not a collision: the same job means
  # the same cell, and a cell is the smallest unit any single agent runs.
  defp takeover(nil, _writer), do: nil
  defp takeover(%{job: nil}, _writer), do: nil
  defp takeover(%{job: job} = owner, %{job: mine}) when job != mine, do: owner
  defp takeover(_owner, _writer), do: nil

  defp severity(theirs, mine) do
    if theirs.tag == mine.tag, do: "note", else: "warning"
  end

  # A same-typed takeover is a one-line note over ordinary sequential work as
  # often as it is a collision, so it stays short; the type change, the one
  # that has a raise waiting downstream, gets the whole explanation.
  defp tail(theirs, mine) do
    if theirs.tag == mine.tag do
      "`Ix.bindings()` names who bound what."
    else
      "One kernel's bindings are shared by every agent on it, a session and " <>
        "its subagents alike; `Ix.bindings()` names who bound what."
    end
  end

  defp origin(owner, reader) do
    said = if owner.intent, do: " (intent: #{inspect(owner.intent)})", else: ""

    session =
      if owner.session_id && owner.session_id != Map.get(reader, :session_id) do
        ", session #{owner.session_id}#{if owner.session, do: " (#{owner.session})", else: ""}"
      else
        ""
      end

    "job #{owner.job}#{said}#{session}"
  end

  defp describe(value, writer, now) do
    %{
      job: writer.job,
      intent: writer.intent,
      session_id: writer.session_id,
      session: writer.session,
      at: now,
      tag: tag(value),
      shape: shape_of(value)
    }
  end

  # The recorded shape is what the value looked like when it was written;
  # `shape/2` prefers it so a warning never re-walks a stale megabyte.
  defp shape(_value, %{shape: shape}) when is_binary(shape), do: shape
  defp shape(value, _owner), do: shape_of(value)

  defp tag(value) when is_binary(value), do: :binary
  defp tag(value) when is_list(value), do: :list
  defp tag(%module{}), do: module
  defp tag(value) when is_map(value), do: :map
  defp tag(value) when is_tuple(value), do: :tuple
  defp tag(value) when is_integer(value), do: :integer
  defp tag(value) when is_float(value), do: :float
  defp tag(value) when is_atom(value), do: :atom
  defp tag(value) when is_function(value), do: :function
  defp tag(value) when is_pid(value), do: :pid
  defp tag(_value), do: :term

  # Nothing about describing a value may take the workspace down: this runs
  # inside the GenServer every cell blocks on, and an improper list bound as
  # iodata (`["a" | "b"]`) is enough to make `length/1` raise (#3967).
  defp shape_of(value) do
    describe_shape(value)
  rescue
    _any -> "a term"
  end

  defp describe_shape(value) when is_binary(value), do: counted(byte_size(value), "byte binary")
  defp describe_shape(value) when is_list(value), do: counted(length(value), "element list")
  defp describe_shape(%module{}), do: "a #{inspect(module)} struct"
  defp describe_shape(value) when is_map(value), do: counted(map_size(value), "key map")
  defp describe_shape(value) when is_tuple(value), do: counted(tuple_size(value), "element tuple")
  defp describe_shape(value) when is_integer(value), do: "the integer #{value}"
  defp describe_shape(value) when is_float(value), do: "a float"
  defp describe_shape(value) when is_atom(value), do: "the atom #{inspect(value)}"
  defp describe_shape(value) when is_function(value), do: "a function"
  defp describe_shape(_value), do: "a term"

  # "an 18-element list", not "a 18-element list": eight and eighteen are the
  # only counts whose spoken form opens on a vowel.
  defp counted(n, noun) do
    digits = Integer.to_string(n)
    vowel = String.starts_with?(digits, "8") or n in [11, 18]
    "#{if vowel, do: "an", else: "a"} #{n}-#{noun}"
  end

  defp ago(then, now) do
    case DateTime.diff(now, then) do
      s when s < 2 -> "just now"
      s when s < 90 -> "#{s}s ago"
      s when s < 5400 -> "#{div(s, 60)}m ago"
      s -> "#{div(s, 3600)}h ago"
    end
  end

  defp checkpoint_provenance(state) do
    IxMcp.Checkpoint.store_provenance(Map.take(state, [:owners, :contested, :modules]))
  end

  defp fresh do
    env = Code.env_for_eval(file: "cell")
    # Evaluate the prelude so its aliases live in the env every cell sees;
    # cells can then write `Jobs.tail("ab12", 20)` with no setup.
    {_value, binding, env} = Code.eval_quoted_with_env(quoted_prelude(), [], env)
    %{binding: binding, env: env, owners: %{}, contested: %{}, modules: %{}}
  end

  defp quoted_prelude do
    Code.string_to_quoted!(@prelude, file: "prelude")
  end
end
