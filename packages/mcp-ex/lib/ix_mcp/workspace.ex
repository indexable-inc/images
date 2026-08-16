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

  ## Named workspaces (#3967, follow-up)

  Warning about clobbers is mitigation; isolation is the fix. This module is
  multi-instance: `"main"` is the shared default (a static supervised child,
  registered under the module name exactly as before), and any other name is
  its own supervised GenServer with its own binding map, env, provenance and
  checkpoint row, started on first use under `IxMcp.Workspaces.Supervisor`
  and found through `IxMcp.Workspaces.Registry`. The exec tool's `workspace`
  parameter is how a cell targets one; inside a cell every function here
  defaults to the workspace the cell is running in (planted in the eval
  process's dictionary), so `Ix.bindings()` from an isolated cell describes
  that cell's own REPL. Separate evaluator state, same BEAM: modules, ETS,
  processes and the OS stay global, which is why module takeover warnings
  stay on even across workspaces.
  """

  use GenServer

  alias IxMcp.Evaluator

  # `Kernel` would shadow Elixir's; cells reach trace/restart as `Ix`.
  @core_prelude "alias IxMcp.Jobs; alias IxMcp.Api; alias IxMcp.Fleet; " <>
                  "alias IxMcp.Read; alias IxMcp.Edit; alias IxMcp.PrWatch; alias IxMcp.Tui; " <>
                  "alias IxMcp.TuiLocal; alias IxMcp.Gmail; alias IxMcp.Imsg; alias IxMcp.Contacts; " <>
                  "alias IxMcp.Dashboard; " <>
                  "alias IxMcp.Kernel, as: Ix; alias IxMcp.Agents; alias IxMcp.Memory; " <>
                  "alias IxMcp.Memories; " <>
                  "alias IxMcp.Ask; alias IxMcp.Cmd; alias IxMcp.Issues; alias IxMcp.Sessions; " <>
                  "alias IxMcp.Requests; alias IxMcp.Web; alias IxMcp.Image; " <>
                  "alias IxMcp.Ctx; alias IxMcp.LM; alias IxMcp.EventLog; alias IxMcp.RLM; " <>
                  "alias IxMcp.Workspace"

  # The short names @core_prelude binds, derived FROM it so the two cannot drift.
  # IxMcp.Stdlib asks for this at runtime to refuse a resident that would shadow
  # one of them in every cell.
  @core_names @core_prelude
              |> String.split(";")
              |> Enum.map(&String.trim/1)
              |> Enum.flat_map(fn entry ->
                case Regex.run(
                       ~r/^alias\s+([A-Za-z0-9_.]+)(?:,\s*as:\s*([A-Za-z0-9_]+))?$/,
                       entry
                     ) do
                  [_all, module] -> [module |> String.split(".") |> List.last()]
                  [_all, _module, as] -> [as]
                  nil -> []
                end
              end)

  @doc false
  @spec core_aliases() :: [String.t()]
  def core_aliases, do: @core_names

  @typedoc "Who is writing: the cell's job, when it began, its intent, and its session row."
  @type writer :: %{
          job: String.t() | nil,
          intent: String.t() | nil,
          session_id: integer() | nil,
          session: String.t() | nil,
          started_at: DateTime.t() | nil
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

  @main "main"
  # Letters, digits, dot, dash, underscore; length-bounded. Names are Registry
  # keys (strings), so nothing here can leak atoms; the bound just keeps ids
  # readable in warnings and Jobs.history.
  @name_format ~r/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/

  @spec start_link(keyword() | String.t()) :: GenServer.on_start()
  def start_link(opts) when is_list(opts), do: start_link(Keyword.get(opts, :workspace, @main))

  def start_link(name) when is_binary(name) do
    GenServer.start_link(__MODULE__, name, name: registration(name))
  end

  # "main" keeps its historical module-name registration (a static child of
  # IxMcp.Supervisor), so Ix.restart/0, tests, and everything that ever said
  # Process.whereis(IxMcp.Workspace) still means the default workspace.
  defp registration(@main), do: __MODULE__
  defp registration(name), do: {:via, Registry, {IxMcp.Workspaces.Registry, name}}

  @doc "The default workspace's name."
  @spec main() :: String.t()
  def main, do: @main

  @doc """
  The workspace the calling process's cell targets: the name exec was called
  with, planted in the eval process's dictionary at spawn. Outside a cell
  (or in a process the cell spawned) it is `"main"`.
  """
  @spec current() :: String.t()
  def current, do: Process.get(:ix_workspace, @main)

  @doc "Whether `name` is a legal workspace name."
  @spec valid_name?(term()) :: boolean()
  def valid_name?(name), do: is_binary(name) and Regex.match?(@name_format, name)

  defp validate!(name) do
    unless valid_name?(name) do
      raise ArgumentError,
            "workspace name must be a string of letters, digits, '.', '-' or '_' " <>
              "(max 64 chars), got: #{inspect(name)}"
    end

    name
  end

  @doc """
  Start (or find) the named workspace and return its pid. `"main"` is always
  running; any other name starts on first use under the dynamic supervisor,
  restoring its own checkpoint row if it crashed or the kernel restarted.
  """
  @spec ensure(String.t()) :: pid()
  def ensure(@main), do: Process.whereis(__MODULE__) || raise("main workspace is not running")

  def ensure(name) do
    validate!(name)

    case Registry.lookup(IxMcp.Workspaces.Registry, name) do
      # The registry unregisters dead pids asynchronously (#3538), so a
      # lookup right after a kill or an Ix.restart can hand back a corpse;
      # only an alive pid answers calls.
      [{pid, _value}] -> if Process.alive?(pid), do: pid, else: start_named(name, 100)
      [] -> start_named(name, 100)
    end
  end

  defp start_named(name, retries) do
    case DynamicSupervisor.start_child(IxMcp.Workspaces.Supervisor, {__MODULE__, name}) do
      {:ok, pid} ->
        pid

      {:error, {:already_started, pid}} ->
        cond do
          Process.alive?(pid) ->
            pid

          retries > 0 ->
            # The dead pid is still registered for the instant between its
            # exit and the registry's cleanup; wait that window out instead
            # of handing a caller a pid whose call will exit :noproc.
            Process.sleep(10)
            start_named(name, retries - 1)

          true ->
            raise "workspace #{inspect(name)} is wedged: a dead process holds its registration"
        end
    end
  end

  @doc """
  Create (or open) a named workspace: an isolated REPL with its own bindings,
  env, and checkpoint, on the same BEAM. Idempotent. A subagent should call
  this once -- or simply pass `workspace:` on exec, which creates it too --
  and then target that name on every exec call.
  """
  @spec new(String.t()) :: %{workspace: String.t(), created: boolean(), bindings: [atom()]}
  def new(name) do
    validate!(name)
    existed = name == @main or Registry.lookup(IxMcp.Workspaces.Registry, name) != []
    pid = ensure(name)
    {binding, _env} = GenServer.call(pid, :snapshot)

    %{
      workspace: name,
      created: not existed,
      bindings: binding |> Keyword.keys() |> Enum.sort()
    }
  end

  @doc "Every live workspace with its binding count. `\"main\"` is always first."
  @spec list() :: [%{workspace: String.t(), bindings: non_neg_integer(), default: boolean()}]
  def list do
    named =
      IxMcp.Workspaces.Registry
      |> Registry.select([{{:"$1", :_, :_}, [], [:"$1"]}])
      |> Enum.sort()

    for name <- [@main | named] do
      %{workspace: name, bindings: length(names(name)), default: name == @main}
    end
  end

  @doc "Names of the live named workspaces (not `\"main\"`), for restart bookkeeping."
  @spec named() :: [String.t()]
  def named do
    IxMcp.Workspaces.Registry
    |> Registry.select([{{:"$1", :_, :_}, [], [:"$1"]}])
    |> Enum.sort()
  end

  @doc """
  Stop a named workspace and delete its checkpoint. `"main"` cannot be
  dropped -- reset it instead. Dropping a workspace a job is currently
  evaluating against loses that cell's merge (it has nothing to merge into).
  """
  @spec drop(String.t()) :: :ok
  def drop(@main) do
    raise ArgumentError,
          "the main workspace cannot be dropped; Workspace.reset(\"main\") clears it"
  end

  def drop(name) do
    validate!(name)

    case Registry.lookup(IxMcp.Workspaces.Registry, name) do
      [{pid, _value}] -> :ok = DynamicSupervisor.terminate_child(IxMcp.Workspaces.Supervisor, pid)
      [] -> :ok
    end

    IxMcp.Checkpoint.clear(name)
    :ok
  end

  @doc "The current {binding, env} snapshot a cell should evaluate against."
  @spec snapshot(String.t()) :: {Code.binding(), Macro.Env.t()}
  def snapshot(workspace \\ nil) do
    GenServer.call(ensure(workspace || current()), :snapshot)
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
  @spec begin_cell(Evaluator.refs(), writer(), String.t() | nil) ::
          {Code.binding(), Macro.Env.t(), [String.t()]}
  def begin_cell(refs, writer, workspace \\ nil) do
    GenServer.call(ensure(workspace || current()), {:begin_cell, refs, writer})
  end

  @doc """
  Merge a finished cell's resulting context back into the shared state, and
  record it as the owner of the variables it wrote and the modules it
  declared. Returns a diagnostic line for every variable this cell took over
  from another cell.
  """
  @spec merge(Code.binding(), Macro.Env.t(), Evaluator.refs(), writer(), String.t() | nil) ::
          [String.t()]
  def merge(binding, env, refs, writer, workspace \\ nil) do
    GenServer.call(ensure(workspace || current()), {:merge, binding, env, refs, writer})
  end

  @doc "Names bound right now (for introspection / api surface)."
  @spec names(String.t() | nil) :: [atom()]
  def names(workspace \\ nil) do
    {binding, _env} = snapshot(workspace)
    binding |> Keyword.keys() |> Enum.sort()
  end

  @doc """
  Every bound name with the cell that bound it: name, value shape, and the
  job, intent, session and time of the write. This is how a cell finds out
  that the `body` it is holding came from somebody else's work.
  """
  @spec owners(String.t() | nil) :: [map()]
  def owners(workspace \\ nil) do
    GenServer.call(ensure(workspace || current()), :owners)
  end

  @doc "Drop all bindings and start from the prelude env again."
  @spec reset(String.t() | nil) :: :ok
  def reset(workspace \\ nil) do
    GenServer.call(ensure(workspace || current()), :reset)
  end

  @impl true
  def init(name) do
    state =
      case IxMcp.Checkpoint.fetch(name) do
        {:ok, binding, env} -> %{binding: binding, env: env}
        :empty -> fresh()
      end

    state =
      state
      |> Map.merge(IxMcp.Checkpoint.fetch_provenance(name))
      |> Map.put(:name, name)

    {:ok, state}
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

    IxMcp.Checkpoint.store(state.name, state.binding, state.env)
    checkpoint_provenance(state)
    {:reply, Enum.reverse(warnings), state}
  end

  def handle_call(:reset, _from, %{name: name}) do
    state = Map.put(fresh(), :name, name)
    IxMcp.Checkpoint.store(name, state.binding, state.env)
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
    contested =
      cond do
        theirs.tag == mine.tag -> Map.delete(contested, name)
        restores?(contested, name, mine) -> Map.delete(contested, name)
        true -> Map.put(contested, name, %{was: theirs, now: mine})
      end

    {report(name, was, theirs, value, mine, warnings), contested}
  end

  # A same-typed rebind of a name whose previous cell had already finished is
  # one agent reusing a scratch name across its own turn, which is most of
  # what a kernel sees: `out` rebound by every cell of a session, reported on
  # every one of them and telling either cell nothing it did not know. Left
  # in, it buries the type change (#3967) -- the one with a raise waiting
  # downstream -- so that one is never suppressed, overlap or not: there the
  # cells are sequential by construction (A binds, B clobbers, A reads).
  defp report(name, was, theirs, value, mine, warnings) do
    if theirs.tag == mine.tag and not overlapped?(theirs, mine) do
      warnings
    else
      [
        "#{severity(theirs, mine)}: shared binding: `#{name}` was bound #{ago(theirs.at, mine.at)} " <>
          "by #{origin(theirs, mine)} as #{shape(was, theirs)}; this cell rebinds it as " <>
          "#{shape(value, mine)}. #{tail(theirs, mine)}"
        | warnings
      ]
    end
  end

  # Were both cells alive at once? A write lands when its cell finishes, so a
  # cell that started before that instant ran alongside the one it took the
  # name from -- the only way two agents can surprise each other, and the
  # thing one session's sequential cells never do. An unknown start time
  # counts as overlapping: the guard reports rather than assumes.
  defp overlapped?(theirs, mine) do
    case {theirs.at, mine.started_at} do
      {%DateTime{} = landed, %DateTime{} = began} -> DateTime.compare(began, landed) == :lt
      _unknown -> true
    end
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
      "One workspace's bindings are shared by every agent targeting it; " <>
        "`Ix.bindings()` names who bound what, and concurrent agents should " <>
        "isolate by passing their own `workspace:` name on exec."
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
      started_at: Map.get(writer, :started_at),
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
    IxMcp.Checkpoint.store_provenance(
      state.name,
      Map.take(state, [:owners, :contested, :modules])
    )
  end

  defp fresh do
    env = Code.env_for_eval(file: "cell")
    # Evaluate the prelude so its aliases live in the env every cell sees;
    # cells can then write `Jobs.tail("ab12", 20)` with no setup.
    {_value, binding, env} = Code.eval_quoted_with_env(quoted_prelude(), [], env)
    %{binding: binding, env: env, owners: %{}, contested: %{}, modules: %{}}
  end

  # The grown stdlib is appended rather than listed: a module under
  # `lib/ix_mcp/stdlib/` is aliased into every cell by existing, with no line
  # to add here. Read when a workspace is created, so a resident that landed
  # after this kernel booted needs a restart -- `IxMcp.Stdlib` says why hot
  # reload is deliberate follow-up.
  defp prelude do
    case IxMcp.Stdlib.prelude() do
      "" -> @core_prelude
      residents -> @core_prelude <> "; " <> residents
    end
  end

  defp quoted_prelude do
    Code.string_to_quoted!(prelude(), file: "prelude")
  end
end
