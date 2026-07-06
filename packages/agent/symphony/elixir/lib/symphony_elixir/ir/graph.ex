defmodule SymphonyElixir.IR.Graph do
  @moduledoc """
  Pure operations over a `RunGraph`. No process state, no IO: every
  function takes a graph (and arguments) and returns a new graph or a
  derived value. The `Runtime` GenServer owns the side effects
  (scheduling tasks, persisting); this module owns the graph algebra it
  schedules against.

  The two load-bearing rules:

  - Dependency satisfaction is read off `IR.Node.deps`, which is itself
    derived from `inputs` (`IR.Node.deps_from_inputs/1`). A node is ready
    only when every dep has reached `:succeeded`.
  - Failure propagates. When a node fails, each transitive dependent that
    is still waiting transitions to `:upstream_failed` unless its trigger
    rule opts to run on failure. A dependent already running or terminal
    is left alone; the runtime reconciles those through the task path.

  `ready_nodes/1` is the scheduler's input, `apply_output/3` is the
  scheduler's commit step, and `reset_node/2` is the retry path. The
  attempt bookkeeping (`mark_running/3`, `record_finished_attempt/4`,
  `mark_attempt_stranded/2`, ...) lives here too: how a node's attempt
  history evolves is graph algebra, even though the runtime decides when.
  """

  alias SymphonyElixir.IR.{Attempt, Node, RunGraph}

  @doc """
  Nodes that may start now: state `:pending`, `:ready`, or `:retrying`,
  with every dep `:succeeded`. Running and terminal nodes are excluded, so
  calling this on a graph with live tasks never reschedules an in-flight
  node. A `:retrying` node is one a crash stranded that policy cleared for
  another attempt; it is eligible for a fresh schedule while its attempt
  history is preserved.

  A node with no deps is ready immediately. The result order is stable
  (sorted by id) so a deterministic replay schedules in a deterministic
  order.
  """
  @schedulable [:pending, :ready, :retrying]

  # `:gate` and `:map_fanout` are dynamic-expansion placeholders, not work
  # to run. The materializer retires a resolved one to `:skipped` before
  # the next schedule pass, but excluding the kinds here is the guard that
  # holds even if a placeholder's deps are satisfied before expansion runs,
  # so one is never handed to an executor as if it were an agent turn.
  @placeholder_kinds [:gate, :map_fanout]

  @spec ready_nodes(RunGraph.t()) :: [Node.t()]
  def ready_nodes(%RunGraph{nodes: nodes}) do
    nodes
    |> Map.values()
    |> Enum.filter(fn node ->
      node.kind not in @placeholder_kinds and node.state in @schedulable and deps_satisfied?(node, nodes)
    end)
    |> Enum.sort_by(& &1.id)
  end

  @doc "Whether every dependency of `node` has succeeded. A node with no deps is satisfied."
  @spec deps_satisfied?(Node.t(), %{String.t() => Node.t()}) :: boolean()
  def deps_satisfied?(%Node{deps: deps}, nodes) when is_map(nodes) do
    Enum.all?(deps, fn dep_id ->
      case Map.fetch(nodes, dep_id) do
        {:ok, %Node{state: :succeeded}} -> true
        _ -> false
      end
    end)
  end

  @doc """
  Record the result of a node's attempt and re-derive dependent states.

  `result` is `{:ok, output}` or `{:error, reason}`. On success the node
  becomes `:succeeded` carrying `output`, which can unlock dependents on
  the next `ready_nodes/1`. On failure the node becomes `:failed` and
  failure propagates to dependents that do not opt to run on failure
  (see `trigger_runs_on_failure?/1`).

  Marking a node terminal is idempotent in the sense that re-applying the
  same result yields the same graph; callers that already moved a node to
  a terminal state via reconciliation should not re-apply.
  """
  @spec apply_output(RunGraph.t(), String.t(), {:ok, term()} | {:error, term()}) :: RunGraph.t()
  def apply_output(%RunGraph{} = graph, node_id, result) do
    case Map.fetch(graph.nodes, node_id) do
      {:ok, node} -> commit_result(graph, node, result)
      :error -> graph
    end
  end

  defp commit_result(%RunGraph{} = graph, %Node{} = node, {:ok, output}) do
    updated = %{node | state: :succeeded, output: output, updated_at: DateTime.utc_now()}
    %{graph | nodes: Map.put(graph.nodes, node.id, updated), updated_at: DateTime.utc_now()}
  end

  defp commit_result(%RunGraph{} = graph, %Node{} = node, {:error, reason}) do
    updated = %{node | state: :failed, output: {:error, reason}, updated_at: DateTime.utc_now()}

    graph
    |> Map.put(:nodes, Map.put(graph.nodes, node.id, updated))
    |> Map.put(:updated_at, DateTime.utc_now())
    |> propagate_upstream_failed(node.id)
  end

  @doc """
  Mark every node transitively downstream of `failed_id` that is still
  waiting (`:pending`/`:ready`) and does not run on failure as
  `:upstream_failed`. Already-running or terminal dependents are left for
  the runtime's task path to resolve. Idempotent: nodes already
  `:upstream_failed` stop the walk.
  """
  @spec propagate_upstream_failed(RunGraph.t(), String.t()) :: RunGraph.t()
  def propagate_upstream_failed(%RunGraph{nodes: nodes} = graph, failed_id) do
    now = DateTime.utc_now()

    updated =
      nodes
      |> direct_dependents(failed_id)
      |> Enum.reduce(nodes, fn dependent, acc ->
        if dependent.state in @schedulable and not trigger_runs_on_failure?(dependent) do
          marked = %{dependent | state: :upstream_failed, updated_at: now}
          acc = Map.put(acc, dependent.id, marked)
          # Recurse so a chain a -> b -> c fails c when a fails, not just b.
          %{nodes: deeper} = propagate_upstream_failed(%{graph | nodes: acc}, dependent.id)
          deeper
        else
          acc
        end
      end)

    %{graph | nodes: updated, updated_at: now}
  end

  defp direct_dependents(nodes, dep_id) do
    nodes
    |> Map.values()
    |> Enum.filter(fn node -> dep_id in node.deps end)
  end

  @doc """
  The trigger rule for failure propagation. The default is conservative:
  a node does not run once a dependency failed. A node opts in by carrying
  `inputs["__on_failure__"]` set to the `{:literal, true}` sentinel, which
  the interpreter emits for combinators that want to observe a failed
  upstream (error handlers, cleanup). Kept narrow on purpose; widen only
  when a combinator needs it.
  """
  @spec trigger_runs_on_failure?(Node.t()) :: boolean()
  def trigger_runs_on_failure?(%Node{inputs: inputs}) when is_map(inputs) do
    Map.get(inputs, "__on_failure__") == {:literal, true}
  end

  def trigger_runs_on_failure?(_node), do: false

  @doc """
  Reset a node back to `:pending` for a retry, clearing its prior output
  while preserving the attempt history. The caller decides retry policy;
  this is the pure state transition the retry path uses.
  """
  @spec reset_node(RunGraph.t(), String.t()) :: RunGraph.t()
  def reset_node(%RunGraph{} = graph, node_id) do
    case Map.fetch(graph.nodes, node_id) do
      {:ok, node} ->
        reset = %{node | state: :pending, output: nil, updated_at: DateTime.utc_now()}
        %{graph | nodes: Map.put(graph.nodes, node_id, reset), updated_at: DateTime.utc_now()}

      :error ->
        graph
    end
  end

  # --- attempt bookkeeping --------------------------------------------

  @doc """
  Mark a node `:running` and open attempt `attempt_n` on it. The attempt
  records what will execute it (engine for agent turns, executor kind
  otherwise), so the run record is honest even before the attempt ends.
  """
  @spec mark_running(RunGraph.t(), Node.t(), pos_integer()) :: RunGraph.t()
  def mark_running(%RunGraph{} = graph, %Node{} = node, attempt_n) do
    attempt = Attempt.start(attempt_n, attempt_engine(node))
    updated = %{node | state: :running, attempts: node.attempts ++ [attempt], updated_at: DateTime.utc_now()}
    %{graph | nodes: Map.put(graph.nodes, node.id, updated), updated_at: DateTime.utc_now()}
  end

  @doc """
  Close the node's current attempt with `result` and stamp the attempt's
  reattach handle to `thread_id`. A node with no open attempt (its
  `:running` mark never persisted before the result arrived) gets one
  synthesized so the run record still explains the outcome. An unknown
  node id is a no-op.
  """
  @spec record_finished_attempt(RunGraph.t(), String.t(), {:ok, term()} | {:error, term()}, String.t() | nil) ::
          RunGraph.t()
  def record_finished_attempt(%RunGraph{} = graph, node_id, result, thread_id) do
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

  @doc """
  Stamp the current attempt's reattach handle mid-flight (an executor
  reports its engine thread or child run id as soon as it opens one, so a
  crash before the result still leaves a handle to reconcile against).
  Only a `:running` node with an open attempt is stamped; anything else is
  a no-op because the terminal path already recorded the handle.
  """
  @spec record_attempt_thread_id(RunGraph.t(), String.t(), String.t()) :: RunGraph.t()
  def record_attempt_thread_id(%RunGraph{} = graph, node_id, thread_id) do
    case Map.fetch(graph.nodes, node_id) do
      {:ok, %Node{attempts: []}} ->
        graph

      {:ok, %Node{state: :running, attempts: attempts} = node} ->
        current = Enum.max_by(attempts, & &1.n)
        updated = Enum.map(attempts, fn a -> if a.n == current.n, do: %{a | thread_id: thread_id}, else: a end)
        put_node(graph, %{node | attempts: updated})

      {:ok, _node} ->
        graph

      :error ->
        graph
    end
  end

  @doc """
  Close the node's current attempt as `:stranded`: its owning task died
  without reporting a result, so the attempt may or may not have acted. A
  node with no recorded attempt gets one synthesized so the run record
  still explains the strand. Routing (retry vs human review) is the
  caller's policy; this only records the fact.
  """
  @spec mark_attempt_stranded(RunGraph.t(), Node.t()) :: RunGraph.t()
  def mark_attempt_stranded(%RunGraph{} = graph, %Node{attempts: []} = node) do
    attempt = Attempt.start(1, attempt_engine(node)) |> Attempt.finish(:stranded, :stranded)
    put_node(graph, %{node | attempts: [attempt]})
  end

  def mark_attempt_stranded(%RunGraph{} = graph, %Node{attempts: attempts} = node) do
    current = Enum.max_by(attempts, & &1.n)
    finished = Attempt.finish(current, :stranded, :stranded)
    updated = Enum.map(attempts, fn a -> if a.n == current.n, do: finished, else: a end)
    put_node(graph, %{node | attempts: updated})
  end

  @doc """
  Set a node's state without touching its output or attempts. The bare
  transition the runtime uses for states no result carries (`:cancelled`,
  `:upstream_failed`, `:retrying`, `:stranded`). An unknown node id is a
  no-op.
  """
  @spec transition(RunGraph.t(), String.t(), Node.state()) :: RunGraph.t()
  def transition(%RunGraph{} = graph, node_id, state) do
    case Map.fetch(graph.nodes, node_id) do
      {:ok, node} -> put_node(graph, %{node | state: state})
      :error -> graph
    end
  end

  defp put_node(%RunGraph{} = graph, %Node{} = node) do
    updated = %{node | updated_at: DateTime.utc_now()}
    %{graph | nodes: Map.put(graph.nodes, node.id, updated), updated_at: DateTime.utc_now()}
  end

  defp finish_current_attempt(attempts, result, thread_id) do
    current = Enum.max_by(attempts, & &1.n)
    finished = %{Attempt.finish(current, attempt_state_for(result), outcome_for(result), cost_for(result)) | thread_id: thread_id}
    Enum.map(attempts, fn a -> if a.n == current.n, do: finished, else: a end)
  end

  defp attempt_n_seed, do: 1

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

  @doc "Nodes currently `:running`. The runtime owns the live task for each of these."
  @spec running_nodes(RunGraph.t()) :: [Node.t()]
  def running_nodes(%RunGraph{nodes: nodes}) do
    nodes |> Map.values() |> Enum.filter(&(&1.state == :running))
  end

  @doc "Whether every node has reached a terminal state (`IR.Node.terminal_states/0`)."
  @spec all_terminal?(RunGraph.t()) :: boolean()
  def all_terminal?(%RunGraph{nodes: nodes}) do
    nodes != %{} and Enum.all?(Map.values(nodes), &Node.terminal?/1)
  end

  @doc "Whether any node has failed (`:failed`) or could not run (`:upstream_failed`)."
  @spec any_failed?(RunGraph.t()) :: boolean()
  def any_failed?(%RunGraph{nodes: nodes}) do
    Enum.any?(Map.values(nodes), &(&1.state in [:failed, :upstream_failed]))
  end

  @doc """
  The terminal run status implied by the node states, or `:running` when
  work remains. A run with any failed/upstream_failed node finishes
  `:failed`; an all-succeeded/skipped/cancelled graph finishes
  `:succeeded`. Used by the runtime to stamp the final `RunGraph.status`.

  An empty node map is a no-op run (every gate resolved its body off) and
  finishes `:succeeded`. This is the deliberate counterpart to
  `all_terminal?/1`, which keeps an empty map non-terminal so a run is
  never declared done before its first materialization; the runtime only
  reaches here once a `:running` graph has no schedulable work left.
  """
  @spec finished_status(RunGraph.t()) :: RunGraph.status() | :running
  def finished_status(%RunGraph{nodes: nodes}) when map_size(nodes) == 0, do: :succeeded

  def finished_status(%RunGraph{} = graph) do
    cond do
      not all_terminal?(graph) -> :running
      any_failed?(graph) -> :failed
      Enum.any?(Map.values(graph.nodes), &(&1.state == :cancelled)) -> :cancelled
      true -> :succeeded
    end
  end
end
