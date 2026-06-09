defmodule SymphonyElixir.Runtime do
  @moduledoc """
  One GenServer per active IR run. It schedules ready `IR.Node`s as
  monitored BEAM tasks, commits each result into the durable `RunGraph`
  through `IR.Store`, and resolves the run when every node is terminal.

  It runs under `Runtime.Supervisor` in the live tree, resumes persisted
  runs at boot, and backs the `/api/v1/ir/runs` operator surface. The
  engine is injected (`Runtime.RoomEngineClient` in production), so tests
  drive it against a fake `EngineClient` with no room-server.

  ## Crash recovery (issue #90)

  Two failure modes are handled, with the same conservative bias:

  1. Executor crash. Every executor task is monitored. A `:DOWN` that
     arrives without a prior `{:node_done, id, result}` means the task
     died mid-attempt. The runtime cannot assume the attempt had no side
     effect (an agent turn may have pushed a commit), so it marks the
     attempt `:stranded` and routes the node by the non-idempotent retry
     policy in `Runtime.Recovery`: auto-retry only when the node opted in
     and showed no observed side effect, otherwise leave it `:stranded`
     for human review.

  2. BEAM restart. A node persisted `:running` is reconciled by
     `Runtime.Recovery.reconcile/2` at boot (reattach via
     `EngineClient.status/1`, else strand by policy), then the run resumes
     from the recomputed ready set. The runtime calls `reconcile/1` in its
     init continuation when started from a reloaded graph.

  ## Deadlock guard

  If a scheduling pass finds no ready nodes, no live tasks, and a
  non-terminal run, the run cannot make progress and would otherwise hang
  forever. The guard fails the run with `:deadlock` and a clear reason
  rather than leaving it stuck. This is the safety net behind the
  upstream-failed propagation: a graph whose only remaining nodes are
  blocked resolves instead of stalling.

  ## Operator hooks

  `cancel/1`, `retry_node/2`, and `rerun/1` are the operator surface, as
  function clauses the HTTP layer can call later. They manipulate the
  graph and reschedule; they do not assume any particular transport.
  """

  use GenServer, restart: :transient
  require Logger

  alias SymphonyElixir.GithubApp
  alias SymphonyElixir.IR.{Attempt, Graph, Materializer, Node, RunGraph}
  alias SymphonyElixir.Runtime.{Events, ExecRunner, Placement, Recovery, SubrunRunner}

  @registry SymphonyElixir.Runtime.Registry

  @typedoc """
  Runtime process state. `graph` is the live `RunGraph`; `tasks` maps a
  monitor ref to the node id it is executing, so a `:DOWN` resolves to the
  right node; `node_refs` is the reverse map for cancellation. `opts`
  carries the injected `EngineClient` and `IR.Store` dir so tests isolate.
  `subrun_depth` is how many subrun levels sit above this run (0 for a
  top-level run); `subrun_ancestors` is the workflow-name chain open above
  it. Both are threaded into a `:subrun` node's executor so a child run
  guards against a cycle and an over-deep chain.
  """
  @type state :: %{
          graph: RunGraph.t(),
          tasks: %{reference() => String.t()},
          node_refs: %{String.t() => reference()},
          engine: module(),
          store_opts: keyword(),
          subrun_depth: non_neg_integer(),
          subrun_ancestors: [String.t()],
          placement: module(),
          placement_acquired?: boolean()
        }

  @spec start_link(RunGraph.t(), keyword()) :: GenServer.on_start()
  def start_link(%RunGraph{} = graph, opts \\ []) do
    GenServer.start_link(__MODULE__, {graph, opts}, name: via(graph.run_id))
  end

  @spec child_spec({RunGraph.t(), keyword()}) :: Supervisor.child_spec()
  def child_spec({%RunGraph{} = graph, opts}) do
    %{
      id: {__MODULE__, graph.run_id},
      start: {__MODULE__, :start_link, [graph, opts]},
      restart: :transient,
      type: :worker
    }
  end

  defp via(run_id), do: {:via, Registry, {@registry, run_id}}

  @doc "Read the current graph snapshot. Used by tests and the operator surface."
  @spec graph(pid() | String.t()) :: RunGraph.t()
  def graph(pid) when is_pid(pid), do: GenServer.call(pid, :graph)
  def graph(run_id) when is_binary(run_id), do: GenServer.call(via(run_id), :graph)

  @typedoc "Who requested an operator action, recorded in the audit log."
  @type actor :: term()

  @doc """
  Cancel the run. Running nodes' tasks are killed, every non-terminal node
  is marked `:cancelled`, and the run is stopped. `actor` is recorded in
  the audit log (defaults to `:operator`). An operator hook.
  """
  @spec cancel(pid() | String.t(), actor()) :: :ok
  def cancel(target, actor \\ :operator)
  def cancel(pid, actor) when is_pid(pid), do: GenServer.call(pid, {:cancel, actor})
  def cancel(run_id, actor) when is_binary(run_id), do: GenServer.call(via(run_id), {:cancel, actor})

  @doc """
  Retry one node: reset it to `:pending` and reschedule. This is the
  explicit operator override of the conservative auto-retry default, so it
  does not consult `__retry__`; the operator is asserting the retry is
  safe. `actor` is recorded in the audit log. An operator hook.
  """
  @spec retry_node(pid() | String.t(), String.t(), actor()) :: :ok
  def retry_node(target, node_id, actor \\ :operator)
  def retry_node(pid, node_id, actor) when is_pid(pid), do: GenServer.call(pid, {:retry_node, node_id, actor})

  def retry_node(run_id, node_id, actor) when is_binary(run_id),
    do: GenServer.call(via(run_id), {:retry_node, node_id, actor})

  @doc """
  Re-run the whole graph from scratch: reset every node to `:pending` and
  reschedule. The AST and expansion log are preserved. `actor` is recorded
  in the audit log. An operator hook.
  """
  @spec rerun(pid() | String.t(), actor()) :: :ok
  def rerun(target, actor \\ :operator)
  def rerun(pid, actor) when is_pid(pid), do: GenServer.call(pid, {:rerun, actor})
  def rerun(run_id, actor) when is_binary(run_id), do: GenServer.call(via(run_id), {:rerun, actor})

  @doc """
  Clear failed nodes: reset every `:failed`, `:upstream_failed`, and
  `:stranded` node to `:pending` and reschedule, leaving succeeded nodes
  intact. This is the surgical recovery operators reach for after fixing
  the cause of a failure, rather than re-running the whole graph. `actor`
  is recorded in the audit log. An operator hook.
  """
  @spec clear_failed(pid() | String.t(), actor()) :: :ok
  def clear_failed(target, actor \\ :operator)
  def clear_failed(pid, actor) when is_pid(pid), do: GenServer.call(pid, {:clear_failed, actor})
  def clear_failed(run_id, actor) when is_binary(run_id), do: GenServer.call(via(run_id), {:clear_failed, actor})

  @impl true
  def init({%RunGraph{} = graph, opts}) do
    state = %{
      graph: graph,
      tasks: %{},
      node_refs: %{},
      engine: Keyword.fetch!(opts, :engine),
      store_opts: Keyword.get(opts, :store_opts, []),
      subrun_depth: Keyword.get(opts, :subrun_depth, 0),
      subrun_ancestors: Keyword.get(opts, :subrun_ancestors, []),
      placement: Keyword.get(opts, :placement, Placement),
      placement_acquired?: false
    }

    # A graph reloaded from disk may carry orphaned :running nodes. The
    # `recover: true` option asks the runtime to reconcile them before its
    # first scheduling pass, the BEAM-restart half of #90.
    state =
      if Keyword.get(opts, :recover, false) do
        recovered = Recovery.reconcile(graph, fn thread_id -> state.engine.status(thread_id) end)
        %{state | graph: recovered}
      else
        state
      end

    # Reconcile may have harvested outputs that resolve a gate, so re-expand
    # before the first scheduling pass. Idempotent and a no-op for a graph
    # with no AST, so it is safe on both the fresh-start and restart paths.
    # An invalid dynamically-emitted envelope fails the run rather than
    # crashing init, so a bad child surfaces as a failed run, not a
    # supervisor restart loop.
    case Materializer.expand_dynamic(state.graph) do
      {:ok, expanded, _new_ids} ->
        state = %{state | graph: expanded}
        # Persist before the first scheduling pass so a producer that
        # navigates to /ir/:run_id the moment start_run returns finds the
        # run on disk, even while a slow placement acquire is still in
        # flight. The run shows :running with :pending nodes; the broadcast
        # also lands so the index row appears without a navigation.
        persist(expanded, state)
        {:ok, state, {:continue, :advance}}

      {:error, reason} ->
        {:ok, %{state | graph: fail_run(state.graph, reason, state.store_opts)}, {:continue, :advance}}
    end
  end

  @impl true
  def handle_continue(:advance, state), do: advance(state)

  @impl true
  def handle_call(:graph, _from, state), do: {:reply, state.graph, state}

  @impl true
  def handle_call({:cancel, actor}, _from, state) do
    Enum.each(Map.keys(state.tasks), &Process.demonitor(&1, [:flush]))

    cancelled =
      Enum.reduce(state.graph.nodes, state.graph, fn {id, node}, acc ->
        if Node.terminal?(node), do: acc, else: transition(acc, id, :cancelled)
      end)

    finished =
      %{cancelled | status: :cancelled}
      |> RunGraph.append_audit(:cancel, nil, actor, %{})

    persist(finished, state)
    release_placement(state)
    {:stop, :normal, :ok, %{state | graph: finished, tasks: %{}, node_refs: %{}}}
  end

  @impl true
  def handle_call({:retry_node, node_id, actor}, _from, state) do
    graph =
      state.graph
      |> Graph.reset_node(node_id)
      |> RunGraph.append_audit(:retry_node, node_id, actor, %{})

    persist(graph, state)
    advance_reply(%{state | graph: graph})
  end

  @impl true
  def handle_call({:rerun, actor}, _from, state) do
    graph =
      Enum.reduce(Map.keys(state.graph.nodes), state.graph, fn id, acc -> Graph.reset_node(acc, id) end)

    graph =
      %{graph | status: :running}
      |> RunGraph.append_audit(:rerun, nil, actor, %{})

    persist(graph, state)
    advance_reply(%{state | graph: graph})
  end

  @impl true
  def handle_call({:clear_failed, actor}, _from, state) do
    cleared_ids =
      for {id, node} <- state.graph.nodes, node.state in [:failed, :upstream_failed, :stranded], do: id

    graph =
      cleared_ids
      |> Enum.reduce(state.graph, fn id, acc -> Graph.reset_node(acc, id) end)
      |> Map.put(:status, :running)
      |> RunGraph.append_audit(:clear_failed, nil, actor, %{cleared: cleared_ids})

    persist(graph, state)
    advance_reply(%{state | graph: graph})
  end

  @impl true
  def handle_info({:node_done, node_id, result, thread_id}, state) do
    state = drop_task_for(state, node_id)
    graph = record_finished_attempt(state.graph, node_id, result, thread_id)
    graph = Graph.apply_output(graph, node_id, result)
    # A succeeded node may unlock a gate or fan-out: its output is now in
    # known_outputs, so re-expand the AST to emit any newly-justified
    # children before the next scheduling pass. A failure cannot resolve a
    # gate (the dep did not produce an output), so re-expansion only runs
    # on success.
    graph = expand_on_success(graph, result, state.store_opts)
    persist(graph, state)
    advance(%{state | graph: graph})
  end

  @impl true
  def handle_info({:DOWN, ref, :process, _pid, reason}, state) do
    case Map.fetch(state.tasks, ref) do
      {:ok, node_id} ->
        # A :DOWN with the node still :running means the task died without
        # reporting a result. (A clean finish removes the monitor before
        # this arrives via `drop_task_for/2`.) Strand the attempt and route
        # by the non-idempotent retry policy.
        Logger.warning("Runtime #{state.graph.run_id} node #{node_id} task down: #{inspect(reason)}")
        state = %{state | tasks: Map.delete(state.tasks, ref), node_refs: Map.delete(state.node_refs, node_id)}
        graph = strand_node(state.graph, node_id)
        persist(graph, state)
        advance(%{state | graph: graph})

      :error ->
        # A monitor we already flushed, or an unrelated process. Ignore.
        {:noreply, state}
    end
  end

  @impl true
  def handle_info(message, state) do
    # Defense in depth: a stray message (for example a late port line
    # leaked by a timed-out Command.run child) must not crash the run.
    # A FunctionClauseError here kills the GenServer; the supervisor's
    # transient restart then replays the run from its persisted graph
    # and can double-submit in-flight turns. Log and drop instead.
    Logger.warning("Runtime #{state.graph.run_id} ignoring unexpected message: #{inspect(message)}")
    {:noreply, state}
  end

  # Re-expand the AST after a successful node so a resolved gate or
  # fan-out emits its children. Only runs on `{:ok, _}`: a failure does not
  # produce an output a gate can read. The new children land `:pending`
  # and the next scheduling pass picks up the ready ones; resolved
  # placeholders are retired to `:skipped` by the materializer.
  defp expand_on_success(%RunGraph{} = graph, {:ok, _output}, store_opts) do
    case Materializer.expand_dynamic(graph) do
      {:ok, expanded, _new_ids} -> expanded
      # A dynamically-emitted child with an invalid envelope fails the run.
      {:error, reason} -> fail_run(graph, reason, store_opts)
    end
  end

  defp expand_on_success(%RunGraph{} = graph, _result, _store_opts), do: graph

  # Mark the run failed for a load-time error (an invalid envelope on a
  # dynamically-emitted node). Every non-terminal node becomes
  # :upstream_failed so the run resolves instead of stalling on a child
  # that can never be scheduled.
  defp fail_run(%RunGraph{} = graph, reason, store_opts) do
    Logger.error("Runtime #{graph.run_id} expansion failed: #{inspect(reason)}")

    failed =
      Enum.reduce(graph.nodes, graph, fn {id, node}, acc ->
        if Node.terminal?(node), do: acc, else: transition(acc, id, :upstream_failed)
      end)

    failed = %{failed | status: :failed}
    persist(failed, %{store_opts: store_opts})
    failed
  end

  # --- scheduling -----------------------------------------------------

  defp advance(state) do
    case advance_step(state) do
      {:noreply, _next} = reply -> reply
      {:stop, next} -> {:stop, :normal, next}
    end
  end

  defp advance_reply(state) do
    case advance_step(state) do
      {:noreply, next} -> {:reply, :ok, next}
      {:stop, next} -> {:stop, :normal, :ok, next}
    end
  end

  defp advance_step(state) do
    cond do
      Graph.all_terminal?(state.graph) ->
        finished = finish(state)
        # A failed run stays alive and idle so the operator surface
        # (clear_failed, retry_node, rerun) can reach a live process. A
        # succeeded or cancelled run has nothing left to operate on, so it
        # stops and frees the process. The supervisor can still resume a
        # failed run from the store after a restart.
        if finished.graph.status == :failed do
          {:noreply, finished}
        else
          {:stop, finished}
        end

      true ->
        ready = Graph.ready_nodes(state.graph)
        schedule(state, ready)
    end
  end

  defp schedule(state, []) do
    cond do
      map_size(state.tasks) > 0 ->
        # Work is in flight; wait for a :node_done or :DOWN to wake us.
        {:noreply, state}

      no_nonterminal_nodes?(state.graph) ->
        # No ready nodes, no live tasks, and nothing non-terminal left. This
        # is a completed run, not a deadlock: it covers a gate that resolved
        # every body off (`when` falsy, `every n` that did not fire this
        # tick) so the graph materialized to zero schedulable work. Resolve
        # it through the normal finish path rather than tripping the guard.
        # `Graph.all_terminal?/1` treats an empty node map as not-terminal so
        # a run is never declared done before its first materialization; here
        # the run is already :running, so an empty or all-terminal node set
        # is a real no-op completion.
        finished = finish(state)
        if finished.graph.status == :failed, do: {:noreply, finished}, else: {:stop, finished}

      true ->
        # No ready nodes, no live tasks, but a non-terminal node remains: the
        # remaining nodes are permanently blocked. Fail rather than hang.
        # This is the #90 deadlock guard.
        {:stop, deadlock(state)}
    end
  end

  defp schedule(state, ready) do
    next = Enum.reduce(ready, state, &start_node(&2, &1))
    {:noreply, next}
  end

  # Whether every node in the graph is terminal. An empty node map (a
  # fully-gated-off materialization) is vacuously all-terminal: there is no
  # work left, so the run completes as a no-op rather than deadlocking.
  defp no_nonterminal_nodes?(%RunGraph{nodes: nodes}) do
    Enum.all?(Map.values(nodes), &Node.terminal?/1)
  end

  defp start_node(state, %Node{} = node) do
    attempt_n = length(node.attempts) + 1
    # Mark + persist the attempt as running before provisioning so the node
    # is observable during a slow placement acquire. The turn task is only
    # spawned after placement resolves (it reads the per-run base_url).
    graph = mark_running(state.graph, node, attempt_n)
    persist(graph, state)
    state = ensure_placement(%{state | graph: graph}, node)
    graph = state.graph
    engine = state.engine
    runtime = self()
    run_opts = run_opts(state, node, attempt_n)

    # Fire-and-forget the task, then monitor the spawned pid. The task
    # reports its result through an explicit `{:node_done, ...}` message;
    # the monitor's `:DOWN` is the crash signal. Owning the monitor ref
    # ourselves (rather than `async_nolink`) keeps the `:DOWN` the only
    # task-lifecycle message the GenServer ever sees, so a clean exit and a
    # crash are told apart purely by whether `:node_done` arrived first.
    {:ok, pid} =
      Task.Supervisor.start_child(SymphonyElixir.TaskSupervisor, fn ->
        case run_attempt(node, engine, run_opts) do
          {:ok, output, thread_id} -> send(runtime, {:node_done, node.id, {:ok, output}, thread_id})
          {:error, reason, thread_id} -> send(runtime, {:node_done, node.id, {:error, reason}, thread_id})
        end
      end)

    ref = Process.monitor(pid)

    %{
      state
      | graph: graph,
        tasks: Map.put(state.tasks, ref, node.id),
        node_refs: Map.put(state.node_refs, node.id, ref)
    }
  end

  # The per-attempt context handed to an executor. Every node gets the run
  # id and attempt number; a `:subrun` node additionally gets the engine,
  # store dir, its place in the subrun depth/ancestor chain, and its inputs
  # resolved against upstream outputs, since a child run is launched from the
  # task and must guard recursion and select its workflow there. The other
  # kinds ignore the subrun keys.
  defp run_opts(state, %Node{kind: :subrun} = node, attempt_n) do
    %{
      run_id: state.graph.run_id,
      attempt: attempt_n,
      engine: state.engine,
      store_opts: state.store_opts,
      subrun_depth: state.subrun_depth,
      subrun_ancestors: state.subrun_ancestors,
      resolved_inputs: resolve_inputs(state.graph, node)
    }
  end

  # An agent node carries the placement module so `Engine.Client` can
  # resolve an `:ixvm` envelope to the run's own provisioned room-server
  # by `run_id`. `ensure_placement/2` already ran before this node was
  # scheduled, so the registry entry exists for an `:ixvm`/`:host` location.
  #
  # The engine turn runs from the run's primary-repo checkout, so the cwd
  # is read back from the resolved placement (`:host` and `:ixvm` clone to
  # different roots). A run with no acquired placement (`:local`/`:room`)
  # has no checkout to name and omits `:cwd`; the engine client then fails
  # loudly with `:missing_cwd` rather than running an agent turn in an
  # unknown directory.
  defp run_opts(state, %Node{kind: :agent}, attempt_n) do
    base = %{
      run_id: state.graph.run_id,
      attempt: attempt_n,
      placement: state.placement,
      trigger: state.graph.trigger
    }

    case state.placement.workspace_cwd(state.graph.run_id, placement_opts(state)) do
      {:ok, cwd} -> Map.put(base, :cwd, cwd)
      :error -> base
    end
  end

  defp run_opts(state, %Node{}, attempt_n) do
    %{run_id: state.graph.run_id, attempt: attempt_n}
  end

  # Provision the run's own room-server before its first agent turn when
  # the node's placement needs one (`:ixvm` or `{:host, _}`). Acquisition
  # is idempotent and run-scoped: one room-server serves every agent node
  # in the run, so only the first such agent node provisions; the rest
  # reuse it. `:local` and `{:room, _}` resolve to a fixed URL in the
  # client and need no per-run server, so they are a no-op here. The
  # `ixvm -> host` fallback (target from `Config.placement_fallback`) lives
  # inside `Placement.acquire`; an `ixvm` failure that falls back to
  # `:local` returns `{:error, {:no_placement_needed, :local}}`, which is a
  # resolved outcome (the client uses the default URL), not an acquire
  # failure. Teardown at run end releases whatever was acquired.
  defp ensure_placement(%{placement_acquired?: true} = state, _node), do: state

  defp ensure_placement(state, %Node{kind: :agent, envelope: %{location: location}})
       when location == :ixvm or (is_tuple(location) and elem(location, 0) == :host) do
    case state.placement.acquire(state.graph.run_id, location, acquire_opts(state)) do
      {:ok, _base_url} ->
        graph = stamp_placement(state.graph, state.placement, location)
        %{state | graph: graph, placement_acquired?: true}

      # The fallback chose `:local`: no per-run server, the turn resolves
      # to the default URL. Mark acquired so later agent nodes do not retry.
      {:error, {:no_placement_needed, :local}} ->
        graph = %{state.graph | placement: %{declared: location, effective: :local}}
        %{state | graph: graph, placement_acquired?: true}

      {:error, reason} ->
        # Setup (and any configured fallback) failed; log and leave the
        # engine turn to fail against the missing placement. Mark acquired
        # so a per-node retry does not re-provision what just failed.
        Logger.warning("Runtime #{state.graph.run_id} placement acquire failed: #{inspect(reason)}")
        %{state | placement_acquired?: true}
    end
  end

  defp ensure_placement(state, _node), do: state

  # Read the resolved placement from the registry (effective location after
  # any ixvm -> host fallback) and stamp it onto the graph so the read view
  # can expose "ixvm (fallback host)" without re-querying ETS on every read.
  defp stamp_placement(%RunGraph{} = graph, placement_mod, declared) do
    effective =
      case placement_mod.resolved(graph.run_id) do
        {:ok, %{location: loc}} -> loc
        :error -> nil
      end

    %{graph | placement: %{declared: declared, effective: effective}}
  end

  defp placement_opts(state), do: Keyword.get(state.store_opts, :placement_opts, [])

  # Acquiring a run's placement clones its repos and boots the room-server
  # the agent turn runs against. When a GitHub App is configured, mint an
  # installation token and pass it as `:bot_token` so the clone auth header
  # and the room-server `GITHUB_TOKEN`/`GH_TOKEN` author agent PRs under the
  # App's bot identity. Without this the placement falls back to the static
  # host `config.github_token`, and `gh pr create` authors PRs as that human
  # account (ENG-2012, indexable-inc/symphony#242). An explicit `:bot_token`
  # in `placement_opts` (tests) is left untouched.
  defp acquire_opts(state) do
    opts = placement_opts(state)

    if Keyword.has_key?(opts, :bot_token) do
      opts
    else
      case bot_token() do
        {:ok, token} -> Keyword.put(opts, :bot_token, token)
        :none -> opts
      end
    end
  end

  # Best-effort, mirroring `Runtime.ExecRunner`: a missing or unconfigured
  # GitHub App (dev laptops, tests) yields no token and the placement keeps
  # the inherited env rather than crashing the run.
  defp bot_token do
    if GithubApp.configured?() do
      case GithubApp.installation_token() do
        {:ok, token} ->
          {:ok, token}

        {:error, reason} ->
          Logger.warning("Runtime: GitHub App token mint failed (#{inspect(reason)}); agent placement uses the static host token")
          :none
      end
    else
      :none
    end
  rescue
    error ->
      Logger.warning("Runtime: bot identity unavailable (#{inspect(error)}); agent placement uses the static host token")
      :none
  end

  # Resolve a node's inputs to concrete values using the outputs of its
  # already-succeeded dependencies. A `{:literal, v}` is the value; a
  # `{:node, id, path}` reads the dependency's output at `path`. A subrun
  # node is only scheduled once every dep succeeded, so every node ref
  # resolves; an unresolvable ref is dropped rather than guessed.
  defp resolve_inputs(%RunGraph{nodes: nodes}, %Node{inputs: inputs}) do
    Enum.reduce(inputs, %{}, fn {name, ref}, acc ->
      case resolve_input_ref(ref, nodes) do
        {:ok, value} -> Map.put(acc, name, value)
        :skip -> acc
      end
    end)
  end

  defp resolve_input_ref({:literal, value}, _nodes), do: {:ok, value}

  defp resolve_input_ref({:node, id, path}, nodes) do
    case Map.fetch(nodes, id) do
      {:ok, %Node{state: :succeeded, output: output}} -> {:ok, dig(output, path)}
      _ -> :skip
    end
  end

  defp resolve_input_ref(_ref, _nodes), do: :skip

  defp dig(value, []), do: value

  defp dig(value, [key | rest]) when is_map(value) do
    dig(Map.get(value, key) || Map.get(value, to_string(key)), rest)
  end

  defp dig(_value, _path), do: nil

  # --- graph transitions ----------------------------------------------

  defp mark_running(%RunGraph{} = graph, %Node{} = node, attempt_n) do
    attempt = Attempt.start(attempt_n, attempt_engine(node))
    updated = %{node | state: :running, attempts: node.attempts ++ [attempt], updated_at: DateTime.utc_now()}
    %{graph | nodes: Map.put(graph.nodes, node.id, updated), updated_at: DateTime.utc_now()}
  end

  defp record_finished_attempt(%RunGraph{} = graph, node_id, result, thread_id) do
    case Map.fetch(graph.nodes, node_id) do
      {:ok, %Node{attempts: []} = node} ->
        attempt =
          attempt_n_seed()
          |> Attempt.start(attempt_engine(node), thread_id)
          |> Attempt.finish(attempt_state_for(result), outcome_for(result), cost_for(result))

        put_node(graph, %{node | attempts: [attempt]})

      {:ok, node} ->
        attempts = finish_current_attempt(node.attempts, result, thread_id)
        put_node(graph, %{node | attempts: attempts})

      :error ->
        graph
    end
  end

  defp finish_current_attempt(attempts, result, thread_id) do
    current = Enum.max_by(attempts, & &1.n)
    finished = %{Attempt.finish(current, attempt_state_for(result), outcome_for(result), cost_for(result)) | thread_id: thread_id}
    Enum.map(attempts, fn a -> if a.n == current.n, do: finished, else: a end)
  end

  defp strand_node(%RunGraph{} = graph, node_id) do
    case Map.fetch(graph.nodes, node_id) do
      {:ok, node} ->
        graph = mark_attempt_stranded(graph, node)
        node = graph.nodes[node_id]

        if Recovery.auto_retryable?(node) do
          # Mark :retrying so the next scheduling pass picks it up
          # (`Graph.ready_nodes/1` treats :retrying as schedulable). The
          # attempt history, including the stranded attempt just recorded,
          # is preserved so the retry budget and audit trail survive.
          transition(graph, node_id, :retrying)
        else
          transition(graph, node_id, :stranded)
        end

      :error ->
        graph
    end
  end

  defp mark_attempt_stranded(%RunGraph{} = graph, %Node{attempts: []} = node) do
    attempt = Attempt.start(1, attempt_engine(node)) |> Attempt.finish(:stranded, :stranded)
    put_node(graph, %{node | attempts: [attempt]})
  end

  defp mark_attempt_stranded(%RunGraph{} = graph, %Node{attempts: attempts} = node) do
    current = Enum.max_by(attempts, & &1.n)
    finished = Attempt.finish(current, :stranded, :stranded)
    updated = Enum.map(attempts, fn a -> if a.n == current.n, do: finished, else: a end)
    put_node(graph, %{node | attempts: updated})
  end

  defp transition(%RunGraph{} = graph, node_id, state) do
    case Map.fetch(graph.nodes, node_id) do
      {:ok, node} -> put_node(graph, %{node | state: state})
      :error -> graph
    end
  end

  defp put_node(%RunGraph{} = graph, %Node{} = node) do
    updated = %{node | updated_at: DateTime.utc_now()}
    %{graph | nodes: Map.put(graph.nodes, node.id, updated), updated_at: DateTime.utc_now()}
  end

  defp drop_task_for(state, node_id) do
    case Map.fetch(state.node_refs, node_id) do
      {:ok, ref} ->
        Process.demonitor(ref, [:flush])
        %{state | tasks: Map.delete(state.tasks, ref), node_refs: Map.delete(state.node_refs, node_id)}

      :error ->
        state
    end
  end

  # --- run resolution -------------------------------------------------

  defp finish(state) do
    status = Graph.finished_status(state.graph)
    graph = %{state.graph | status: status}
    persist(graph, state)
    Logger.info("Runtime #{graph.run_id} finished with status=#{status}")
    notify_finished(graph)
    finished = %{state | graph: graph}

    # A succeeded run is done and its process stops, so release the per-run
    # room-server now. A failed run stays alive for the operator surface
    # (clear_failed/retry/rerun may schedule more agent turns against the
    # same placement), so keep it until the run truly ends through cancel
    # or a later success.
    unless status == :failed, do: release_placement(finished)
    finished
  end

  defp deadlock(state) do
    Logger.error("Runtime #{state.graph.run_id} deadlocked: no ready nodes, no live tasks, run not terminal")
    graph = %{state.graph | status: :failed}
    persist(graph, state)
    notify_finished(graph)
    deadlocked = %{state | graph: graph}
    release_placement(deadlocked)
    deadlocked
  end

  # Fire the terminal Slack summary off the runtime process so a slow Slack
  # round-trip never stalls run resolution. Best-effort: the notifier swallows
  # its own failures and the channel/token may be unset.
  defp notify_finished(%RunGraph{} = graph) do
    Task.Supervisor.start_child(SymphonyElixir.TaskSupervisor, fn ->
      SymphonyElixir.IR.RunNotifier.notify_finished(graph)
    end)

    :ok
  end

  # Tear down the run's per-run room-server, if it acquired one. Run-scoped
  # and idempotent: a `:local`/`:room` run never acquired a placement, so
  # this is a no-op for it. Wrapped so a teardown failure (a slow `ix rm`,
  # an unreachable VM) never blocks the run from resolving. The placement
  # module is the one threaded into state, so a test injects a fake.
  defp release_placement(%{placement: placement, graph: %RunGraph{run_id: run_id}}) do
    placement.release(run_id)
    :ok
  rescue
    error ->
      Logger.warning("Runtime #{run_id} placement release failed: #{inspect(error)}")
      :ok
  end

  defp release_placement(_state), do: :ok

  # Persist then announce. The store write is the durable record; the
  # PubSub broadcast is the live notification the dashboard subscribes to,
  # so the operator sees a transition without polling. Announcing only
  # after a successful persist keeps a subscriber's refresh-from-store path
  # consistent with the event. A failed broadcast (no subscribers, dead
  # PubSub) never blocks the run: the durable state already landed.
  defp persist(%RunGraph{} = graph, state) do
    case SymphonyElixir.IR.Store.persist(graph, state.store_opts) do
      :ok ->
        Events.broadcast(graph)
        :ok

      {:error, reason} ->
        Logger.warning("Runtime #{graph.run_id} persist failed: #{inspect(reason)}")
        :ok
    end
  end

  # --- helpers --------------------------------------------------------

  defp attempt_n_seed, do: 1

  # Dispatch one attempt by node kind. Only `:agent` nodes are engine
  # turns and go through the injected engine client; `:exec` runs a pack
  # script locally; `:subrun` launches a nested run through `SubrunRunner`
  # and maps its terminal state back to one result triple. Placeholder
  # kinds never reach here: `Graph.ready_nodes/1` excludes them.
  defp run_attempt(%Node{kind: :agent} = node, engine, run_opts), do: engine.run_node(node, run_opts)
  defp run_attempt(%Node{kind: :exec} = node, _engine, run_opts), do: ExecRunner.run(node, run_opts)
  defp run_attempt(%Node{kind: :subrun} = node, _engine, run_opts), do: SubrunRunner.run(node, run_opts)

  # An attempt records what executed it. Agent attempts carry the engine;
  # exec/subrun carry the executor kind so the run record is honest about a
  # node that never touched an engine.
  defp attempt_engine(%Node{kind: :agent, envelope: %{engine: engine}}) when engine in [:codex, :claude, :pi],
    do: engine

  defp attempt_engine(%Node{kind: :exec}), do: :exec
  defp attempt_engine(%Node{kind: :subrun}), do: :subrun
  defp attempt_engine(_node), do: :codex

  defp attempt_state_for({:ok, _}), do: :succeeded
  defp attempt_state_for({:error, _}), do: :failed

  defp outcome_for({:ok, _}), do: :ok
  defp outcome_for({:error, reason}), do: {:error, reason}

  # Per-turn cost rides on the successful result's output map (the engine
  # client lowers the room-server `usage` totals to the `Attempt.cost`
  # shape there). A failure carries only the error reason on the
  # synchronous path, so its cost is unknown (nil), and an exec/subrun
  # output without a cost key is also nil.
  defp cost_for({:ok, output}) when is_map(output) do
    case Map.get(output, :cost) do
      cost when is_map(cost) -> cost
      _ -> nil
    end
  end

  defp cost_for(_), do: nil
end
